use git2::{Oid, Repository, Signature, StatusOptions};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "fixture",
        &tree,
        &parent.iter().collect::<Vec<_>>(),
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
fn report(output: &Output, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["complete"], exit == 0);
    integrity_refs(&value);
    value
}
fn integrity_refs(value: &Value) {
    let nodes: HashSet<_> = value["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .chain(value["module_contexts"].as_array().unwrap())
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    let sites: HashSet<_> = value["call_sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        nodes.len(),
        value["symbols"].as_array().unwrap().len()
            + value["module_contexts"].as_array().unwrap().len()
    );
    for c in value["call_sites"].as_array().unwrap() {
        assert!(nodes.contains(c["caller"].as_str().unwrap()));
        if let Some(target) = c["target"].as_str() {
            assert!(nodes.contains(target));
        }
    }
    for e in value["edges"].as_array().unwrap() {
        assert!(nodes.contains(e["caller"].as_str().unwrap()));
        assert!(nodes.contains(e["callee"].as_str().unwrap()));
        for id in e["call_sites"].as_array().unwrap() {
            assert!(sites.contains(id.as_str().unwrap()));
        }
    }
    for change in value["symbol_changes"].as_array().unwrap() {
        for field in ["before", "after"] {
            if let Some(id) = change[field].as_str() {
                assert!(nodes.contains(id));
            }
        }
    }
    for impact in value["impacts"].as_array().unwrap() {
        assert!(nodes.contains(impact["root"].as_str().unwrap()));
        for n in impact["nodes"].as_array().unwrap() {
            assert!(nodes.contains(n["id"].as_str().unwrap()));
        }
        for edge in impact["edges"].as_array().unwrap() {
            assert!(nodes.contains(edge["caller"].as_str().unwrap()));
            assert!(nodes.contains(edge["callee"].as_str().unwrap()));
        }
    }
}
fn symbol<'a>(report: &'a Value, name: &str, role: &str) -> &'a Value {
    report["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name && s["snapshot"] == role)
        .unwrap()
}
fn rust_symbol<'a>(report: &'a Value, name: &str, role: &str) -> &'a Value {
    report["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name && s["snapshot"] == role && s["language"] == "rust")
        .unwrap()
}
type RepoState = (
    Option<Vec<u8>>,
    Vec<(String, String)>,
    Vec<(String, u32)>,
    Vec<Option<Vec<u8>>>,
);

