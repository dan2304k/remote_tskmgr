//! The tray icon and its menu. Lives on its own thread because the tray needs a platform
//! event loop on the thread that created it (GTK on Linux, a Win32 message pump on Windows)
//! and the main thread belongs to winit/eframe.

use super::{icon, interval_label, Ctl, GuiEvent, INTERVALS_MS};
use crate::status::{Status, DAEMON_CRASHED};
use anyhow::Context;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// How often the tray thread pumps events and refreshes from `Status`.
const POLL: Duration = Duration::from_millis(100);

#[cfg(windows)]
const ICON_SIZE: u32 = 32;
#[cfg(not(windows))]
const ICON_SIZE: u32 = 64;

/// What the tray shows; recomputed from `Status` every poll and applied only when it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    pub status_line: String,
    pub tooltip: String,
    pub paused: bool,
    pub interval_ms: u64,
}

impl TrayView {
    pub fn from_status(s: &Status) -> Self {
        if s.crashed {
            return Self {
                status_line: DAEMON_CRASHED.into(),
                tooltip: format!("rtm-sender · {DAEMON_CRASHED}"),
                paused: s.paused,
                interval_ms: s.interval_ms,
            };
        }
        if !s.running {
            return Self {
                status_line: "starting…".into(),
                tooltip: "rtm-sender · starting".into(),
                paused: false,
                interval_ms: 0,
            };
        }
        let targets = match s.target_count {
            1 => "1 target".to_string(),
            n => format!("{n} targets"),
        };
        let mut line = format!("{} · {} · {targets}", s.host, interval_label(s.interval_ms));
        if s.paused {
            line.push_str(" · paused");
        } else if s.last_error.is_some() {
            line.push_str(" · send error");
        }
        Self {
            tooltip: format!("rtm-sender · {line}"),
            status_line: line,
            paused: s.paused,
            interval_ms: s.interval_ms,
        }
    }
}

struct Tray {
    icon: TrayIcon,
    status_item: MenuItem,
    intervals: Vec<(u64, CheckMenuItem)>,
    pause: MenuItem,
    settings: MenuId,
    open_log: MenuId,
    open_config: MenuId,
    quit: MenuId,
    last: Option<TrayView>,
}

fn tray_icon(paused: bool) -> anyhow::Result<tray_icon::Icon> {
    let (rgba, w, h) = icon::rgba(ICON_SIZE, paused);
    tray_icon::Icon::from_rgba(rgba, w, h).context("tray icon image")
}

impl Tray {
    fn new() -> anyhow::Result<Self> {
        let status_item = MenuItem::new("rtm-sender: starting…", false, None);
        let intervals: Vec<(u64, CheckMenuItem)> = INTERVALS_MS
            .iter()
            .map(|&ms| {
                let label = format!("Interval {}", interval_label(ms));
                (ms, CheckMenuItem::new(label, true, false, None))
            })
            .collect();
        let pause = MenuItem::new("Pause sending", true, None);
        let settings = MenuItem::new("Settings…", true, None);
        let open_log = MenuItem::new("Open log file", true, None);
        let open_config = MenuItem::new("Open config folder", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append(&status_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        for (_, item) in &intervals {
            menu.append(item)?;
        }
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&pause)?;
        menu.append(&settings)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&open_log)?;
        menu.append(&open_config)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("rtm-sender")
            .with_title("rtm-sender")
            .with_icon(tray_icon(false)?)
            .build()
            .context("creating tray icon")?;
        Ok(Self {
            icon,
            status_item,
            intervals,
            pause,
            settings: settings.id().clone(),
            open_log: open_log.id().clone(),
            open_config: open_config.id().clone(),
            quit: quit.id().clone(),
            last: None,
        })
    }

    fn handle_events(&mut self, ctl: &Ctl) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.quit {
                ctl.request(GuiEvent::Quit);
            } else if ev.id == self.settings {
                ctl.request(GuiEvent::OpenSettings);
            } else if ev.id == self.open_log {
                open_log(ctl);
            } else if ev.id == self.open_config {
                open_path(&ctl.config_dir());
            } else if ev.id == *self.pause.id() {
                ctl.toggle_pause();
            } else if let Some((ms, _)) = self.intervals.iter().find(|(_, i)| i.id() == &ev.id) {
                if let Err(e) = ctl.set_interval(*ms) {
                    tracing::error!("changing interval: {e:#}");
                }
                // The click toggled the check mark itself; re-sync from the real state.
                self.last = None;
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            // Windows: left click opens the settings (Linux AppIndicator only has the menu).
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                ctl.request(GuiEvent::OpenSettings);
            }
        }
    }

    fn refresh(&mut self, ctl: &Ctl) {
        let view = TrayView::from_status(&ctl.status.read());
        if self.last.as_ref() == Some(&view) {
            return;
        }
        let paused_changed = self.last.as_ref().is_none_or(|l| l.paused != view.paused);
        self.status_item.set_text(&view.status_line);
        let _ = self.icon.set_tooltip(Some(&view.tooltip));
        for (ms, item) in &self.intervals {
            item.set_checked(*ms == view.interval_ms);
        }
        self.pause.set_text(if view.paused {
            "Resume sending"
        } else {
            "Pause sending"
        });
        if paused_changed {
            match tray_icon(view.paused) {
                Ok(i) => {
                    let _ = self.icon.set_icon(Some(i));
                }
                Err(e) => tracing::warn!("{e:#}"),
            }
        }
        self.last = Some(view);
    }
}

