use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};

pub struct Watcher {
    _watcher: RecommendedWatcher,
    markers: tempfile::TempDir,
    events: Receiver<notify::Result<Event>>,
    paths: BTreeSet<PathBuf>,
    sequence: u64,
}

impl Watcher {
    pub fn start(root: &Path, git_dir: &Path) -> io::Result<Self> {
        let markers = tempfile::Builder::new()
            .prefix("kao-watch-")
            .tempdir_in(git_dir)?;
        let (sender, events) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = sender.send(event);
            },
            Config::default().with_follow_symlinks(false),
        )
        .map_err(io::Error::other)?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;
        if !git_dir.starts_with(root) {
            watcher
                .watch(markers.path(), RecursiveMode::Recursive)
                .map_err(io::Error::other)?;
        }
        let mut watcher = Self {
            _watcher: watcher,
            markers,
            events,
            paths: BTreeSet::new(),
            sequence: 0,
        };
        watcher.barrier()?;
        watcher.paths.clear();
        Ok(watcher)
    }

    fn barrier(&mut self) -> io::Result<()> {
        self.sequence += 1;
        let marker = self.markers.path().join(self.sequence.to_string());
        fs::write(&marker, b"capture barrier\n")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(remaining)
                .map_err(|error| io::Error::other(format!("watcher barrier failed: {error}")))?
                .map_err(io::Error::other)?;
            if event.need_rescan() {
                return Err(io::Error::other(
                    "filesystem events were lost; capture is incomplete",
                ));
            }
            let observed = event.paths.iter().any(|path| path == &marker);
            self.paths.extend(event.paths);
            if observed {
                return Ok(());
            }
        }
    }

    pub fn finish(mut self) -> io::Result<BTreeSet<PathBuf>> {
        self.barrier()?;
        self.barrier()?;
        Ok(std::mem::take(&mut self.paths))
    }
}
