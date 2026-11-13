use git2::{Oid, Repository, Signature, StatusOptions};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}
fn commit(repo: &Repository, paths: &[&str]) -> Oid {
    let mut index = repo.index().unwrap();
    for path in paths {
        index.add_path(Path::new(path)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.com").unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<_> = parent.iter().collect();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "fixture",
        &tree,
        &parents,
    )
    .unwrap()
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dflow"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn json(output: &Output, expected_exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 2);
    report
}
fn changes(report: &Value) -> Vec<(String, String)> {
    report["file_changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["path"]["text"].as_str().unwrap().to_owned(),
                change["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
fn symbol_id<'a>(report: &'a Value, name: &str, snapshot: &str) -> &'a str {
    report["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| symbol["name"] == name && symbol["snapshot"] == snapshot)
        .unwrap_or_else(|| panic!("missing {snapshot} symbol {name}"))["id"]
        .as_str()
        .unwrap()
}
fn impact<'a>(report: &'a Value, root: &str) -> &'a Value {
    report["impacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["root"] == root)
        .unwrap()
}
#[derive(Debug, PartialEq, Eq)]
struct Integrity {
    index: Option<Vec<u8>>,
    refs: Vec<(String, String)>,
    status: Vec<(String, u32)>,
    files: Vec<Option<Vec<u8>>>,
}
fn integrity(repo: &Repository, files: &[&str]) -> Integrity {
    let mut refs: Vec<_> = repo
        .references()
        .unwrap()
        .filter_map(|reference| {
            let reference = reference.ok()?;
            Some((
                reference.name()?.to_owned(),
                reference
                    .target()
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            ))
        })
        .collect();
    refs.sort();
    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let mut status: Vec<_> = repo
        .statuses(Some(&mut options))
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry.path().unwrap_or_default().to_owned(),
                entry.status().bits(),
            )
        })
        .collect();
    status.sort();
    Integrity {
        index: fs::read(repo.path().join("index")).ok(),
        refs,
        status,
        files: files
            .iter()
            .map(|path| fs::read(repo.workdir().unwrap().join(path)).ok())
            .collect(),
    }
}

#[test]
fn binary_local_matrix_keeps_snapshot_selections_and_repository_state() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), ".gitignore", "ignored.py\n");
    write(dir.path(), "a.py", "def f(): return 1\n");
    write(dir.path(), "caller.py", "from a import f\ndef use(): f()\n");
    commit(&repo, &[".gitignore", "a.py", "caller.py"]);
    write(dir.path(), "a.py", "def f(): return 2\n");
    write(dir.path(), "staged.py", "def staged(): pass\n");
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.py")).unwrap();
    index.add_path(Path::new("staged.py")).unwrap();
    index.write().unwrap();
    write(dir.path(), "a.py", "def f(): return 3\n");
    fs::remove_file(dir.path().join("staged.py")).unwrap();
    write(dir.path(), "new.py", "def fresh(): pass\n");
    write(dir.path(), "ignored.py", "def ignored(): pass\n");
    let before = integrity(
        &repo,
        &["a.py", "caller.py", "staged.py", "new.py", "ignored.py"],
    );

    let combined_output = run(dir.path(), &["local", "--format", "json", "--depth", "2"]);
    let combined = json(&combined_output, 0);
    assert_eq!(combined["comparison"]["selection"], "combined");
    assert_eq!(
        changes(&combined),
        vec![
            ("a.py".into(), "modified".into()),
            ("new.py".into(), "added".into())
        ]
    );
    let target_f = symbol_id(&combined, "f", "target");
    let target_use = symbol_id(&combined, "use", "target");
    assert!(
        impact(&combined, target_f)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == target_use && n["depth"] == 1)
    );
    assert_eq!(
        run(dir.path(), &["local", "--format", "json", "--depth", "2"]).stdout,
        combined_output.stdout
    );

    let staged = json(
        &run(dir.path(), &["local", "--staged", "--format", "json"]),
        0,
    );
    assert_eq!(staged["comparison"]["selection"], "staged");
    assert_eq!(
        changes(&staged),
        vec![
            ("a.py".into(), "modified".into()),
            ("staged.py".into(), "added".into())
        ]
    );
    let unstaged = json(
        &run(dir.path(), &["local", "--unstaged", "--format", "json"]),
        0,
    );
    assert_eq!(unstaged["comparison"]["selection"], "unstaged");
    assert_eq!(
        changes(&unstaged),
        vec![
            ("a.py".into(), "modified".into()),
            ("new.py".into(), "added".into()),
            ("staged.py".into(), "deleted".into())
        ]
    );
    for report in [&combined, &staged, &unstaged] {
        assert!(
            !report["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "ignored")
        );
    }
    assert_eq!(
        integrity(
            &repo,
            &["a.py", "caller.py", "staged.py", "new.py", "ignored.py"]
        ),
        before
    );
}

