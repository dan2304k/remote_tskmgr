//! Start-up splash: a small frameless always-on-top note near the tray, shown for `DURATION`
//! when the sender starts minimized, so the user knows where it went. Click to dismiss.
//!
//! Runs on the main thread like the settings window (eframe `run_and_return`); the daemon and
//! tray threads are already running. Tray events are watched while it is up: Quit closes it
//! and quits, Settings… closes it and opens the settings window.

use super::{icon, GuiEvent};
use crossbeam_channel::Receiver;
use eframe::egui;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long the splash stays up.
pub const DURATION: Duration = Duration::from_secs(3);
/// Inner size in logical pixels.
const SIZE: egui::Vec2 = egui::vec2(320.0, 120.0);
/// Gap to the work area's edges, logical pixels.
const MARGIN: f32 = 12.0;
/// Linux: room left for a bottom panel, since the work area is not known there.
#[cfg(not(windows))]
const PANEL_GUESS: f32 = 48.0;

// egui's built-in fonts have no arrow glyphs (▸, →); the arrow is painted instead.
#[cfg(windows)]
const WHERE: &str = "Look for its icon in the system tray, bottom-right (click ^ if it is hidden).";
#[cfg(not(windows))]
const WHERE: &str = "Look for its icon in the system tray, bottom-right (or in the top bar).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Timed out or clicked away; continue in the tray.
    Done,
    /// Settings… was chosen in the tray while the splash was up.
    OpenSettings,
    /// Quit was chosen (or the daemon exited) while the splash was up.
    Quit,
}

/// Fraction of the progress bar still showing after `elapsed`.
pub fn remaining(elapsed: Duration) -> f32 {
    (1.0 - elapsed.as_secs_f32() / DURATION.as_secs_f32()).clamp(0.0, 1.0)
}

/// Top-left corner (points) that puts a `size` window in the bottom-right of `work`.
pub fn bottom_right(work: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    let p = work.max - size - egui::vec2(MARGIN, MARGIN);
    p.max(work.min)
}

/// Show the splash and block until it closes. Tries a frameless always-on-top window first,
/// then a normal small window with a title if the platform refuses that.
pub fn show(events: &Receiver<GuiEvent>) -> anyhow::Result<Outcome> {
    match run(events, true) {
        Ok(o) => Ok(o),
        Err(e) => {
            tracing::debug!("frameless splash failed ({e:#}); trying a normal window");
            run(events, false)
        }
    }
}

fn run(events: &Receiver<GuiEvent>, frameless: bool) -> anyhow::Result<Outcome> {
    let (rgba, width, height) = icon::rgba(64, false);
    let mut vp = egui::ViewportBuilder::default()
        .with_title("rtm-sender")
        .with_app_id("rtm-sender")
        .with_inner_size(SIZE)
        .with_resizable(false)
        .with_taskbar(false)
        .with_active(false)
        .with_icon(egui::IconData {
            rgba: rgba.clone(),
            width,
            height,
        });
    if frameless {
        vp = vp.with_decorations(false).with_always_on_top();
    }
    let options = eframe::NativeOptions {
        viewport: vp,
        renderer: eframe::Renderer::Glow,
        run_and_return: true,
        ..Default::default()
    };
    let outcome = Rc::new(Cell::new(Outcome::Done));
    let app = Splash {
        start: Instant::now(),
        events: events.clone(),
        outcome: outcome.clone(),
        icon: (rgba, width as usize, height as usize),
        tex: None,
        placed: false,
        closing: false,
    };
    eframe::run_native(
        "rtm-sender",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(outcome.get())
}

struct Splash {
    start: Instant,
    events: Receiver<GuiEvent>,
    outcome: Rc<Cell<Outcome>>,
    icon: (Vec<u8>, usize, usize),
    tex: Option<egui::TextureHandle>,
    placed: bool,
    closing: bool,
}

impl Splash {
    fn close(&mut self, ctx: &egui::Context, outcome: Outcome) {
        if !self.closing {
            self.closing = true;
            self.outcome.set(outcome);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Move to the bottom-right of the primary work area once its size is known.
    fn place(&mut self, ctx: &egui::Context) {
        if self.placed {
            return;
        }
        let Some(work) = work_area(ctx) else { return };
        let size = ctx
            .input(|i| i.viewport().outer_rect)
            .map_or(SIZE, |r| r.size());
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(bottom_right(
            work, size,
        )));
        self.placed = true;
    }
}

/// The primary monitor's work area in points (excluding the taskbar).
#[cfg(windows)]
fn work_area(ctx: &egui::Context) -> Option<egui::Rect> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: SPI_GETWORKAREA writes one RECT to the pointer we pass.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut r as *mut RECT).cast(), 0) };
    if ok == 0 || r.right <= r.left {
        return None;
    }
    // Physical pixels (winit makes the process per-monitor DPI aware) -> points.
    let ppp = ctx.pixels_per_point();
    Some(egui::Rect::from_min_max(
        egui::pos2(r.left as f32 / ppp, r.top as f32 / ppp),
        egui::pos2(r.right as f32 / ppp, r.bottom as f32 / ppp),
    ))
}

