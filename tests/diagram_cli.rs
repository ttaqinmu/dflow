use git2::{Repository, Signature};
use std::{fs, path::Path, process::{Command, Output}};
use tempfile::TempDir;

const OLD: &str = "def B(): pass\ndef C(): pass\ndef A(): B()\n";
const NEW: &str = "def B(): pass\ndef C(): pass\ndef A(): C()\n";

fn commit(repo: &Repository) {
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.py")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("Fixture", "fixture@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &parent.iter().collect::<Vec<_>>()).unwrap();
}

fn run(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dflow")).current_dir(dir.path()).args(args).output().unwrap()
}

fn assert_retarget(output: &Output, format: &str) {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    let text = std::str::from_utf8(&output.stdout).unwrap();
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
    assert!(!text.contains("```"));
    let mut names = std::collections::BTreeMap::new();
    for line in text.lines() {
        for name in ["A", "B", "C"] {
            let label = if format == "mermaid" {
                format!("{name} #124; a.py #124; both")
            } else {
                format!("{name} | a.py | both")
            };
            if line.contains(&label) {
                let id = line.trim_start().split_once(['[', ' ']).unwrap().0;
                names.insert(name, id.to_owned());
            }
        }
    }
    assert_eq!(names.len(), 3, "{text}");
    if format == "mermaid" {
        assert!(text.starts_with("flowchart LR\n"));
        assert!(text.contains(&format!("{} -->|- base| {}", names["A"], names["B"])));
        assert!(text.contains(&format!("{} -->|+ target| {}", names["A"], names["C"])));
        assert_eq!(text.matches(" -->").count(), 2);
    } else {
        assert!(text.starts_with("digraph dflow {\n  rankdir=LR;\n"));
        assert!(text.contains(&format!("{} -> {} [label=\"- base\"]", names["A"], names["B"])));
        assert!(text.contains(&format!("{} -> {} [label=\"+ target\"]", names["A"], names["C"])));
        assert_eq!(text.matches(" -> ").count(), 2);
    }
    assert!(!text.contains("modified"));
}

#[test]
fn local_staged_unstaged_and_branch_export_retargets() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.py"), OLD).unwrap();
    commit(&repo);
    for format in ["mermaid", "dot"] {
        fs::write(dir.path().join("a.py"), NEW).unwrap();
        assert_retarget(&run(&dir, &["local", "--format", format, "--depth", "0"]), format);
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.py")).unwrap();
        index.write().unwrap();
        assert_retarget(&run(&dir, &["local", "--staged", "--format", format, "--depth", "0"]), format);
        fs::write(dir.path().join("a.py"), OLD).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.py")).unwrap();
        index.write().unwrap();
        fs::write(dir.path().join("a.py"), NEW).unwrap();
        assert_retarget(&run(&dir, &["local", "--unstaged", "--format", format, "--depth", "0"]), format);
    }
    let base = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("base", &base, false).unwrap();
    fs::write(dir.path().join("a.py"), NEW).unwrap();
    commit(&repo);
    let target = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("target", &target, false).unwrap();
    let args = ["branch", "--base", "base", "--target", "target", "--depth", "0"];
    for format in ["mermaid", "dot"] {
        let mut export = args.to_vec();
        export.extend(["--format", format]);
        let clean = run(&dir, &export);
        assert_retarget(&clean, format);
        fs::write(dir.path().join("a.py"), "def invalid(:\n").unwrap();
        assert_eq!(run(&dir, &export).stdout, clean.stdout);
    }
}

#[test]
fn empty_partial_and_default_json_behavior() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.py"), OLD).unwrap();
    commit(&repo);
    let default = run(&dir, &["local"]);
    let json = run(&dir, &["local", "--format", "json"]);
    assert_eq!(default.stdout, json.stdout);
    assert_eq!(default.status.code(), Some(0));
    let tui = run(&dir, &["local", "--format", "tui"]);
    assert_eq!(tui.status.code(), Some(2));
    for (format, header) in [("mermaid", "flowchart LR"), ("dot", "digraph dflow {")] {
        let empty = run(&dir, &["local", "--format", format]);
        assert_eq!(empty.status.code(), Some(0));
        let text = String::from_utf8(empty.stdout).unwrap();
        assert!(text.starts_with(header));
        assert!(text.contains("No symbol or edge changes"));
        fs::write(dir.path().join("a.py"), "def invalid(:\n").unwrap();
        let partial = run(&dir, &["local", "--format", format]);
        assert_eq!(partial.status.code(), Some(1));
        let text = String::from_utf8(partial.stdout).unwrap();
        assert!(text.contains("PARTIAL analysis"));
        assert!(text.ends_with('\n'));
        fs::write(dir.path().join("a.py"), OLD).unwrap();
    }
}