#[test]
fn binary_unborn_head_and_conflicts_have_distinct_results() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "new.py", "def new(): pass\n");
    let before = integrity(&repo, &["new.py"]);
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    assert_eq!(report["snapshots"]["base"]["kind"], "empty");
    assert_eq!(changes(&report), vec![("new.py".into(), "added".into())]);
    assert_eq!(integrity(&repo, &["new.py"]), before);

    commit(&repo, &["new.py"]);
    let mut index = repo.index().unwrap();
    let mut ours = index.get_path(Path::new("new.py"), 0).unwrap();
    let mut theirs = index.get_path(Path::new("new.py"), 0).unwrap();
    index.remove_path(Path::new("new.py")).unwrap();
    ours.flags = (ours.flags & !0x3000) | 0x2000;
    theirs.flags = (theirs.flags & !0x3000) | 0x3000;
    index.add(&ours).unwrap();
    index.add(&theirs).unwrap();
    index.write().unwrap();
    let before = integrity(&repo, &["new.py"]);
    let output = run(dir.path(), &["local", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved conflicts"));
    assert_eq!(integrity(&repo, &["new.py"]), before);
}

#[test]
fn binary_branch_mode_uses_diverged_tips_even_with_dirty_files() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "a.py", "def f(): return 1\n");
    let ancestor = commit(&repo, &["a.py"]);
    write(dir.path(), "a.py", "def f(): return 2\n");
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.py")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.com").unwrap();
    let parent = repo.find_commit(ancestor).unwrap();
    let feature = repo
        .commit(
            Some("refs/heads/feature"),
            &signature,
            &signature,
            "feature",
            &tree,
            &[&parent],
        )
        .unwrap();
    write(dir.path(), "a.py", "def f(): return 3\n");
    let main = commit(&repo, &["a.py"]);
    let main_name = repo.head().unwrap().shorthand().unwrap().to_owned();
    write(dir.path(), "a.py", "def f(:\n");
    let before = integrity(&repo, &["a.py"]);
    let args = [
        "branch",
        "--base",
        main_name.as_str(),
        "--target",
        "feature",
        "--format",
        "json",
    ];
    let output = run(dir.path(), &args);
    let report = json(&output, 0);
    assert_eq!(report["snapshots"]["base"]["commit_id"], main.to_string());
    assert_eq!(
        report["snapshots"]["target"]["commit_id"],
        feature.to_string()
    );
    assert_ne!(main, ancestor);
    assert_eq!(changes(&report), vec![("a.py".into(), "modified".into())]);
    assert_eq!(report["complete"], true);
    assert_eq!(run(dir.path(), &args).stdout, output.stdout);
    assert_eq!(integrity(&repo, &["a.py"]), before);
}

#[test]
fn property_getter_read_from_imported_instance_has_caller_edge() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "service.py", "class Service:\n    pass\n");
    write(
        dir.path(),
        "entry.py",
        "from service import Service\n\ndef entry():\n    obj = Service()\n    return obj.value\n\ndef write_only():\n    obj = Service()\n    obj.value = 1\n\ndef rebound():\n    obj = Service()\n    obj = object()\n    return obj.value\n",
    );
    commit(&repo, &["service.py", "entry.py"]);
    write(
        dir.path(),
        "service.py",
        "class Service:\n    @property\n    def value(self):\n        return 42\n",
    );
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    let getter = symbol_id(&report, "value", "target");
    let caller = symbol_id(&report, "entry", "target");
    assert!(
        impact(&report, getter)["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| { edge["caller"] == caller && edge["callee"] == getter }),
        "{:#}",
        impact(&report, getter)
    );
    let nodes = impact(&report, getter)["nodes"].as_array().unwrap();
    for name in ["write_only", "rebound"] {
        let id = symbol_id(&report, name, "target");
        assert!(
            !nodes.iter().any(|node| node["id"] == id),
            "{name} is not a getter caller"
        );
    }
}

