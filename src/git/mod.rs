use crate::cli::CompareMode;
use git2::{ErrorCode, ObjectType, Oid, Repository};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SnapshotKind {
    Commit,
    Index,
    WorkingTree,
    Empty,
}
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SnapshotRole {
    Base,
    Target,
}
#[derive(Debug, Clone)]
pub struct CapturedFile {
    pub path: Vec<u8>,
    pub bytes: Vec<u8>,
    pub mode: u32,
}
#[derive(Debug, Clone)]
#[allow(dead_code)] // Consumed by analysis/report milestones after M1.
pub struct CapturedSnapshot {
    pub kind: SnapshotKind,
    pub role: SnapshotRole,
    pub commit_id: Option<Oid>,
    pub branch: Option<String>,
    pub files: BTreeMap<Vec<u8>, CapturedFile>,
    pub content_id: String,
}
#[derive(Debug)]
pub struct SnapshotPair {
    pub base: CapturedSnapshot,
    pub target: CapturedSnapshot,
}
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}
#[derive(Debug)]
#[allow(dead_code)] // Consumed by analysis/report milestones after M1.
pub struct FileChange {
    pub path: Vec<u8>,
    pub kind: ChangeKind,
}
#[derive(Debug)]
#[allow(dead_code)] // Consumed by analysis/report milestones after M1.
pub struct Diagnostic {
    pub path: Vec<u8>,
    pub message: String,
}
#[derive(Debug)]
pub struct CaptureResult {
    pub pair: SnapshotPair,
    pub changes: Vec<FileChange>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Debug)]
