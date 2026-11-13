#!/usr/bin/env python3
"""Exercise the debug DFlow binary in a real Linux pseudoterminal."""

import codecs
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", *args], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


class Screen:
    """Track the cells drawn by the TUI, rather than matching stale redraws."""

    def __init__(self, width: int = 120, height: int = 30) -> None:
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.x = self.y = 0
        self.decoder = codecs.getincrementaldecoder("utf-8")()
        self.escape = ""

    def resize(self, width: int, height: int) -> None:
        self.cells = [(row[:width] + [" "] * max(0, width - len(row))) for row in self.cells[:height]]
        self.cells.extend([[" "] * width for _ in range(height - len(self.cells))])
        self.width, self.height = width, height
        self.x = min(self.x, width - 1)
        self.y = min(self.y, height - 1)

    def feed(self, data: bytes) -> None:
        for char in self.decoder.decode(data):
            if self.escape:
                self.escape += char
                if self.escape == "\x1b[":
                    continue
                if self.escape.startswith("\x1b["):
                    if "@" <= char <= "~":
                        self.csi(self.escape[2:-1], char)
                        self.escape = ""
                    continue
                if self.escape == "\x1b]":
                    continue
                if self.escape.startswith("\x1b]"):
                    if char == "\x07" or self.escape.endswith("\x1b\\"):
                        self.escape = ""
                    continue
                self.escape = ""
            elif char == "\x1b":
                self.escape = char
            elif char == "\r":
                self.x = 0
            elif char == "\n":
                self.y = min(self.y + 1, self.height - 1)
            elif char == "\b":
                self.x = max(0, self.x - 1)
            elif char >= " " and self.x < self.width and self.y < self.height:
                self.cells[self.y][self.x] = char
                self.x += 1

    def csi(self, params: str, command: str) -> None:
        numbers = [int(part) if part.isdecimal() else 0 for part in params.split(";")]
        first = numbers[0] if numbers else 0
        if command in "Hf":
            self.y = min(max(first - 1, 0), self.height - 1)
            self.x = min(max((numbers[1] if len(numbers) > 1 else 1) - 1, 0), self.width - 1)
        elif command == "G":
            self.x = min(max(first - 1, 0), self.width - 1)
        elif command == "A":
            self.y = max(0, self.y - (first or 1))
        elif command == "B":
            self.y = min(self.height - 1, self.y + (first or 1))
        elif command == "C":
            self.x = min(self.width - 1, self.x + (first or 1))
        elif command == "D":
            self.x = max(0, self.x - (first or 1))
        elif command == "J" and first in (0, 2):
            self.cells = [[" "] * self.width for _ in range(self.height)]
        elif command == "K":
            self.cells[self.y][self.x:] = [" "] * (self.width - self.x)

    def rows(self) -> list[str]:
        return ["".join(row) for row in self.cells]


def canvas(screen: Screen) -> str:
    rows = screen.rows()
    assert rows[0].startswith("┌─"), "header border was not drawn without a title"
    assert rows[3][0] == "└" and rows[4].startswith("┌─"), "graph border was not drawn below the header without a title"
    assert "q quit" not in "\n".join(rows[:4]), "obsolete quit hint appeared in the header"
    assert rows[4][-1] == "┐" and rows[-1][0] == "└" and rows[-1][-1] == "┘", "graph border did not fill terminal width and height"
    image = "\n".join(rows)
    assert "Changes" not in image, "obsolete Changes panel appeared"
    return image


def input_and_redraw(master: int, process: subprocess.Popen[bytes], screen: Screen, key: bytes, marker: bytes | None) -> bytes:
    os.write(master, key)
    output = collect(master, process, marker, 0.3 if marker is None else 5)
    assert output and (marker is None or marker in output), f"{key!r} did not redraw {marker!r}"
    if marker is not None:
        output += collect(master, process, None, 0.2)
    screen.feed(output)
    return output




def collect(master: int, process: subprocess.Popen[bytes], marker: bytes | None, seconds: float = 5) -> bytes:
    output = bytearray()
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.05)
        if ready:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            output.extend(chunk)
            if marker is not None and marker in output:
                break
        elif process.poll() is not None:
            break
    return bytes(output)