#[test]
fn annotated_method_receivers_trace_imported_and_local_callers() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "app/model.py",
        "from typing import Optional\nclass Rate:\n    pass\nclass Transaction:\n    def apply(self, rate: Optional[\"Rate\"]):\n        return rate.sell(1)\n\ndef local(rate: Rate):\n    return rate.sell(1)\n",
    );
    write(
        dir.path(),
        "app/entry.py",
        "from typing import Optional\nfrom model import Rate\n\ndef direct(rate: Rate):\n    return rate.sell(1)\n\ndef optional(rate: Optional[Rate]):\n    if rate is None:\n        return None\n    return sum(rate.sell(n) for n in [1, 2])\n\ndef unknown(rate):\n    return rate.sell(1)\n\ndef rebound(rate: Rate):\n    rate = object()\n    return rate.sell(1)\n",
    );
    write(
        dir.path(),
        "app/pyproject.toml",
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    );
    commit(
        &repo,
        &["app/pyproject.toml", "app/model.py", "app/entry.py"],
    );
    write(
        dir.path(),
        "app/model.py",
        "from typing import Optional\nclass Rate:\n    def sell(self, amount):\n        return amount\nclass Transaction:\n    def apply(self, rate: Optional[\"Rate\"]):\n        return rate.sell(1)\n\ndef local(rate: Rate):\n    return rate.sell(1)\n",
    );
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    let getter = symbol_id(&report, "sell", "target");
    let nodes = impact(&report, getter)["nodes"].as_array().unwrap();
    for name in ["local", "direct", "optional", "apply"] {
        let id = symbol_id(&report, name, "target");
        assert!(
            nodes
                .iter()
                .any(|node| node["id"] == id && node["depth"] == 1),
            "{name} missing"
        );
    }
    for name in ["unknown", "rebound"] {
        let id = symbol_id(&report, name, "target");
        assert!(
            !nodes.iter().any(|node| node["id"] == id),
            "{name} must remain unresolved"
        );
    }
}

#[test]
fn binary_reports_modified_added_deleted_and_unchanged_callers() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "service.py",
        "def modified(): return 1\ndef removed(): pass\ndef formatting(): return 1 # before\ndef legacy(): removed()\n",
    );
    write(
        dir.path(),
        "entry.py",
        "from service import modified as dispatch\ndef entry(): dispatch()\n",
    );
    commit(&repo, &["service.py", "entry.py"]);
    write(
        dir.path(),
        "service.py",
        "def modified(): return 2\ndef added(): pass\ndef formatting(): return 1 # after\ndef legacy(): added()\n",
    );
    let before = integrity(&repo, &["service.py", "entry.py"]);
    let report = json(
        &run(dir.path(), &["local", "--format", "json", "--depth", "2"]),
        0,
    );
    let modified = symbol_id(&report, "modified", "target");
    let added = symbol_id(&report, "added", "target");
    let removed = symbol_id(&report, "removed", "base");
    let entry = symbol_id(&report, "entry", "target");
    let legacy_target = symbol_id(&report, "legacy", "target");
    let legacy_base = symbol_id(&report, "legacy", "base");
    assert!(
        impact(&report, modified)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == entry && node["depth"] == 1)
    );
    assert!(
        impact(&report, added)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == legacy_target)
    );
    assert_eq!(impact(&report, removed)["snapshot"], "base");
    assert!(
        impact(&report, removed)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == legacy_base)
    );
    let changed_ids: Vec<_> = report["symbol_changes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|change| [change["before"].as_str(), change["after"].as_str()])
        .flatten()
        .collect();
    assert!(!changed_ids.contains(&symbol_id(&report, "formatting", "base")));
    assert!(!changed_ids.contains(&symbol_id(&report, "formatting", "target")));
    assert_eq!(integrity(&repo, &["service.py", "entry.py"]), before);
}

