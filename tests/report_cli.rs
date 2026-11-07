use git2::{Repository, Signature};
use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;
fn commit(repo: &Repository, files: &[&str]) {
    let mut index = repo.index().unwrap();
    for file in files {
        index.add_path(Path::new(file)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("Fixture", "fixture@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<_> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &parents)
        .unwrap();
}
fn run(dir: &TempDir, args: &[&str]) -> (std::process::Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_dflow"))
        .current_dir(dir.path())
        .args(args)
        .output()
        .unwrap();
    let report = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}; stderr={}", String::from_utf8_lossy(&output.stderr)));
    (output, report)
}
#[test]
fn all_local_selections_emit_json_without_mutating_index() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.py"), "def f(): return 1\n").unwrap();
    commit(&repo, &["a.py"]);
    fs::write(dir.path().join("a.py"), "def f(): return 2\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.py")).unwrap();
    index.write().unwrap();
    fs::write(dir.path().join("a.py"), "def f(): return 3\n").unwrap();
    let before = fs::read(repo.path().join("index")).unwrap();
    for (args, selection) in [
        (&["local", "--format", "json"][..], "combined"),
        (&["local", "--staged", "--format", "json"][..], "staged"),
        (&["local", "--unstaged", "--format", "json"][..], "unstaged"),
    ] {
        let (output, report) = run(&dir, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(report["comparison"]["selection"], selection);
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["symbol_changes"][0]["kind"], "modified");
        assert_eq!(output.stdout.iter().filter(|b| **b == b'\n').count(), 1);
        assert!(output.stderr.is_empty());
        let (repeat, _) = run(&dir, args);
        assert_eq!(output.stdout, repeat.stdout);
    }
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.py")).unwrap(),
        "def f(): return 3\n"
    );
}
#[test]
fn branch_tips_ignore_dirty_worktree_and_tui_requires_terminals() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.py"), "def f(): return 1\n").unwrap();
    commit(&repo, &["a.py"]);
    let base = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("base", &base, false).unwrap();
    fs::write(dir.path().join("a.py"), "def f(): return 2\n").unwrap();
    commit(&repo, &["a.py"]);
    let target = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("target", &target, false).unwrap();
    fs::write(dir.path().join("a.py"), "def f(:\n").unwrap();
    let (output, report) = run(&dir, &["branch", "--base", "base", "--target", "target"]);
    assert!(output.status.success());
    assert_eq!(report["comparison"]["mode"], "branch");
    assert_eq!(report["symbol_changes"][0]["kind"], "modified");
    let output = Command::new(env!("CARGO_BIN_EXE_dflow"))
        .current_dir(dir.path())
        .args(["local", "--format", "tui"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
#[test]
fn partial_and_empty_reports_have_correct_status() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.py"), "def f(): pass\n").unwrap();
    commit(&repo, &["a.py"]);
    let (output, report) = run(&dir, &["local"]);
    assert!(output.status.success());
    assert_eq!(report["symbol_changes"].as_array().unwrap().len(), 0);
    fs::write(dir.path().join("a.py"), "def f(:\n").unwrap();
    let (output, report) = run(&dir, &["local"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["complete"], false);
    assert!(output.stderr.is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_dflow"))
        .current_dir(dir.path())
        .args(["branch", "--base", "missing", "--target", "also-missing"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