/// Linux and others: the current monitor minus a guessed bottom panel.
#[cfg(not(windows))]
fn work_area(ctx: &egui::Context) -> Option<egui::Rect> {
    let m = ctx.input(|i| i.viewport().monitor_size)?;
    (m.x > 0.0 && m.y > PANEL_GUESS)
        .then(|| egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(m.x, m.y - PANEL_GUESS)))
}

impl eframe::App for Splash {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                GuiEvent::OpenSettings => self.close(ctx, Outcome::OpenSettings),
                GuiEvent::Quit | GuiEvent::DaemonExited => self.close(ctx, Outcome::Quit),
            }
        }
        if self.start.elapsed() >= DURATION {
            self.close(ctx, Outcome::Done);
        }
        if ctx.input(|i| i.pointer.any_click()) {
            self.close(ctx, Outcome::Done);
        }
        self.place(ctx);
        // Animate the bar; also polls the event channel.
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let tex = self.tex.get_or_insert_with(|| {
            let (rgba, w, h) = &self.icon;
            ui.ctx().load_texture(
                "rtm-icon",
                egui::ColorImage::from_rgba_unmultiplied([*w, *h], rgba),
                egui::TextureOptions::LINEAR,
            )
        });
        let tex = egui::load::SizedTexture::new(tex.id(), egui::vec2(48.0, 48.0));
        let visuals = ui.visuals().clone();
        let frame = egui::Frame::new()
            .fill(visuals.window_fill)
            .stroke(visuals.window_stroke)
            .inner_margin(12.0);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let full = ui.max_rect();
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(tex));
                ui.vertical(|ui| {
                    ui.set_max_width(full.width() - 48.0 - 40.0);
                    ui.label(
                        egui::RichText::new("rtm-sender is running")
                            .strong()
                            .size(16.0),
                    );
                    ui.add(egui::Label::new(WHERE).wrap());
                });
            });
            // An arrow towards the tray corner, and the countdown bar along the bottom.
            let tip = full.right_bottom() - egui::vec2(2.0, 14.0);
            ui.painter().arrow(
                tip - egui::vec2(18.0, 18.0),
                egui::vec2(16.0, 16.0),
                egui::Stroke::new(2.0, visuals.selection.bg_fill),
            );
            let bar = egui::Rect::from_min_max(
                egui::pos2(full.left(), full.bottom() - 3.0),
                full.right_bottom(),
            );
            ui.painter().rect_filled(bar, 0.0, visuals.extreme_bg_color);
            let left = bar.width() * remaining(self.start.elapsed());
            ui.painter().rect_filled(
                egui::Rect::from_min_size(bar.min, egui::vec2(left, bar.height())),
                0.0,
                visuals.selection.bg_fill,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_shrinks_to_zero() {
        assert_eq!(remaining(Duration::ZERO), 1.0);
        assert!((remaining(DURATION / 2) - 0.5).abs() < 1e-6);
        assert_eq!(remaining(DURATION * 2), 0.0);
    }

    #[test]
    fn placed_bottom_right_inside_work_area() {
        let work = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1920.0, 1040.0));
        let p = bottom_right(work, SIZE);
        assert_eq!(
            p,
            egui::pos2(1920.0 - 320.0 - MARGIN, 1040.0 - 120.0 - MARGIN)
        );
        // A tiny screen never pushes it off the top-left.
        let tiny = egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(100.0, 50.0));
        assert_eq!(bottom_right(tiny, SIZE), egui::pos2(10.0, 10.0));
    }
}
