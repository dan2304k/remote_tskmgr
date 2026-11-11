//! `gui` feature: tray icon + egui settings window.
//!
//! Threads:
//! * main thread: waits for tray requests and runs the eframe settings window on demand
//!   (eframe keeps its winit event loop in a thread-local, so the window can be opened and
//!   closed any number of times).
//! * `rtm-daemon`: the unchanged daemon loop (`app::run_with_status`).
//! * `rtm-tray`: owns the tray icon and pumps the platform event loop it needs (GTK on Linux,
//!   Win32 messages on Windows).
//!
//! The GUI talks to the daemon only through the existing control channel plus the shared
//! `Config` (what is in the file) and `Status` (what the loop is doing).

pub mod autostart;
#[cfg(windows)]
pub mod console;
pub mod icon;
pub mod settings;
pub mod splash;
pub mod tray;

use crate::app::Control;
use crate::status::{SharedStatus, DAEMON_CRASHED};
use anyhow::Context;
use crossbeam_channel::{Receiver, Sender};
use parking_lot::RwLock;
use rtm_config::Config;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The config as stored in the file (no CLI overrides). The GUI edits and saves this.
pub type SharedConfig = Arc<RwLock<Config>>;
/// Applies CLI overrides and validates, exactly like a hot reload does.
pub type Effective = Arc<dyn Fn(Config) -> anyhow::Result<Config> + Send + Sync>;

/// Intervals offered in the tray and the settings window (1 s .. 30 s, plus 10/s and 5/s).
pub const INTERVALS_MS: [u64; 6] = [100, 200, 1000, 5000, 15000, 30000];

/// "1 s", "5 s", ... for whole seconds, "10/s (100 ms)" for whole rates per second below a
/// second, "750 ms" otherwise.
pub fn interval_label(ms: u64) -> String {
    if ms > 0 && ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else if ms > 0 && ms < 1000 && 1000u64.is_multiple_of(ms) {
        format!("{}/s ({ms} ms)", 1000 / ms)
    } else {
        format!("{ms} ms")
    }
}

/// Events for the main (GUI) thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiEvent {
    OpenSettings,
    Quit,
    /// The daemon loop returned (Ctrl-C, error) or panicked (then `Status::crashed` is set);
    /// the GUI must go too.
    DaemonExited,
}

/// Everything the tray and the settings window need. Cheap to clone, `Send`.
#[derive(Clone)]
pub struct Ctl {
    pub cfg: SharedConfig,
    pub status: SharedStatus,
    pub path: PathBuf,
    control: Sender<Control>,
    events: Sender<GuiEvent>,
    effective: Effective,
    paused: Arc<AtomicBool>,
}

impl Ctl {
    /// Validate, register/unregister autostart if it differs from the real state, write the TOML, hot-apply.
    pub fn save(&self, new: Config) -> anyhow::Result<()> {
        new.validate()?;
        let effective = (self.effective)(new.clone()).context("with command line overrides")?;
        // Compare with the real registration, not the old config: installers register it too.
        if new.gui.autostart != autostart::is_enabled() {
            autostart::set_autostart(new.gui.autostart).context("autostart")?;
        }
        new.save(&self.path)
            .with_context(|| format!("writing {}", self.path.display()))?;
        *self.cfg.write() = new;
        // The file watcher will also notice the write; that second reload diffs to nothing.
        let _ = self.control.send(Control::Reload(Box::new(effective)));
        tracing::info!(path = %self.path.display(), "settings saved");
        Ok(())
    }

    pub fn set_interval(&self, ms: u64) -> anyhow::Result<()> {
        let mut c = self.cfg.read().clone();
        if c.general.interval_ms == ms {
            return Ok(());
        }
        c.general.interval_ms = ms;
        c.gui.autostart = autostart::is_enabled(); // keep the registration as it is
        self.save(c)
    }

    pub fn toggle_pause(&self) {
        let p = !self.paused.fetch_xor(true, Ordering::SeqCst);
        let _ = self.control.send(Control::Pause(p));
    }

    pub fn request(&self, ev: GuiEvent) {
        let _ = self.events.send(ev);
    }
}