fn repo_state(repo: &Repository, files: &[&str]) -> RepoState {
    let mut refs: Vec<_> = repo
        .references()
        .unwrap()
        .filter_map(|r| {
            let r = r.ok()?;
            Some((
                r.name()?.to_owned(),
                r.target().map(|id| id.to_string()).unwrap_or_default(),
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
        .map(|e| (e.path().unwrap_or_default().to_owned(), e.status().bits()))
        .collect();
    status.sort();
    (
        fs::read(repo.path().join("index")).ok(),
        refs,
        status,
        files
            .iter()
            .map(|file| fs::read(repo.workdir().unwrap().join(file)).ok())
            .collect(),
    )
}

#[test]
fn local_modes_manifest_and_historical_callers_preserve_repository() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = 'sample'\nversion = '0.1.0'\n",
    );
    write(
        dir.path(),
        "src/lib.rs",
        "mod worker; pub fn rust_root() -> u32 { 1 } pub fn caller() -> u32 { rust_root() }\n",
    );
    write(
        dir.path(),
        "src/worker.rs",
        "pub fn gone() {} pub fn old() { gone(); }\n",
    );
    write(
        dir.path(),
        "same.py",
        "def rust_root(): pass\ndef python_caller(): rust_root()\n",
    );
    commit(
        &repo,
        &["Cargo.toml", "src/lib.rs", "src/worker.rs", "same.py"],
    );
    write(
        dir.path(),
        "src/lib.rs",
        "mod worker; pub fn rust_root() -> u32 { 2 } pub fn caller() -> u32 { rust_root() }\n",
    );
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("src/lib.rs")).unwrap();
    index.write().unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        "mod worker; pub fn rust_root() -> u32 { 3 } pub fn caller() -> u32 { rust_root() }\n",
    );
    write(dir.path(), "src/worker.rs", "pub fn old() {}\n");
    write(dir.path(), "src/new.rs", "pub fn extra() {}\n");
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = 'sample'\nversion = '0.1.1'\n",
    );
    let tracked = [
        "Cargo.toml",
        "src/lib.rs",
        "src/worker.rs",
        "src/new.rs",
        "same.py",
    ];
    let before = repo_state(&repo, &tracked);
    let output = run(dir.path(), &["local", "--format", "json"]);
    let combined = report(&output, 1); // unreferenced new.rs is an uncovered candidate
    assert_eq!(
        run(dir.path(), &["local", "--format", "json"]).stdout,
        output.stdout
    );
    let root = rust_symbol(&combined, "rust_root", "target")["id"]
        .as_str()
        .unwrap();
    let caller = rust_symbol(&combined, "caller", "target")["id"]
        .as_str()
        .unwrap();
    assert!(
        combined["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["caller"] == caller && c["target"] == root)
    );
    assert!(combined["impacts"].as_array().unwrap().iter().any(|i| {
        i["root"] == root
            && i["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == caller)
    }));
    assert!(
        combined["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |c| c["caller"] != symbol(&combined, "python_caller", "target")["id"]
                    || c["target"] != root
            )
    );
    let gone = symbol(&combined, "gone", "base")["id"].as_str().unwrap();
    assert!(
        combined["impacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["root"] == gone
                && i["snapshot"] == "base"
                && i["nodes"].as_array().unwrap().len() > 1)
    );
    let staged = report(
        &run(dir.path(), &["local", "--staged", "--format", "json"]),
        0,
    );
    assert_eq!(staged["comparison"]["selection"], "staged");
    let unstaged = report(
        &run(dir.path(), &["local", "--unstaged", "--format", "json"]),
        1,
    );
    assert_eq!(unstaged["comparison"]["selection"], "unstaged");
    assert_eq!(repo_state(&repo, &tracked), before);
}

#[test]
fn branches_and_malformed_manifest_report_partial_without_mutation() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = 'branch-case'\nversion = '0.1.0'\n",
    );
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn old() {} pub fn entry() { old(); }\n",
    );
    let base = commit(&repo, &["Cargo.toml", "src/lib.rs"]);
    repo.branch("base", &repo.find_commit(base).unwrap(), false)
        .unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn old() { let _n = 1; } pub fn entry() { old(); }\n",
    );
    let target = commit(&repo, &["src/lib.rs"]);
    repo.branch("target", &repo.find_commit(target).unwrap(), false)
        .unwrap();
    let tracked = ["Cargo.toml", "src/lib.rs"];
    let before = repo_state(&repo, &tracked);
    let branch = report(
        &run(
            dir.path(),
            &[
                "branch", "--base", "base", "--target", "target", "--format", "json",
            ],
        ),
        0,
    );
    assert_eq!(branch["comparison"]["mode"], "branch");
    assert_eq!(branch["symbol_changes"].as_array().unwrap().len(), 1);
    write(dir.path(), "Cargo.toml", "[package\n");
    let failed_before = repo_state(&repo, &tracked);
    let partial = report(&run(dir.path(), &["local", "--format", "json"]), 1);
    assert!(
        partial["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["affects_completeness"] == true)
    );
    assert_eq!(repo_state(&repo, &tracked), failed_before);
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = 'branch-case'\nversion = '0.1.0'\n",
    );
    assert_eq!(repo_state(&repo, &tracked), before);
}