def run_case(binary: Path, root: Path, *, marker: bytes | None, keys: bytes = b"", fault: str | None = None, resize: bool = False, help_key: bool = False, expand_node: bool = False, node_title: bytes | None = None, node_path: bytes | None = None, canvas_titles: tuple[str, str] | None = None, edge_glyphs: bool = False) -> tuple[int, bytes]:
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
    before = termios.tcgetattr(slave)
    env = os.environ.copy()
    env.pop("NO_COLOR", None)
    env["TERM"] = "xterm-256color"
    if fault:
        env["DFLOW_INTERNAL_TUI_FAULT"] = fault
    process = subprocess.Popen(
        [str(binary), "local", "--format", "tui"],
        cwd=root,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
    )
    try:
        output = collect(master, process, marker)
        if marker is not None and marker not in output:
            raise AssertionError(f"TUI marker {marker!r} did not appear")
        if marker is not None:
            output += collect(master, process, None, 0.2)
        screen = Screen()
        screen.feed(output)
        if canvas_titles is not None:
            image = canvas(screen)
            for title in canvas_titles:
                assert title in image, f"unrelated changed root {title!r} was not visible on initial screen"
            assert "complete" in image
            assert "base " in image and "target " in image, "both snapshot identities were not shown"
        if edge_glyphs:
            def check_edges() -> None:
                image = canvas(screen)
                assert "+→" in image and "-→" in image, "changed edge markers disappeared from visible collapsed graph"
            check_edges()
            assert "0 symbols | 2 edge changes" in canvas(screen)
            output += input_and_redraw(master, process, screen, b"d", None)
            assert "Edge deleted [base]" in canvas(screen), "expanded snapshot edge evidence missing"
            output += input_and_redraw(master, process, screen, b"d", None)
            check_edges()
            for key in (b"j", b"\x1b[1;2C", b"\x1b[1;2D", b"+", b"-"):
                output += input_and_redraw(master, process, screen, key, None)
                check_edges()
        if expand_node:
            assert b"Details" not in output, "separate Details panel appeared initially"
            assert node_title is not None and node_title in output, "node title was not visible"
            assert node_path is not None and node_path.decode() in canvas(screen), "collapsed node path was not visible"
            expanded_output = input_and_redraw(master, process, screen, b"d", b"Source [target]:")
            assert b"Details" not in expanded_output, "separate Details panel appeared after d"
            output += expanded_output
        if canvas_titles is not None:
            assert "Source [target]:" in canvas(screen), "d did not expand selected-node source in the graph"
            before_pan = screen.rows()[5:-1]
            output += input_and_redraw(master, process, screen, b"\x1b[1;2C", None)
            assert screen.rows()[5:-1] != before_pan, "Shift+Right did not pan the expanded wide node"
            output += input_and_redraw(master, process, screen, b"\x1b[6~", None)
            output += input_and_redraw(master, process, screen, b"d", None)
            image = canvas(screen)
            for title in canvas_titles:
                assert title in image, f"changed root {title!r} disappeared after collapse"
            depth = int(re.search(r"depth: (\d+)", image).group(1))
            output += input_and_redraw(master, process, screen, b"+", None)
            assert f"depth: {depth + 1}" in canvas(screen), "depth increase did not update header"
            output += input_and_redraw(master, process, screen, b"-", None)
            image = canvas(screen)
            for title in canvas_titles:
                assert title in image, f"changed root {title!r} disappeared after depth refresh"
            output += input_and_redraw(master, process, screen, b"j", None)
        if resize:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 100, 0, 0))
            screen.resize(100, 20)
            os.kill(process.pid, signal.SIGWINCH)
            resized_output = collect(master, process, None, 0.3)
            assert resized_output, "resize did not redraw graph"
            resized_output += collect(master, process, None, 0.2)
            screen.feed(resized_output)
            if canvas_titles is not None:
                canvas(screen)
            output += resized_output
            if edge_glyphs:
                check_edges()
        if help_key:
            help_output = input_and_redraw(master, process, screen, b"?", None)
            assert "DFlow keyboard help" in "\n".join(screen.rows()), "help key did not open help"
            output += help_output
            if canvas_titles is not None:
                output += input_and_redraw(master, process, screen, b"?", None)
                canvas(screen)
        if keys:
            os.write(master, keys)
        process.wait(timeout=5)
        output += collect(master, process, None, 0.2)
        after = termios.tcgetattr(slave)
        assert before == after, "terminal mode was not fully restored"
        assert b"\x1b[?1049h" in output, "alternate screen was not entered"
        assert b"\x1b[?1049l" in output, "alternate screen was not left"
        return process.returncode, output
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