/// Sends `DaemonExited` when the daemon thread ends, however it ends. On a panic it first
/// marks the shared status as crashed so the tray and window show it. The workspace release
/// profile has `panic = "abort"`, so there a panic kills the whole process (tray included)
/// and this guard never runs; it matters for unwinding (dev/test) builds.
struct DaemonExitGuard {
    status: SharedStatus,
    events: Sender<GuiEvent>,
}

impl Drop for DaemonExitGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!("{DAEMON_CRASHED}: the daemon thread panicked");
            let mut s = self.status.write();
            s.running = true;
            s.crashed = true;
            s.last_error = Some(DAEMON_CRASHED.into());
        }
        let _ = self.events.send(GuiEvent::DaemonExited);
    }
}

/// Run `f` (the daemon loop) with a [`DaemonExitGuard`].
fn guarded<R>(status: SharedStatus, events: Sender<GuiEvent>, f: impl FnOnce() -> R) -> R {
    let _guard = DaemonExitGuard { status, events };
    f()
}

/// Why the GUI cannot start here, if we can tell up front.
pub fn unavailable_reason() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    {
        let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if !set("DISPLAY") && !set("WAYLAND_DISPLAY") {
            return Some("neither DISPLAY nor WAYLAND_DISPLAY is set");
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The tray would have to share the main thread with winit; not implemented.
        return Some("the tray is not implemented on macOS");
    }
    #[allow(unreachable_code)]
    None
}

pub struct Launch {
    /// Effective config (file + CLI overrides) to start the daemon with.
    pub cfg: Config,
    pub shared_cfg: SharedConfig,
    pub path: PathBuf,
    pub control_tx: Sender<Control>,
    pub control_rx: Receiver<Control>,
    pub effective: Effective,
}