pub enum CaptureError {
    Input(String),
    Analysis(String),
}
impl CaptureError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Input(_) => 2,
            Self::Analysis(_) => 1,
        }
    }
}
impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(s) | Self::Analysis(s) => f.write_str(s),
        }
    }
}
type Result<T> = std::result::Result<T, CaptureError>;
fn analysis(e: impl fmt::Display) -> CaptureError {
    CaptureError::Analysis(e.to_string())
}
fn input(e: impl fmt::Display) -> CaptureError {
    CaptureError::Input(e.to_string())
}
fn relevant(path: &[u8]) -> bool {
    path.ends_with(b".py") || path == b"pyproject.toml" || path.ends_with(b"/pyproject.toml")
}
fn hash_files(files: &BTreeMap<Vec<u8>, CapturedFile>) -> String {
    let mut h = Sha256::new();
    for file in files.values() {
        for part in [
            &file.path[..],
            &file.mode.to_be_bytes()[..],
            &file.bytes[..],
        ] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part);
        }
    }
    format!("{:x}", h.finalize())
}
fn snapshot(
    kind: SnapshotKind,
    role: SnapshotRole,
    commit_id: Option<Oid>,
    branch: Option<String>,
    files: BTreeMap<Vec<u8>, CapturedFile>,
) -> CapturedSnapshot {
    let content_id = hash_files(&files);
    CapturedSnapshot {
        kind,
        role,
        commit_id,
        branch,
        files,
        content_id,
    }
}
fn insert_blob(
    repo: &Repository,
    files: &mut BTreeMap<Vec<u8>, CapturedFile>,
    path: Vec<u8>,
    oid: Oid,
    mode: u32,
) -> Result<()> {
    if relevant(&path) {
        let blob = repo.find_blob(oid).map_err(analysis)?;
        files.insert(
            path.clone(),
            CapturedFile {
                path,
                bytes: blob.content().to_vec(),
                mode,
            },
        );
    }
    Ok(())
}
fn walk_tree(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &[u8],
    files: &mut BTreeMap<Vec<u8>, CapturedFile>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    for entry in tree.iter() {
        let mut path = prefix.to_vec();
        path.extend_from_slice(entry.name_bytes());
        match entry.kind() {
            Some(ObjectType::Tree) => {
                path.push(b'/');
                let child = repo.find_tree(entry.id()).map_err(analysis)?;
                walk_tree(repo, &child, &path, files, diagnostics)?;
            }
            Some(ObjectType::Blob) if entry.filemode() == 0o120000 => {
                if relevant(&path) {
                    diagnostics.push(Diagnostic {
                        path,
                        message: "symlink source skipped".into(),
                    });
                }
            }
            Some(ObjectType::Blob) => {
                insert_blob(repo, files, path, entry.id(), entry.filemode() as u32)?
            }
            Some(ObjectType::Commit) => diagnostics.push(Diagnostic {
                path,
                message: "submodule skipped".into(),
            }),
            _ => {}
        }
    }
    Ok(())
}
fn collect_gitlinks(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &[u8],
    gitlinks: &mut BTreeSet<Vec<u8>>,
) -> Result<()> {
    for entry in tree.iter() {
        let mut path = prefix.to_vec();
        path.extend_from_slice(entry.name_bytes());
        match entry.kind() {
            Some(ObjectType::Commit) => {
                gitlinks.insert(path);
            }
            Some(ObjectType::Tree) => {
                path.push(b'/');
                let child = repo.find_tree(entry.id()).map_err(analysis)?;
                collect_gitlinks(repo, &child, &path, gitlinks)?;
            }
            _ => {}
        }
    }
    Ok(())
}
fn commit_snapshot(
    repo: &Repository,
    oid: Oid,
    role: SnapshotRole,
    branch: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<CapturedSnapshot> {
    let commit = repo.find_commit(oid).map_err(analysis)?;
    let tree = commit.tree().map_err(analysis)?;
    let mut files = BTreeMap::new();
    walk_tree(repo, &tree, b"", &mut files, diagnostics)?;
    Ok(snapshot(
        SnapshotKind::Commit,
        role,
        Some(oid),
        branch,
        files,
    ))
}
fn head_snapshot(repo: &Repository, diagnostics: &mut Vec<Diagnostic>) -> Result<CapturedSnapshot> {
    match repo.head() {
        Ok(head) => {
            let oid = head.peel_to_commit().map_err(analysis)?.id();
            commit_snapshot(repo, oid, SnapshotRole::Base, None, diagnostics)
        }
        Err(e) if e.code() == ErrorCode::UnbornBranch => Ok(snapshot(
            SnapshotKind::Empty,
            SnapshotRole::Base,
            None,
            None,
            BTreeMap::new(),
        )),
        Err(e) => Err(input(e)),
    }
}
fn index_snapshot(
    repo: &Repository,
    role: SnapshotRole,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<CapturedSnapshot> {
    let index = repo.index().map_err(analysis)?;
    if index.has_conflicts() {
        return Err(input("index has unresolved conflicts"));
    }
    let mut files = BTreeMap::new();
    for entry in index.iter() {
        if (entry.flags >> 12) & 3 != 0 {
            return Err(input("index has unresolved conflicts"));
        }
        if entry.mode == 0o120000 {
            if relevant(&entry.path) {
                diagnostics.push(Diagnostic {
                    path: entry.path,
                    message: "symlink source skipped".into(),
                });
            }
            continue;
        }
        if entry.mode == 0o160000 {
            diagnostics.push(Diagnostic {
                path: entry.path,
                message: "submodule skipped".into(),
            });
            continue;
        }
        insert_blob(repo, &mut files, entry.path, entry.id, entry.mode)?;
    }
    Ok(snapshot(SnapshotKind::Index, role, None, None, files))
}
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}
fn has_symlink_ancestor(root: &Path, relative: &Path) -> Result<bool> {
    let mut parent = root.to_path_buf();
    if let Some(ancestors) = relative.parent() {
        for component in ancestors.components() {
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(e) => {
                    return Err(analysis(format!(
                        "cannot inspect {}: {e}",
                        parent.display()
                    )));
                }
            }
        }
    }
    Ok(false)
}
fn discover_untracked(
    repo: &Repository,
    root: &Path,
    dir: &Path,
    tracked: &BTreeSet<Vec<u8>>,
    gitlinks: &BTreeSet<Vec<u8>>,
    candidates: &mut BTreeSet<Vec<u8>>,
) -> Result<()> {
    for item in fs::read_dir(dir).map_err(analysis)? {
        let item = item.map_err(analysis)?;
        let path = item.path();
        let relative = path.strip_prefix(root).map_err(analysis)?;
        if relative == Path::new(".git") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(analysis)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let raw = path_bytes(relative);
        if metadata.is_dir() {
            if gitlinks.contains(&raw) {
                continue;
            }
            if !tracked
                .iter()
                .any(|p| p.starts_with(&[raw.as_slice(), b"/"].concat()))
                && repo.is_path_ignored(relative).map_err(analysis)?
            {
                continue;
            }
            discover_untracked(repo, root, &path, tracked, gitlinks, candidates)?;
        } else if metadata.is_file()
            && relevant(&raw)
            && !tracked.contains(&raw)
            && !repo.is_path_ignored(relative).map_err(analysis)?
        {
            candidates.insert(raw);
        }
    }
    Ok(())
}
fn worktree_snapshot(
    repo: &Repository,
    role: SnapshotRole,
    tracked: &BTreeSet<Vec<u8>>,
    gitlinks: &BTreeSet<Vec<u8>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<CapturedSnapshot> {
    let root = repo
        .workdir()
        .ok_or_else(|| input("local mode requires a working tree"))?;
    let mut candidates: BTreeSet<_> = tracked.difference(gitlinks).cloned().collect();
    discover_untracked(repo, root, root, tracked, gitlinks, &mut candidates)?;
    let mut files = BTreeMap::new();
    for path in candidates {
        if !relevant(&path) {
            continue;
        }
        let relative = path_from_bytes(&path);
        if has_symlink_ancestor(root, &relative)? {
            diagnostics.push(Diagnostic {
                path,
                message: "symlink source skipped".into(),
            });
            continue;
        }
        let full = root.join(relative);
        match fs::symlink_metadata(&full) {
            Ok(m) if m.file_type().is_symlink() => diagnostics.push(Diagnostic {
                path,
                message: "symlink source skipped".into(),
            }),
            Ok(m) if m.is_file() => {
                let bytes = fs::read(&full)
                    .map_err(|e| analysis(format!("cannot read {}: {e}", full.display())))?;
                #[cfg(unix)]
                let mode = if std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0
                {
                    0o100755
                } else {
                    0o100644
                };
                #[cfg(not(unix))]
                let mode = 0o100644;
                files.insert(path.clone(), CapturedFile { path, bytes, mode });
            }
            Ok(_) => {
                return Err(analysis(format!(
                    "source path is not a regular file: {}",
                    full.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !tracked.contains(&path) {
                    return Err(analysis(format!(
                        "source disappeared while capturing: {}",
                        full.display()
                    )));
                }
                // The tracked source was deleted from the worktree.
            }
            Err(e) => return Err(analysis(format!("cannot inspect {}: {e}", full.display()))),
        }
    }
    Ok(snapshot(SnapshotKind::WorkingTree, role, None, None, files))
}
fn branch_oid(repo: &Repository, name: &str) -> Result<(Oid, String)> {
    if name.is_empty() || name.starts_with('-') || name.contains("..") || name.contains('@') {
        return Err(input("invalid branch name"));
    }
    let candidates: Vec<String> =
        if name.starts_with("refs/heads/") || name.starts_with("refs/remotes/") {
            vec![name.into()]
        } else {
            vec![format!("refs/heads/{name}"), format!("refs/remotes/{name}")]
        };
    let mut found = Vec::new();
    for candidate in candidates {
        if !git2::Reference::is_valid_name(&candidate) {
            return Err(input(format!("invalid branch name: {name}")));
        }
        match repo.find_reference(&candidate) {
            Ok(reference) => {
                let oid = reference
                    .peel_to_commit()
                    .map_err(|_| input(format!("branch does not point to a commit: {candidate}")))?
                    .id();
                found.push((oid, candidate));
            }
            Err(e) if e.code() == ErrorCode::NotFound => {}
            Err(e) => return Err(input(e)),
        }
    }
    match found.len() {
        0 => Err(input(format!("branch not found: {name}"))),
        1 => Ok(found.remove(0)),
        _ => Err(input(format!("ambiguous branch name: {name}"))),
    }
}
fn changes(base: &CapturedSnapshot, target: &CapturedSnapshot) -> Vec<FileChange> {
    let paths: BTreeSet<_> = base
        .files
        .keys()
        .chain(target.files.keys())
        .cloned()
        .collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let kind = match (base.files.get(&path), target.files.get(&path)) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                (Some(a), Some(b)) if a.bytes != b.bytes || a.mode != b.mode => {
                    ChangeKind::Modified
                }
                _ => return None,
            };
            Some(FileChange { path, kind })
        })
        .collect()
}
pub fn capture_comparison(mode: &CompareMode, cwd: PathBuf) -> Result<CaptureResult> {
    let repo = Repository::discover(cwd).map_err(input)?;
    let mut diagnostics = Vec::new();
    let pair = match mode {
        CompareMode::Branch(args) => {
            let (base_oid, base_name) = branch_oid(&repo, &args.base)?;
            let (target_oid, target_name) = branch_oid(&repo, &args.target)?;
            SnapshotPair {
                base: commit_snapshot(
                    &repo,
                    base_oid,
                    SnapshotRole::Base,
                    Some(base_name),
                    &mut diagnostics,
                )?,
                target: commit_snapshot(
                    &repo,
                    target_oid,
                    SnapshotRole::Target,
                    Some(target_name),
                    &mut diagnostics,
                )?,
            }
        }
        CompareMode::Local(args) => {
            if repo.is_bare() {
                return Err(input("local mode requires a working tree"));
            }
            // Detect conflicts regardless of local comparison selection.
            let index = repo.index().map_err(analysis)?;
            if index.has_conflicts() {
                return Err(input("index has unresolved conflicts"));
            }
            let base = if args.unstaged {
                index_snapshot(&repo, SnapshotRole::Base, &mut diagnostics)?
            } else {
                head_snapshot(&repo, &mut diagnostics)?
            };
            let target = if args.staged {
                index_snapshot(&repo, SnapshotRole::Target, &mut diagnostics)?
            } else {
                let mut gitlinks: BTreeSet<_> = index
                    .iter()
                    .filter(|entry| entry.mode == 0o160000)
                    .map(|entry| entry.path)
                    .collect();
                match repo.head() {
                    Ok(head) => {
                        let tree = head
                            .peel_to_commit()
                            .map_err(analysis)?
                            .tree()
                            .map_err(analysis)?;
                        collect_gitlinks(&repo, &tree, b"", &mut gitlinks)?;
                    }
                    Err(e) if e.code() == ErrorCode::UnbornBranch => {}
                    Err(e) => return Err(input(e)),
                }
                let tracked: BTreeSet<_> = base
                    .files
                    .keys()
                    .cloned()
                    .chain(index.iter().map(|e| e.path))
                    .collect();
                worktree_snapshot(
                    &repo,
                    SnapshotRole::Target,
                    &tracked,
                    &gitlinks,
                    &mut diagnostics,
                )?
            };
            SnapshotPair { base, target }
        }
    };
    let changes = changes(&pair.base, &pair.target);
    Ok(CaptureResult {
        pair,
        changes,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{AnalyzeOptions, BranchArgs, LocalArgs};
    use git2::Signature;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        (dir, repo)
    }
    fn commit(repo: &Repository, name: &str) -> Oid {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let parents = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect::<Vec<_>>();
        let refs = parents.iter().collect::<Vec<_>>();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &refs)
            .unwrap()
    }
    fn local(staged: bool, unstaged: bool) -> CompareMode {
        CompareMode::Local(LocalArgs {
            staged,
            unstaged,
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        })
    }
    fn capture(mode: &CompareMode, dir: &TempDir) -> CaptureResult {
        capture_comparison(mode, dir.path().to_path_buf()).unwrap()
    }
    fn body<'a>(snapshot: &'a CapturedSnapshot, path: &[u8]) -> Option<&'a [u8]> {
        snapshot.files.get(path).map(|f| f.bytes.as_slice())
    }
    #[test]
    fn local_pairs_are_independent() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"initial").unwrap();
        commit(&repo, "a.py");
        fs::write(dir.path().join("a.py"), b"staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.py")).unwrap();
        index.write().unwrap();
        fs::write(dir.path().join("a.py"), b"initial").unwrap();
        let combined = capture(&local(false, false), &dir);
        let staged = capture(&local(true, false), &dir);
        let unstaged = capture(&local(false, true), &dir);
        assert!(combined.changes.is_empty());
        assert_eq!(body(&staged.pair.target, b"a.py"), Some(&b"staged"[..]));
        assert_eq!(body(&unstaged.pair.base, b"a.py"), Some(&b"staged"[..]));
        assert_eq!(body(&unstaged.pair.target, b"a.py"), Some(&b"initial"[..]));
        assert_eq!(staged.changes[0].kind, ChangeKind::Modified);
        assert_eq!(unstaged.changes[0].kind, ChangeKind::Modified);
        assert_eq!(
            combined.pair.base.content_id,
            combined.pair.target.content_id
        );
    }
    #[test]
    fn untracked_and_ignored_sources() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join(".gitignore"), "ignored.py\n").unwrap();
        fs::write(dir.path().join("tracked.py"), b"x").unwrap();
        commit(&repo, "tracked.py");
        fs::write(dir.path().join("new.py"), b"new").unwrap();
        fs::write(dir.path().join("ignored.py"), b"ignored").unwrap();
        let combined = capture(&local(false, false), &dir);
        let staged = capture(&local(true, false), &dir);
        let unstaged = capture(&local(false, true), &dir);
        assert_eq!(combined.changes.len(), 1);
        assert_eq!(combined.changes[0].path, b"new.py");
        assert_eq!(combined.changes[0].kind, ChangeKind::Added);
        assert!(staged.changes.is_empty());
        assert_eq!(unstaged.changes.len(), 1);
        assert!(
            !combined
                .pair
                .target
                .files
                .contains_key(b"ignored.py".as_slice())
        );
    }
    #[test]
    fn branch_tips_ignore_dirty_worktree() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        let base = commit(&repo, "a.py");
        repo.branch("base", &repo.find_commit(base).unwrap(), false)
            .unwrap();
        fs::write(dir.path().join("a.py"), b"target").unwrap();
        let target = commit(&repo, "a.py");
        repo.branch("target", &repo.find_commit(target).unwrap(), false)
            .unwrap();
        fs::write(dir.path().join("a.py"), b"dirty").unwrap();
        let mode = CompareMode::Branch(BranchArgs {
            base: "base".into(),
            target: "target".into(),
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        });
        let result = capture(&mode, &dir);
        assert_eq!(body(&result.pair.base, b"a.py"), Some(&b"base"[..]));
        assert_eq!(body(&result.pair.target, b"a.py"), Some(&b"target"[..]));
        assert_eq!(result.pair.base.commit_id, Some(base));
        assert_eq!(result.pair.target.commit_id, Some(target));
        assert_eq!(fs::read(dir.path().join("a.py")).unwrap(), b"dirty");
    }
    #[test]
    fn unborn_head_uses_empty_snapshot() {
        let (dir, _) = fixture();
        fs::write(dir.path().join("new.py"), b"new").unwrap();
        let result = capture(&local(false, false), &dir);
        assert_eq!(result.pair.base.kind, SnapshotKind::Empty);
        assert_eq!(result.changes.len(), 1);
    }
    #[test]
    fn staged_addition_and_worktree_deletion() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("seed.py"), b"seed").unwrap();
        commit(&repo, "seed.py");
        fs::write(dir.path().join("new.py"), b"staged new").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("new.py")).unwrap();
        index.write().unwrap();
        fs::remove_file(dir.path().join("new.py")).unwrap();
        fs::remove_file(dir.path().join("seed.py")).unwrap();
        let staged = capture(&local(true, false), &dir);
        let unstaged = capture(&local(false, true), &dir);
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].kind, ChangeKind::Added);
        assert_eq!(unstaged.changes.len(), 2);
        assert!(
            unstaged
                .changes
                .iter()
                .all(|c| c.kind == ChangeKind::Deleted)
        );
        assert_eq!(
            body(&unstaged.pair.base, b"new.py"),
            Some(&b"staged new"[..])
        );
    }
    #[test]
    fn invalid_branch_and_bare_local_are_input_errors() {
        let (dir, _) = fixture();
        let mode = CompareMode::Branch(BranchArgs {
            base: "missing".into(),
            target: "also-missing".into(),
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        });
        assert_eq!(
            capture_comparison(&mode, dir.path().to_path_buf())
                .unwrap_err()
                .exit_code(),
            2
        );
        let bare = TempDir::new().unwrap();
        Repository::init_bare(bare.path()).unwrap();
        assert_eq!(
            capture_comparison(&local(false, false), bare.path().to_path_buf())
                .unwrap_err()
                .exit_code(),
            2
        );
    }
    #[test]
    fn index_conflict_is_rejected_without_mutation() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        commit(&repo, "a.py");
        let mut index = repo.index().unwrap();
        let mut ours = index.get_path(Path::new("a.py"), 0).unwrap();
        let mut theirs = index.get_path(Path::new("a.py"), 0).unwrap();
        index.remove_path(Path::new("a.py")).unwrap();
        ours.flags = (ours.flags & !0x3000) | 0x2000;
        theirs.flags = (theirs.flags & !0x3000) | 0x3000;
        index.add(&ours).unwrap();
        index.add(&theirs).unwrap();
        index.write().unwrap();
        assert_eq!(
            capture_comparison(&local(false, false), dir.path().to_path_buf())
                .unwrap_err()
                .exit_code(),
            2
        );
        assert!(repo.index().unwrap().has_conflicts());
    }
    #[test]
    fn deleted_parent_directory_and_subdirectory_discovery() {
        let (dir, repo) = fixture();
        fs::create_dir(dir.path().join("pkg")).unwrap();
        fs::write(dir.path().join("pkg/a.py"), b"old").unwrap();
        commit(&repo, "pkg/a.py");
        fs::remove_file(dir.path().join("pkg/a.py")).unwrap();
        fs::remove_dir(dir.path().join("pkg")).unwrap();
        let result =
            capture_comparison(&local(false, false), dir.path().join(".git").join("..")).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Deleted);
        assert_eq!(body(&result.pair.base, b"pkg/a.py"), Some(&b"old"[..]));
    }
    #[test]
    fn remote_branch_ref_and_invalid_revision() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        let oid = commit(&repo, "a.py");
        repo.reference("refs/remotes/origin/base", oid, false, "fixture")
            .unwrap();
        let mode = CompareMode::Branch(BranchArgs {
            base: "origin/base".into(),
            target: "refs/remotes/origin/base".into(),
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        });
        let result = capture(&mode, &dir);
        assert_eq!(result.pair.base.commit_id, Some(oid));
        assert_eq!(
            result.pair.base.branch.as_deref(),
            Some("refs/remotes/origin/base")
        );
        let bad = CompareMode::Branch(BranchArgs {
            base: "HEAD~1".into(),
            target: "origin/base".into(),
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        });
        assert_eq!(
            capture_comparison(&bad, dir.path().to_path_buf())
                .unwrap_err()
                .exit_code(),
            2
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_is_skipped_without_following() {
        use std::os::unix::fs::symlink;
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        commit(&repo, "a.py");
        symlink(dir.path().join("a.py"), dir.path().join("link.py")).unwrap();
        let result = capture(&local(false, false), &dir);
        assert!(!result.pair.target.files.contains_key(b"link.py".as_slice()));
    }
    #[cfg(unix)]
    #[test]
    fn tracked_file_under_symlinked_parent_is_skipped() {
        use std::os::unix::fs::symlink;
        let (dir, repo) = fixture();
        fs::create_dir(dir.path().join("pkg")).unwrap();
        fs::write(dir.path().join("pkg/a.py"), b"repository").unwrap();
        commit(&repo, "pkg/a.py");
        fs::remove_file(dir.path().join("pkg/a.py")).unwrap();
        fs::remove_dir(dir.path().join("pkg")).unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("a.py"), b"outside").unwrap();
        symlink(outside.path(), dir.path().join("pkg")).unwrap();

        for mode in [local(false, false), local(false, true)] {
            let result = capture(&mode, &dir);
            assert_eq!(body(&result.pair.target, b"pkg/a.py"), None);
            assert!(
                result
                    .diagnostics
                    .iter()
                    .any(|d| { d.path == b"pkg/a.py" && d.message == "symlink source skipped" })
            );
        }
    }

    #[test]
    fn initialized_submodule_sources_are_not_discovered() {
        let (dir, repo) = fixture();
        let vendor = dir.path().join("vendor");
        fs::create_dir(&vendor).unwrap();
        let subrepo = Repository::init(&vendor).unwrap();
        fs::write(vendor.join("a.py"), b"submodule").unwrap();
        commit(&subrepo, "a.py");
        commit(&repo, "vendor");
        assert!(!repo.is_path_ignored(Path::new("vendor")).unwrap());

        for mode in [local(false, false), local(false, true)] {
            let result = capture(&mode, &dir);
            assert_eq!(body(&result.pair.target, b"vendor/a.py"), None);
            assert!(result.changes.is_empty());
        }

        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("vendor")).unwrap();
        index.write().unwrap();
        let result = capture(&local(false, true), &dir);
        assert_eq!(body(&result.pair.target, b"vendor/a.py"), None);
    }

    #[test]
    fn direct_branch_tips_differ_from_merge_base() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"common").unwrap();
        let ancestor = commit(&repo, "a.py");
        fs::write(dir.path().join("a.py"), b"feature").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.py")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let parent = repo.find_commit(ancestor).unwrap();
        let feature = repo
            .commit(
                Some("refs/heads/feature"),
                &sig,
                &sig,
                "feature",
                &tree,
                &[&parent],
            )
            .unwrap();
        fs::write(dir.path().join("a.py"), b"main").unwrap();
        let main = commit(&repo, "a.py");
        let main_name = repo.head().unwrap().shorthand().unwrap().to_string();
        let mode = CompareMode::Branch(BranchArgs {
            base: main_name,
            target: "feature".into(),
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        });
        let result = capture(&mode, &dir);
        assert_eq!(result.pair.base.commit_id, Some(main));
        assert_eq!(result.pair.target.commit_id, Some(feature));
        assert_eq!(body(&result.pair.base, b"a.py"), Some(&b"main"[..]));
        assert_eq!(body(&result.pair.target, b"a.py"), Some(&b"feature"[..]));
        assert_eq!(result.changes[0].kind, ChangeKind::Modified);
    }
    #[test]
    fn capture_preserves_index_and_worktree_state() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        commit(&repo, "a.py");
        fs::write(dir.path().join("a.py"), b"dirty").unwrap();
        let index_path = repo.path().join("index");
        let before_index = fs::read(&index_path).unwrap();
        let before_status = repo
            .statuses(None)
            .unwrap()
            .iter()
            .map(|e| e.status().bits())
            .collect::<Vec<_>>();
        let first = capture(&local(false, false), &dir);
        let second = capture(&local(false, false), &dir);
        assert_eq!(first.pair.target.content_id, second.pair.target.content_id);
        assert_eq!(fs::read(index_path).unwrap(), before_index);
        assert_eq!(fs::read(dir.path().join("a.py")).unwrap(), b"dirty");
        let after_status = repo
            .statuses(None)
            .unwrap()
            .iter()
            .map(|e| e.status().bits())
            .collect::<Vec<_>>();
        assert_eq!(before_status, after_status);
    }
    #[test]
    fn linked_worktree_is_discovered() {
        let (dir, repo) = fixture();
        fs::write(dir.path().join("a.py"), b"base").unwrap();
        commit(&repo, "a.py");
        let linked = TempDir::new().unwrap();
        let path = linked.path().join("linked");
        repo.worktree("linked", &path, None).unwrap();
        let result = capture_comparison(&local(false, false), path).unwrap();
        assert_eq!(body(&result.pair.base, b"a.py"), Some(&b"base"[..]));
        assert_eq!(body(&result.pair.target, b"a.py"), Some(&b"base"[..]));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_source_path_is_preserved() {
        let (dir, _) = fixture();
        let raw = b"name-\xff.py";
        fs::write(dir.path().join(path_from_bytes(raw)), b"body").unwrap();
        let result = capture(&local(false, false), &dir);
        assert_eq!(body(&result.pair.target, raw), Some(&b"body"[..]));
        assert_eq!(result.changes[0].path, raw);
    }
}
