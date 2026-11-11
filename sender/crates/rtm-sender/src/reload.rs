//! Config hot reload: watch the config file's directory with `notify`, debounce bursts of
//! events (editors write in several steps, our own saves are temp-file + rename), then call
//! back once the file has been quiet for `quiet`.

use crossbeam_channel::{RecvTimeoutError, Sender as ChanSender};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Trailing-edge debouncer: fires once, `quiet` after the last event. Pure, no clocks inside.
#[derive(Debug, Clone)]
pub struct Debouncer {
    quiet: Duration,
    last: Option<Instant>,
}

impl Debouncer {
    pub fn new(quiet: Duration) -> Self {
        Self { quiet, last: None }
    }
    pub fn event(&mut self, now: Instant) {
        self.last = Some(now);
    }
    /// When the pending change becomes due, if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.last.map(|t| t + self.quiet)
    }
    /// True (once) if a change is pending and has been quiet long enough.
    pub fn fire(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(d) if now >= d => {
                self.last = None;
                true
            }
            _ => false,
        }
    }
}

/// Is this notify event about our file, and a content change (not a read/access)?
fn relevant(ev: &notify::Event, file_name: &OsString) -> bool {
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    ) && ev
        .paths
        .iter()
        .any(|p| p.file_name() == Some(file_name.as_os_str()))
}

/// Start watching `path`. `on_change` runs on a background thread after each debounced
/// burst. Keep the returned watcher alive; dropping it stops the watch and the thread.
pub fn watch<F>(path: &Path, quiet: Duration, on_change: F) -> notify::Result<RecommendedWatcher>
where
    F: Fn() + Send + 'static,
{
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir: PathBuf = abs
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name: OsString = abs
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_default();

    let (tx, rx) = crossbeam_channel::unbounded::<()>();
    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(ev) if relevant(&ev, &file_name) => {
                let _ = tx.send(());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("config watch error: {e}"),
        })?;
    // Watch the directory, not the file: atomic saves replace the inode.
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    std::thread::Builder::new()
        .name("rtm-config-watch".into())
        .spawn(move || debounce_loop(rx, quiet, on_change))
        .map_err(notify::Error::io)?;
    tracing::debug!(dir = %dir.display(), "watching config");
    Ok(watcher)
}

fn debounce_loop<F: Fn()>(rx: crossbeam_channel::Receiver<()>, quiet: Duration, on_change: F) {
    let mut deb = Debouncer::new(quiet);
    loop {
        let r = match deb.deadline() {
            Some(d) => rx.recv_deadline(d),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match r {
            Ok(()) => deb.event(Instant::now()),
            Err(RecvTimeoutError::Timeout) => {
                if deb.fire(Instant::now()) {
                    on_change();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Convenience used by `main`: on each debounced change, run `load` and forward its result.
pub fn watch_into<T, L>(
    path: &Path,
    tx: ChanSender<T>,
    load: L,
) -> notify::Result<RecommendedWatcher>
where
    T: Send + 'static,
    L: Fn() -> Option<T> + Send + 'static,
{
    watch(path, DEBOUNCE, move || {
        if let Some(v) = load() {
            let _ = tx.send(v);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn debouncer_fires_once_after_quiet_period() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = Debouncer::new(ms(500));
        assert!(d.deadline().is_none());
        assert!(!d.fire(t0));
        d.event(t0);
        d.event(t0 + ms(100));
        d.event(t0 + ms(300));
        assert_eq!(d.deadline(), Some(t0 + ms(800)));
        assert!(!d.fire(t0 + ms(600)), "burst extends the window");
        assert!(!d.fire(t0 + ms(799)));
        assert!(d.fire(t0 + ms(800)));
        assert!(!d.fire(t0 + ms(900)), "fires only once");
        assert!(d.deadline().is_none());
        d.event(t0 + ms(1000));
        assert!(d.fire(t0 + ms(1600)));
    }

    #[test]
    fn watcher_reports_debounced_change() {
        let dir = std::env::temp_dir().join(format!("rtm-reload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sender.toml");
        std::fs::write(&path, "a = 1\n").unwrap();

        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        // Generous debounce: loaded CI runners can stall the writer for hundreds of ms.
        let _w = watch(&path, Duration::from_millis(1000), move || {
            h.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        // A burst of writes plus an unrelated file: one callback.
        for i in 0..5 {
            std::fs::write(&path, format!("a = {i}\n")).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(dir.join("other.toml"), "x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Atomic replace (temp + rename) is also seen.
        std::fs::write(dir.join("sender.toml.tmp"), "a = 9\n").unwrap();
        std::fs::rename(dir.join("sender.toml.tmp"), &path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) == 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