def live_case(binary: Path, root: Path, *, staged: bool = False) -> None:
    source = root / "a.py"
    base = "def B(x): return x\ndef C(x): return x\ndef A(x): return B(x)\n"
    changed = "def B(x): return x\ndef C(x): return x\ndef A(x): return C(x)\n"
    source.write_text(base)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
    before = termios.tcgetattr(slave)
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    command = [str(binary), "local", "--format", "tui"]
    if staged:
        command.append("--staged")
    process = subprocess.Popen(command, cwd=root, stdin=slave, stdout=slave, stderr=slave, env=env)
    screen = Screen()
    output = bytearray()

    def expect(predicate, label: str, timeout: float = 3) -> str:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                screen.feed(chunk)
                image = "\n".join(screen.rows())
                rows = screen.rows()
                border_ready = (
                    rows[0].startswith("┌─")
                    and rows[3][0] == "└"
                    and rows[4].startswith("┌─")
                    and rows[4][-1] == "┐"
                    and rows[-1][0] == "└"
                    and rows[-1][-1] == "┘"
                )
                if predicate(image) and border_ready:
                    return canvas(screen)
            if process.poll() is not None:
                break
        raise AssertionError(f"live TUI did not show {label} within {timeout}s: {''.join(screen.rows()[:4])}")

    def settle() -> None:
        chunk = collect(master, process, None, 0.25)
        output.extend(chunk)
        screen.feed(chunk)

    try:
        expect(lambda image: "No symbol or edge changes" in image and "complete" in image, "initial empty graph")
        source.write_text(changed)
        if staged:
            chunk = collect(master, process, None, 1.2)
            output.extend(chunk)
            screen.feed(chunk)
            assert "No symbol or edge changes" in canvas(screen), "unstaged worktree edit changed staged graph"
            git(root, "add", "a.py")
        image = expect(lambda image: "0 symbols | 2 edge changes" in image and "+→" in image and "-→" in image, "changed edges")
        assert "complete" in image
        if not staged:
            index = root / ".git/index"
            saved_index = index.read_bytes()
            index.write_bytes(b"invalid index")
            image = expect(lambda image: "RELOAD ERROR:" in image, "capture failure")
            assert "+→" in image and "-→" in image, "failed capture discarded the last valid graph"
            index.write_bytes(saved_index)
            expect(lambda image: "RELOAD ERROR:" not in image and "+→" in image and "-→" in image, "capture recovery")
        if not staged:
            os.write(master, b"d")
            expect(lambda image: "Edge added [target]" in image or "Edge deleted [base]" in image, "expanded edge evidence")
            source.write_text(base)
            expect(lambda image: "No symbol or edge changes" in image, "empty graph after repair")
            source.write_text("def A(:\n")
            expect(lambda image: "PARTIAL analysis" in image, "invalid Python partial analysis")
            source.write_text(base)
            expect(lambda image: "No symbol or edge changes" in image and "complete" in image, "recovered complete analysis")
            source.write_text(changed)
            expect(lambda image: "0 symbols | 2 edge changes" in image, "edges after recovery")
            os.write(master, b"j\x1b[1;2C\x1b[1;2D")
            settle()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 100, 0, 0))
            screen.resize(100, 20)
            os.kill(process.pid, signal.SIGWINCH)
            expect(lambda image: "0 symbols | 2 edge changes" in image and "+→" in image and "-→" in image, "resized graph")
        os.write(master, b"\x03" if staged else b"q")
        process.wait(timeout=5)
        output.extend(collect(master, process, None, 0.2))
        assert process.returncode == 0, f"live TUI exit status {process.returncode}"
        assert termios.tcgetattr(slave) == before, "live TUI failed to restore terminal"
        assert b"\x1b[?1049h" in output and b"\x1b[?1049l" in output, "live TUI did not restore alternate screen"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


