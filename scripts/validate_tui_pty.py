#!/usr/bin/env python3
"""Exercise the debug DFlow binary in a real Linux pseudoterminal."""

import fcntl
import os
from pathlib import Path
import pty
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


def run_case(binary: Path, root: Path, *, marker: bytes | None, keys: bytes = b"", fault: str | None = None, resize: bool = False, help_key: bool = False, expand_node: bool = False, node_title: bytes | None = None) -> tuple[int, bytes]:
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
        if expand_node:
            assert b"Details" not in output, "separate Details panel appeared initially"
            assert node_title is not None and node_title in output, "node title was not visible"
            assert b"Location:" in output, "collapsed node location was not visible"
            os.write(master, b"d")
            expanded_output = collect(master, process, b"Source:")
            assert b"Source:" in expanded_output, "d did not expand source inside the selected node"
            assert b"Details" not in expanded_output, "separate Details panel appeared after d"
            output += expanded_output
        if resize:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 100, 0, 0))
            os.kill(process.pid, signal.SIGWINCH)
            output += collect(master, process, None, 0.2)
        if help_key:
            os.write(master, b"?")
            help_output = collect(master, process, b"Help")
            assert b"Help" in help_output, "help key did not open help"
            output += help_output
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


def main() -> None:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/dflow").resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}; run cargo build first")
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        source = root / "a.py"
        source.write_text("def f(): return 1\n")
        git(root, "init")
        git(root, "config", "user.name", "Fixture")
        git(root, "config", "user.email", "fixture@example.com")
        git(root, "add", ".")
        git(root, "commit", "-m", "base")
        source.write_text("def f(): return 2\n")

        code, output = run_case(binary, root, marker=b"Graph", keys=b"q", resize=True, help_key=True, expand_node=True, node_title=b"a.py:f")
        assert code == 0 and b"complete" in output
        assert "─".encode() in output, "normal graph did not draw box lines"
        assert b"\x1b[38;5;3;49m" in output, "modified change did not receive yellow terminal styling"

        source.write_text("def f(:\n")
        code, output = run_case(binary, root, marker=b"PARTIAL", keys=b"\x03")
        assert code == 1 and b"PARTIAL" in output
        source.write_text("def f(): return 2\n")

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
        git(root, "add", "a.py", "Cargo.toml", "src/lib.rs")
        git(root, "commit", "-m", "rust base")
        rust_source.write_text("pub fn rust_root() -> i32 { 2 }\npub fn caller() -> i32 { rust_root() }\n")
        code, output = run_case(binary, root, marker=b"Graph", keys=b"q", expand_node=True, node_title=b"lib.rs:rust_root")
        assert code == 0 and b"rust_root" in output

        print("PTY: Python and Rust inline node expansion, help, resize, partial Ctrl-C, panic, error, and terminal restoration passed")


if __name__ == "__main__":
    main()