#[test]
fn binary_keeps_ambiguous_calls_unresolved_and_cycles_bounded() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "graph.py", "def root(): return 1\n");
    commit(&repo, &["graph.py"]);
    write(
        dir.path(),
        "graph.py",
        "def root(): return 2\ndef a(): b(); root()\ndef b(): a()\ndef uncertain(root): root(); getattr(root, 'x')()\n",
    );
    let before = integrity(&repo, &["graph.py"]);
    let report = json(
        &run(dir.path(), &["local", "--format", "json", "--depth", "5"]),
        0,
    );
    let root = symbol_id(&report, "root", "target");
    let a = symbol_id(&report, "a", "target");
    let b = symbol_id(&report, "b", "target");
    let nodes = impact(&report, root)["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3);
    assert!(
        nodes
            .iter()
            .any(|node| node["id"] == a && node["depth"] == 1)
    );
    assert!(
        nodes
            .iter()
            .any(|node| node["id"] == b && node["depth"] == 2)
    );
    let edges = impact(&report, root)["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| edge["caller"] == a && edge["callee"] == b)
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge["caller"] == b && edge["callee"] == a)
    );
    let uncertain: Vec<_> = report["call_sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|site| site["path"]["text"] == "graph.py" && site["target"].is_null())
        .collect();
    assert!(
        uncertain
            .iter()
            .any(|site| site["unresolved_reason"] == "shadowed")
    );
    assert!(
        uncertain
            .iter()
            .any(|site| site["unresolved_reason"] == "dynamic_target")
    );
    assert!(
        report["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["caller"] != symbol_id(&report, "uncertain", "target"))
    );
    assert_eq!(integrity(&repo, &["graph.py"]), before);
}

#[test]
fn binary_classifies_signature_literal_and_nested_changes_but_ignores_comments() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "a.py",
        "def outer(x=1):\n    def inner(): return 'a'\n    return inner()\n",
    );
    commit(&repo, &["a.py"]);
    write(
        dir.path(),
        "a.py",
        "# changed comment\ndef outer(x=1):\n    def inner(): return 'a'\n    return inner()\n",
    );
    let before = integrity(&repo, &["a.py"]);
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    assert!(report["symbol_changes"].as_array().unwrap().is_empty());
    assert_eq!(integrity(&repo, &["a.py"]), before);

    write(
        dir.path(),
        "a.py",
        "def outer(x=2):\n    def inner(): return 'a'\n    return inner()\n",
    );
    let before = integrity(&repo, &["a.py"]);
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    let outer = symbol_id(&report, "outer", "target");
    assert!(
        report["symbol_changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["kind"] == "modified" && change["after"] == outer)
    );
    assert_eq!(integrity(&repo, &["a.py"]), before);

    write(
        dir.path(),
        "a.py",
        "def outer(x=1):\n    def inner(): return 'b'\n    return inner()\n",
    );
    let before = integrity(&repo, &["a.py"]);
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    let inner = symbol_id(&report, "inner", "target");
    assert!(
        report["symbol_changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["kind"] == "modified" && change["after"] == inner)
    );
    assert_eq!(integrity(&repo, &["a.py"]), before);
}

#[test]
fn binary_rename_without_matching_is_added_and_deleted() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "old.py", "def stable(): pass\n");
    commit(&repo, &["old.py"]);
    fs::rename(dir.path().join("old.py"), dir.path().join("new.py")).unwrap();
    let mut index = repo.index().unwrap();
    index.remove_path(Path::new("old.py")).unwrap();
    index.write().unwrap();
    let before = integrity(&repo, &["old.py", "new.py"]);
    let report = json(&run(dir.path(), &["local", "--format", "json"]), 0);
    assert_eq!(
        changes(&report),
        vec![
            ("new.py".into(), "added".into()),
            ("old.py".into(), "deleted".into())
        ]
    );
    let kinds: Vec<_> = report["symbol_changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"added") && kinds.contains(&"deleted"));
    let staged = json(
        &run(dir.path(), &["local", "--staged", "--format", "json"]),
        0,
    );
    assert_eq!(changes(&staged), vec![("old.py".into(), "deleted".into())]);
    assert_eq!(integrity(&repo, &["old.py", "new.py"]), before);
}

#[test]
fn binary_empty_partial_and_invalid_runs_keep_repository_untouched() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(dir.path(), "a.py", "def f(): pass\n");
    commit(&repo, &["a.py"]);
    let before = integrity(&repo, &["a.py"]);
    let empty_output = run(dir.path(), &["local", "--format", "json"]);
    let empty = json(&empty_output, 0);
    assert!(empty["symbol_changes"].as_array().unwrap().is_empty());
    assert!(!empty_output.stdout.contains(&0x1b));
    assert_eq!(integrity(&repo, &["a.py"]), before);

    write(dir.path(), "a.py", "def f(:\n");
    let before = integrity(&repo, &["a.py"]);
    let partial = json(&run(dir.path(), &["local", "--format", "json"]), 1);
    assert_eq!(partial["complete"], false);
    assert!(
        partial["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["affects_completeness"] == true)
    );
    for args in [
        vec!["branch", "--base", "missing", "--target", "also-missing"],
        vec!["local", "--depth", "-1"],
        vec!["local", "--format", "tui"],
    ] {
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    assert_eq!(integrity(&repo, &["a.py"]), before);
}