def main() -> None:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/dflow").resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}; run cargo build first")
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        source = root / "a.py"
        other_source = root / "b.py"
        source.write_text("def f(): return 1\ndef from_a(): return f()\n")
        other_source.write_text("def other(): return 1\ndef from_b(): return other()\n")
        git(root, "init")
        git(root, "config", "user.name", "Fixture")
        git(root, "config", "user.email", "fixture@example.com")
        git(root, "add", ".")
        git(root, "commit", "-m", "base")
        source.write_text("def f(): return 2  # " + "wide-node-evidence-" * 9 + "\ndef from_a(): return f()\n")
        other_source.write_text("def other(): return 2\ndef from_b(): return other()\n")

        code, output = run_case(binary, root, marker=b"depth:", keys=b"q", resize=True, help_key=True, expand_node=True, node_title=b"f", node_path=b"a.py", canvas_titles=("f", "other"))
        assert code == 0 and b"complete" in output
        assert "─".encode() in output, "normal graph did not draw box lines"
        assert b"\x1b[38;5;3;49m" in output, "modified change did not receive yellow terminal styling"
        source.write_text("def B(x): return x\ndef C(x): return x\ndef A(x): return B(x)\n")
        other_source.write_text("")
        git(root, "add", "a.py", "b.py")
        git(root, "commit", "-m", "edge base")
        source.write_text("def B(x): return x\ndef C(x): return x\ndef A(x): return C(x)\n")
        code, output = run_case(binary, root, marker=b"depth:", keys=b"q", resize=True, edge_glyphs=True)
        assert code == 0

        source.write_text("def f(:\n")
        code, output = run_case(binary, root, marker=b"PARTIAL", keys=b"\x03")
        assert code == 1 and b"PARTIAL" in output
        source.write_text("def f(): return 2\ndef from_a(): return f()\n")

        code, output = run_case(binary, root, marker=None, fault="panic")
        assert code != 0 and b"injected terminal panic" in output
        assert output.index(b"\x1b[?1049l") < output.index(b"injected terminal panic")

        code, output = run_case(binary, root, marker=None, fault="error")
        assert code == 1 and b"injected terminal error" in output
        assert output.index(b"\x1b[?1049l") < output.index(b"terminal UI error")

        (root / "src").mkdir()
        (root / "Cargo.toml").write_text('[package]\nname = "pty-rust"\nversion = "0.1.0"\nedition = "2021"\n')
        rust_source = root / "src/lib.rs"
        rust_source.write_text("pub fn rust_root() -> i32 { 1 }\npub fn caller() -> i32 { rust_root() }\n")
        git(root, "add", "a.py", "b.py", "Cargo.toml", "src/lib.rs")
        git(root, "commit", "-m", "rust base")
        rust_source.write_text("pub fn rust_root() -> i32 { 2 }\npub fn caller() -> i32 { rust_root() }\n")
        code, output = run_case(binary, root, marker=b"depth:", keys=b"q", expand_node=True, node_title=b"rust_root", node_path=b"src/lib.rs")
        assert code == 0 and b"rust_root" in output

        live = root / "live"
        live.mkdir()
        (live / "a.py").write_text("def B(x): return x\ndef C(x): return x\ndef A(x): return B(x)\n")
        git(live, "init")
        git(live, "config", "user.name", "Fixture")
        git(live, "config", "user.email", "fixture@example.com")
        git(live, "add", "a.py")
        git(live, "commit", "-m", "live base")
        live_case(binary, live)
        live_case(binary, live, staged=True)
        print("PTY: changed edge markers across redraw, pan, navigation, expansion and resize; Python/Rust graph, partial analysis, faults, and terminal restoration passed")


if __name__ == "__main__":
    main()