/// Run the daemon on a background thread and the GUI on this one. Falls back to plain
/// headless behaviour (just waiting for the daemon) if the tray cannot be created.
pub fn run(l: Launch) -> anyhow::Result<()> {
    let status = SharedStatus::default();
    let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
    let gui_cfg = l.shared_cfg.read().gui.clone();
    let start_window = !gui_cfg.start_minimized;
    let show_splash = gui_cfg.start_minimized && gui_cfg.show_splash;

    let daemon = {
        let (cfg, rx, status, ev_tx) = (l.cfg, l.control_rx, status.clone(), ev_tx.clone());
        std::thread::Builder::new()
            .name("rtm-daemon".into())
            .spawn(move || {
                guarded(status.clone(), ev_tx, || {
                    crate::app::run_with_status(cfg, rx, Some(status))
                })
            })
            .context("spawning daemon thread")?
    };
    let join_daemon = move || {
        daemon
            .join()
            .map_err(|_| anyhow::anyhow!("daemon thread panicked"))?
    };

    let ctl = Ctl {
        cfg: l.shared_cfg,
        status,
        path: l.path,
        control: l.control_tx.clone(),
        events: ev_tx,
        effective: l.effective,
        paused: Arc::default(),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let tray = match tray::spawn(ctl.clone(), stop.clone()) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("tray icon unavailable ({e:#}); running headless");
            return join_daemon();
        }
    };

    let mut open = start_window;
    let mut window_ok = true;
    let mut run_loop = true;
    // The daemon and the tray are already running; the splash only occupies this thread.
    if show_splash {
        match splash::show(&ev_rx) {
            Ok(splash::Outcome::Done) => {}
            Ok(splash::Outcome::OpenSettings) => open = true,
            Ok(splash::Outcome::Quit) => run_loop = false,
            Err(e) => tracing::warn!("start-up splash unavailable: {e:#}"),
        }
    }
    if run_loop {
        loop {
            if open && window_ok {
                open = false;
                match settings::show(&ctl, &ev_rx) {
                    Ok(settings::Outcome::Closed) => continue,
                    Ok(settings::Outcome::Quit) => break,
                    Err(e) => {
                        tracing::warn!("settings window unavailable: {e:#}");
                        window_ok = false;
                    }
                }
            }
            match ev_rx.recv() {
                Ok(GuiEvent::OpenSettings) if window_ok => open = true,
                Ok(GuiEvent::OpenSettings) => tracing::warn!("settings window unavailable"),
                Ok(GuiEvent::Quit) | Ok(GuiEvent::DaemonExited) | Err(_) => break,
            }
        }
    }

    tracing::debug!("gui: shutting down");
    let _ = l.control_tx.send(Control::Shutdown);
    let res = join_daemon();
    stop.store(true, Ordering::SeqCst);
    if tray.join().is_err() {
        tracing::warn!("tray thread panicked");
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of `interval_label` over the presets (what a menu click maps back to).
    fn interval_from_label(label: &str) -> Option<u64> {
        INTERVALS_MS
            .into_iter()
            .find(|&ms| interval_label(ms) == label.trim())
    }

    #[test]
    fn interval_labels_round_trip() {
        let labels: Vec<String> = INTERVALS_MS.iter().map(|&m| interval_label(m)).collect();
        assert_eq!(
            labels,
            [
                "10/s (100 ms)",
                "5/s (200 ms)",
                "1 s",
                "5 s",
                "15 s",
                "30 s"
            ]
        );
        for &ms in &INTERVALS_MS {
            assert_eq!(interval_from_label(&interval_label(ms)), Some(ms));
        }
        assert_eq!(interval_label(750), "750 ms");
        assert_eq!(interval_label(500), "2/s (500 ms)");
        assert_eq!(interval_label(300), "300 ms");
        assert_eq!(interval_from_label("10/s (100 ms)"), Some(100));
        assert_eq!(interval_label(2000), "2 s");
        assert_eq!(interval_from_label("2 s"), None, "not a preset");
        assert_eq!(interval_from_label("bogus"), None);
    }

    fn test_ctl(dir: &std::path::Path) -> (Ctl, Receiver<Control>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ev_tx, _ev_rx) = crossbeam_channel::unbounded();
        let mut cfg = Config::default();
        cfg.general.id = "test-id".into();
        let ctl = Ctl {
            cfg: Arc::new(RwLock::new(cfg)),
            status: Default::default(),
            path: dir.join("sender.toml"),
            control: tx,
            events: ev_tx,
            effective: Arc::new(|c: Config| {
                c.validate()?;
                Ok(c)
            }),
            paused: Arc::default(),
        };
        (ctl, rx)
    }

    #[test]
    fn set_interval_saves_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let (ctl, rx) = test_ctl(dir.path());
        ctl.set_interval(15000).unwrap();
        assert_eq!(ctl.cfg.read().general.interval_ms, 15000);
        let on_disk = Config::load(&ctl.path).unwrap();
        assert_eq!(on_disk.general.interval_ms, 15000);
        assert_eq!(on_disk.general.id, "test-id");
        match rx.try_recv() {
            Ok(Control::Reload(c)) => assert_eq!(c.general.interval_ms, 15000),
            _ => panic!("expected a reload"),
        }
        // Same value again: nothing written, nothing sent.
        ctl.set_interval(15000).unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn invalid_save_is_rejected_without_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let (ctl, rx) = test_ctl(dir.path());
        let mut bad = ctl.cfg.read().clone();
        bad.network.tcp_unicast = vec!["no pe".into()];
        assert!(ctl.save(bad).is_err());
        assert!(!ctl.path.exists());
        assert!(rx.try_recv().is_err());
        assert!(ctl.cfg.read().network.tcp_unicast.is_empty());
    }

    #[test]
    fn daemon_exit_is_reported_even_on_panic() {
        let status = SharedStatus::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        assert_eq!(guarded(status.clone(), tx.clone(), || 7), 7);
        assert_eq!(rx.try_recv(), Ok(GuiEvent::DaemonExited));
        assert!(!status.read().crashed, "a normal return is not a crash");

        let (st, t) = (status.clone(), tx.clone());
        let h = std::thread::spawn(move || guarded(st, t, || panic!("test panic in daemon")));
        assert!(h.join().is_err());
        assert_eq!(rx.try_recv(), Ok(GuiEvent::DaemonExited));
        let s = status.read();
        assert!(s.crashed && s.running);
        assert_eq!(s.last_error.as_deref(), Some(DAEMON_CRASHED));
    }

    #[test]
    fn pause_toggles() {
        let dir = tempfile::tempdir().unwrap();
        let (ctl, rx) = test_ctl(dir.path());
        ctl.toggle_pause();
        ctl.toggle_pause();
        let got: Vec<bool> = rx
            .try_iter()
            .map(|c| match c {
                Control::Pause(p) => p,
                _ => panic!("unexpected control"),
            })
            .collect();
        assert_eq!(got, [true, false]);
    }
}