#[test]
fn supported_rust_edition_fixtures_extract_without_building_projects() {
    for edition in ["2018", "2021", "2024"] {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            &format!("[package]\nname='edition-case'\nversion='0.1.0'\nedition='{edition}'\n"),
        );
        write(
            dir.path(),
            "src/lib.rs",
            "pub fn target() -> u32 { 1 }\npub fn caller() -> u32 { target() }\n",
        );
        commit(&repo, &["Cargo.toml", "src/lib.rs"]);
        write(
            dir.path(),
            "src/lib.rs",
            "pub fn target() -> u32 { 2 }\npub fn caller() -> u32 { target() }\n",
        );
        let result = report(&run(dir.path(), &["local", "--format", "json"]), 0);
        let callee = symbol(&result, "target", "target")["id"].as_str().unwrap();
        let caller = symbol(&result, "caller", "target")["id"].as_str().unwrap();
        assert!(
            result["call_sites"]
                .as_array()
                .unwrap()
                .iter()
                .any(|site| site["caller"] == caller && site["target"] == callee),
            "{edition}"
        );
    }
}

#[test]
fn grouped_aliases_cfg_imports_and_macros_do_not_invent_edges() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname='certainty'\nversion='0.1.0'\n",
    );
    write(
        dir.path(),
        "src/lib.rs",
        "mod dep;\nuse crate::dep::{ok as renamed};\n#[cfg(feature = \"x\")]\nuse crate::dep::maybe as maybe_alias;\npub fn caller() { renamed(); maybe_alias(); opaque!(dep::ok()); }\n",
    );
    write(
        dir.path(),
        "src/dep.rs",
        "pub fn ok() -> u32 { 1 }\npub fn maybe() {}\n",
    );
    commit(&repo, &["Cargo.toml", "src/lib.rs", "src/dep.rs"]);
    write(
        dir.path(),
        "src/dep.rs",
        "pub fn ok() -> u32 { 2 }\npub fn maybe() {}\n",
    );
    let value = report(&run(dir.path(), &["local", "--format", "json"]), 0);
    let ok = rust_symbol(&value, "ok", "target")["id"].as_str().unwrap();
    let caller = rust_symbol(&value, "caller", "target")["id"]
        .as_str()
        .unwrap();
    let calls: Vec<_> = value["call_sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["snapshot"] == "target" && c["caller"] == caller)
        .collect();
    assert_eq!(calls.len(), 3);
    assert!(
        calls
            .iter()
            .any(|c| c["expression"] == "renamed" && c["target"] == ok)
    );
    assert!(calls.iter().any(|c| c["expression"] == "maybe_alias"
        && c["target"].is_null()
        && c["unresolved_reason"] == "conditional_compilation"));
    assert!(calls.iter().any(|c| c["expression"] == "opaque!"
        && c["target"].is_null()
        && c["unresolved_reason"] == "macro_expansion"));
}

#[test]
fn conditional_and_attributed_types_block_inherent_dispatch() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname='type-availability'\nversion='0.1.0'\n",
    );
    let before = "#[cfg(feature = \"x\")]\nstruct Guard;\nimpl Guard { fn run() {} }\n#[custom]\nstruct Hidden;\nimpl Hidden { fn run() {} }\nfn caller() { Guard::run(); Hidden::run(); }\n";
    write(dir.path(), "src/lib.rs", before);
    commit(&repo, &["Cargo.toml", "src/lib.rs"]);
    write(
        dir.path(),
        "src/lib.rs",
        &before.replace("fn caller()", "pub fn caller()"),
    );
    let value = report(&run(dir.path(), &["local", "--format", "json"]), 0);
    let caller = rust_symbol(&value, "caller", "target")["id"]
        .as_str()
        .unwrap();
    let calls: Vec<_> = value["call_sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["snapshot"] == "target" && call["caller"] == caller)
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().any(|call| call["expression"] == "Guard::run"
        && call["target"].is_null()
        && call["unresolved_reason"] == "conditional_compilation"));
    assert!(calls.iter().any(|call| call["expression"] == "Hidden::run"
        && call["target"].is_null()
        && call["unresolved_reason"] == "unsupported_dispatch"));
}
