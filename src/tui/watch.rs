use crate::cli::CompareMode;
use crate::languages;
use git2::Repository;
use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(150);
const MAX_BURST: Duration = Duration::from_secs(1);
const HEARTBEAT: Duration = Duration::from_secs(5);
const FALLBACK: Duration = Duration::from_millis(750);

struct Paths {
    workdir: Option<PathBuf>,
    gitdir: PathBuf,
    common: PathBuf,
}

impl Paths {
    fn relevant(&self, event: &Event) -> bool {
        if event.paths.is_empty() {
            return true;
        }
        event
            .paths
            .iter()
            .any(|path| self.path_relevant(path, &event.kind))
    }

    fn path_relevant(&self, path: &Path, kind: &EventKind) -> bool {
        if self.metadata(path) {
            return true;
        }
        // Git directories can reside inside the worktree (or outside it for linked worktrees).
        if path.starts_with(&self.gitdir) || path.starts_with(&self.common) {
            return false;
        }
        let Some(relative) = self
            .workdir
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
        else {
            return false;
        };
        if relative.as_os_str().is_empty()
            || relative.components().any(|part| part.as_os_str() == ".git")
        {
            return false;
        }
        if relative
            .file_name()
            .is_some_and(|name| name == ".gitignore")
        {
            return true;
        }
        // Removed paths cannot be stat'ed; an unknown removal may be a directory,
        // including one with a dot in its name.
        if matches!(
            kind,
            EventKind::Create(CreateKind::Folder)
                | EventKind::Remove(RemoveKind::Folder | RemoveKind::Any)
        ) || (matches!(
            kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        ) && (path.is_dir() || path.extension().is_none()))
        {
            return true;
        }
        languages::capture_path(&path_bytes(relative))
    }