/// The log file in the default app (the folder instead if there is no log yet).
fn open_log(ctl: &Ctl) {
    let log = ctl.log_path();
    if log.is_file() {
        open_path(&log);
    } else {
        tracing::warn!("no log file at {} yet; opening its folder", log.display());
        open_path(&ctl.config_dir());
    }
}

/// Open a file or folder with the desktop's default handler (`explorer` on Windows, `open`
/// on macOS, `xdg-open` elsewhere). Failures are logged.
fn open_path(path: &std::path::Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let spawned = std::process::Command::new(opener)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        // Reap it off this thread (explorer's exit code is meaningless, so it is not checked).
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::error!("cannot run {opener} {}: {e}", path.display()),
    }
}

/// Start the tray thread. Returns once the icon exists, or the error that prevented it.
pub fn spawn(ctl: Ctl, stop: Arc<AtomicBool>) -> anyhow::Result<JoinHandle<()>> {
    let (init_tx, init_rx) = crossbeam_channel::bounded::<anyhow::Result<()>>(1);
    let handle = std::thread::Builder::new()
        .name("rtm-tray".into())
        .spawn(move || {
            let tray = platform::init().and_then(|()| Tray::new());
            let mut tray = match tray {
                Ok(t) => {
                    let _ = init_tx.send(Ok(()));
                    t
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return;
                }
            };
            while !stop.load(Ordering::SeqCst) {
                platform::pump();
                tray.handle_events(&ctl);
                tray.refresh(&ctl);
                std::thread::sleep(POLL);
            }
            drop(tray);
            platform::pump(); // let the platform process the icon's removal
        })
        .context("spawning tray thread")?;
    match init_rx.recv() {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(e)
        }
        Err(_) => {
            let _ = handle.join();
            anyhow::bail!("tray thread exited during start-up")
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    pub fn init() -> anyhow::Result<()> {
        gtk::init().map_err(|e| anyhow::anyhow!("GTK init failed: {e}"))
    }
    pub fn pump() {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    pub fn init() -> anyhow::Result<()> {
        Ok(())
    }
    /// Drain this thread's message queue (the tray's hidden window lives on this thread).
    pub fn pump() {
        // SAFETY: standard message loop on the current thread; `msg` is a valid out-pointer.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub fn init() -> anyhow::Result<()> {
        anyhow::bail!("tray not supported on this platform")
    }
    pub fn pump() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_from_status() {
        let v = TrayView::from_status(&Status::default());
        assert_eq!(v.status_line, "starting…");

        let mut s = Status {
            running: true,
            host: "box".into(),
            interval_ms: 1000,
            target_count: 2,
            ..Default::default()
        };
        let v = TrayView::from_status(&s);
        assert_eq!(v.tooltip, "rtm-sender · box · 1 s · 2 targets");
        assert_eq!(v.interval_ms, 1000);
        assert!(!v.paused);

        s.target_count = 1;
        s.interval_ms = 30000;
        s.paused = true;
        let v = TrayView::from_status(&s);
        assert_eq!(v.status_line, "box · 30 s · 1 target · paused");
        assert!(v.paused);

        s.interval_ms = 100;
        s.paused = false;
        s.crashed = true;
        s.last_error = Some(DAEMON_CRASHED.into());
        let v = TrayView::from_status(&s);
        assert_eq!(v.status_line, "daemon stopped unexpectedly");
        assert_eq!(v.tooltip, "rtm-sender · daemon stopped unexpectedly");
        let mut s = Status {
            crashed: true,
            ..Default::default()
        };
        assert_eq!(
            TrayView::from_status(&s).status_line,
            DAEMON_CRASHED,
            "even before the first publish"
        );
        s.crashed = false;
        s.running = true;
        s.host = "box".into();
        s.target_count = 1;
        s.interval_ms = 100;
        assert_eq!(
            TrayView::from_status(&s).status_line,
            "box · 10/s (100 ms) · 1 target"
        );
    }
}