    fn metadata(&self, path: &Path) -> bool {
        [self.gitdir.as_path(), self.common.as_path()]
            .into_iter()
            .any(|root| {
                let Ok(relative) = path.strip_prefix(root) else {
                    return false;
                };
                relative == Path::new("index")
                    || relative == Path::new("HEAD")
                    || relative == Path::new("packed-refs")
                    || relative == Path::new("info/exclude")
                    || relative == Path::new("refs")
                    || relative.starts_with("refs/")
            })
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

pub struct ReloadWatch {
    watcher: Option<RecommendedWatcher>,
    receiver: Option<Receiver<notify::Result<Event>>>,
    paths: Option<Paths>,
    first_dirty: Option<Instant>,
    last_dirty: Option<Instant>,
    last_capture: Instant,
}

impl ReloadWatch {
    pub fn new(mode: &CompareMode, discovery_dir: &Path) -> Self {
        let mut result = Self {
            watcher: None,
            receiver: None,
            paths: None,
            first_dirty: None,
            last_dirty: None,
            last_capture: Instant::now(),
        };
        let CompareMode::Local(args) = mode else {
            return result;
        };
        let Ok(repo) = Repository::discover(discovery_dir) else {
            return result;
        };
        let paths = Paths {
            workdir: if args.staged {
                None
            } else {
                repo.workdir().map(Path::to_path_buf)
            },
            gitdir: repo.path().to_path_buf(),
            common: repo.commondir().to_path_buf(),
        };
        let (tx, rx) = mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else {
            return result;
        };
        let mut registered = HashSet::new();
        let registrations = paths
            .workdir
            .iter()
            .map(|path| (path.clone(), RecursiveMode::Recursive))
            .chain([
                (paths.gitdir.clone(), RecursiveMode::NonRecursive),
                (paths.common.clone(), RecursiveMode::NonRecursive),
            ])
            .chain(
                [paths.gitdir.join("info"), paths.common.join("info")]
                    .into_iter()
                    .filter(|path| path.is_dir())
                    .map(|path| (path, RecursiveMode::NonRecursive)),
            )
            .chain(
                [paths.common.join("refs")]
                    .into_iter()
                    .filter(|path| path.is_dir())
                    .map(|path| (path, RecursiveMode::Recursive)),
            );
        for (path, recursion) in registrations {
            if registered.insert(path.clone()) && watcher.watch(&path, recursion).is_err() {
                return result;
            }
        }
        result.paths = Some(paths);
        result.receiver = Some(rx);
        result.watcher = Some(watcher);
        result
    }

    fn ingest(&mut self, now: Instant) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        let mut relevant = false;
        let mut failed = false;
        while let Ok(event) = receiver.try_recv() {
            match event {
                Ok(event) => {
                    relevant |= self
                        .paths
                        .as_ref()
                        .is_none_or(|paths| paths.relevant(&event))
                }
                Err(_) => failed = true,
            }
        }
        if failed {
            self.watcher = None;
            self.receiver = None;
            self.first_dirty = None;
            self.last_dirty = None;
        } else if relevant {
            self.first_dirty.get_or_insert(now);
            self.last_dirty = Some(now);
        }
    }

    pub fn should_capture(&mut self, now: Instant) -> bool {
        self.ingest(now);
        if self.watcher.is_none() {
            return now.duration_since(self.last_capture) >= FALLBACK;
        }
        self.first_dirty
            .is_some_and(|first| now.duration_since(first) >= MAX_BURST)
            || self
                .last_dirty
                .is_some_and(|last| now.duration_since(last) >= QUIET)
            || now.duration_since(self.last_capture) >= HEARTBEAT
    }

    pub fn captured(&mut self, now: Instant) {
        self.last_capture = now;
        self.first_dirty = None;
        self.last_dirty = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};

    fn paths() -> Paths {
        Paths {
            workdir: Some("/repo".into()),
            gitdir: "/repo/.git".into(),
            common: "/repo/.git".into(),
        }
    }
    fn event(kind: EventKind, paths: &[&str]) -> Event {
        let mut event = Event::new(kind);
        event.paths = paths.iter().map(PathBuf::from).collect();
        event
    }
    #[test]
    fn filters_sources_renames_and_metadata() {
        let paths = paths();
        assert!(paths.relevant(&event(
            EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Both)),
            &["/repo/a.py", "/repo/b.py"]
        )));
        assert!(paths.relevant(&event(
            EventKind::Create(CreateKind::Any),
            &["/repo/newdir"]
        )));
        assert!(paths.relevant(&event(
            EventKind::Remove(RemoveKind::Any),
            &["/repo/.gitignore"]
        )));
        assert!(paths.relevant(&event(
            EventKind::Remove(RemoveKind::Folder),
            &["/repo/pkg.v2"]
        )));
        assert!(paths.relevant(&event(EventKind::Any, &["/repo/.git/index"])));
        assert!(paths.relevant(&event(EventKind::Any, &["/repo/.git/info/exclude"])));
        assert!(!paths.relevant(&event(EventKind::Any, &["/repo/.git/objects/12/abc"])));
        assert!(!paths.relevant(&event(EventKind::Any, &["/repo/.git/logs/HEAD"])));
        assert!(!paths.relevant(&event(EventKind::Any, &["/repo/unrelated.txt"])));
        assert!(paths.relevant(&event(EventKind::Any, &[])));
        let staged = Paths {
            workdir: None,
            ..paths
        };
        assert!(!staged.relevant(&event(EventKind::Any, &["/repo/a.py"])));
        assert!(staged.relevant(&event(EventKind::Any, &["/repo/.git/index"])));
    }
    #[test]
    fn coalesces_bursts_and_heartbeats() {
        let now = Instant::now();
        let (tx, rx) = mpsc::channel();
        let watcher = notify::recommended_watcher(tx.clone()).unwrap();
        let mut watch = ReloadWatch {
            watcher: Some(watcher),
            receiver: Some(rx),
            paths: Some(paths()),
            first_dirty: None,
            last_dirty: None,
            last_capture: now,
        };
        tx.send(Ok(event(EventKind::Any, &["/repo/unrelated.txt"])))
            .unwrap();
        assert!(!watch.should_capture(now + QUIET));
        tx.send(Ok(event(EventKind::Any, &["/repo/a.py"]))).unwrap();
        assert!(!watch.should_capture(now));
        assert!(!watch.should_capture(now + QUIET - Duration::from_millis(1)));
        assert!(watch.should_capture(now + QUIET));
        watch.captured(now + QUIET);
        tx.send(Ok(event(EventKind::Any, &["/repo/a.py"]))).unwrap();
        assert!(!watch.should_capture(now + QUIET));
        for millis in [400, 700, 1000] {
            tx.send(Ok(event(EventKind::Any, &["/repo/a.py"]))).unwrap();
            assert!(!watch.should_capture(now + Duration::from_millis(millis)));
        }
        assert!(watch.should_capture(now + QUIET + MAX_BURST));
        watch.captured(now + QUIET + MAX_BURST);
        assert!(
            !watch.should_capture(now + QUIET + MAX_BURST + HEARTBEAT - Duration::from_millis(1))
        );
        assert!(watch.should_capture(now + QUIET + MAX_BURST + HEARTBEAT));
        watch.captured(now + QUIET + MAX_BURST + HEARTBEAT);
        tx.send(Err(notify::Error::generic("watch failed")))
            .unwrap();
        assert!(!watch.should_capture(now + QUIET + MAX_BURST + HEARTBEAT));
        assert!(watch.should_capture(now + QUIET + MAX_BURST + HEARTBEAT + FALLBACK));
    }
}
