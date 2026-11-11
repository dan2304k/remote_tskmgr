//! The settings window: an egui form over a copy of the config (`Form`), plus a read-only
//! status panel. Nothing is applied until Save.

use super::{icon, interval_label, Ctl, GuiEvent, INTERVALS_MS};
use crate::status::Status;
use crossbeam_channel::Receiver;
use eframe::egui;
use rtm_collect::gpu::adapters::{self as gpu_adapters, Adapter};
use rtm_collect::{GpuBackend, KNOWN_IDS};
use rtm_config::{
    parse_target_spec, BroadcastInterfaces, Config, TargetSpec, GPU_BACKENDS, MIN_INTERVAL_MS,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Form model (pure, unit tested)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfaceMode {
    All,
    Default,
    Named,
}

/// Editable copy of `Config`. Lists are edited as rows of text; empty rows are dropped on Save.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub host_name: String,
    pub interval_ms: u64,
    pub udp_broadcast: bool,
    pub udp_port: String,
    pub iface_mode: IfaceMode,
    pub iface_names: Vec<String>,
    pub skip_prefixes: Vec<String>,
    pub udp_unicast: Vec<String>,
    pub tcp_unicast: Vec<String>,
    pub shared_secret: String,
    /// Every known metric plus any unknown enabled key, in display order.
    pub metrics: Vec<(String, bool)>,
    pub gpu_backend: GpuBackend,
    /// `metrics.gpu_adapter`: "auto", "#N" or part of a name.
    pub gpu_adapter: String,
    pub disk_include: Vec<String>,
    pub start_minimized: bool,
    pub show_splash: bool,
    pub autostart: bool,
    /// The config the form was made from; carries fields the form does not edit (`id`).
    base: Config,
}

/// Port used for unicast rows without one when the port field is not a valid number.
const FALLBACK_PORT: u16 = 48000;

/// One unicast target row: `ip`, `ip:port`, `[v6]:port`, `host` or `host:port`, a missing
/// port meaning `default_port`. Host names are not resolved here. Empty rows are allowed (and
/// dropped). Errors are short: they are shown under the row.
pub fn check_addr(row: &str, default_port: u16) -> Result<Option<TargetSpec>, String> {
    let t = row.trim();
    if t.is_empty() {
        return Ok(None);
    }
    parse_target_spec(t, default_port).map(Some)
}

/// The hint shown under a valid address row: where packets will actually go, when that is
/// not exactly what was typed.
pub fn addr_hint(row: &str, spec: &TargetSpec) -> Option<String> {
    let eff = spec.to_string();
    match spec {
        TargetSpec::Host(..) => Some(format!("sends to {eff} (name looked up when sending)")),
        TargetSpec::Addr(_) if eff != row.trim() => Some(format!("sends to {eff}")),
        TargetSpec::Addr(_) => None,
    }
}

/// One name row (interface name, prefix, disk). Empty rows are allowed (and dropped).
pub fn check_name(row: &str) -> Result<Option<String>, String> {
    let t = row.trim();
    if t.is_empty() {
        Ok(None)
    } else if t.chars().any(char::is_whitespace) {
        Err("no spaces allowed".into())
    } else {
        Ok(Some(t.to_string()))
    }
}

fn rows<T>(
    key: &str,
    list: &[String],
    check: impl Fn(&str) -> Result<Option<T>, String>,
    errs: &mut Vec<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for r in list {
        match check(r) {
            Ok(Some(_)) => out.push(r.trim().to_string()),
            Ok(None) => {}
            Err(e) => errs.push(format!("{key}: '{}': {e}", r.trim())),
        }
    }
    out
}

/// What to store when an adapter is picked from the list: its name if no other adapter's name
/// contains it (stable if the order changes), else `#N`.
pub fn adapter_pin(list: &[Adapter], a: &Adapter) -> String {
    let lower = a.name.to_ascii_lowercase();
    let first_match = list
        .iter()
        .find(|o| o.name.to_ascii_lowercase().contains(&lower));
    if first_match.is_some_and(|o| o.index == a.index) {
        a.name.clone()
    } else {
        format!("#{}", a.index)
    }
}

/// The adapter combo's label for the current `gpu_adapter` text.
pub fn adapter_choice_label(list: &[Adapter], pin: &str) -> String {
    if gpu_adapters::is_auto(pin) {
        return match gpu_adapters::auto_choice(list) {
            Some(a) => format!("Auto: {}", a.name),
            None => "Auto (most VRAM)".into(),
        };
    }
    match gpu_adapters::find(list, pin) {
        Some(a) => a.label(),
        None => format!("'{}' (not detected: auto is used)", pin.trim()),
    }
}

/// Human name and a one-line explanation for each GPU backend (config values unchanged).
pub fn gpu_label(b: GpuBackend) -> (&'static str, &'static str) {
    match b {
        GpuBackend::Auto => (
            "Auto (recommended)",
            "Uses the first backend that reports a value, and switches if it stops.",
        ),
        GpuBackend::Nvml => (
            "NVIDIA (NVML)",
            "NVIDIA driver library: load, VRAM, temperature, clock.",
        ),
        GpuBackend::Amdgpu => (
            "AMD (Linux sysfs)",
            "Reads gpu_busy_percent from the amdgpu driver.",
        ),
        GpuBackend::Intel => (
            "Intel (Linux sysfs, no utilisation)",
            "Only the clock frequency; the gauge stays empty.",
        ),
        GpuBackend::Pdh => (
            "Windows performance counters (any GPU)",
            "The 3D engine load that Task Manager shows; works with any vendor.",
        ),
        GpuBackend::None => ("Off", "Do not report a GPU."),
    }
}

impl Form {
    /// The form for the settings window: `from_config`, with "Start at login" showing whether
    /// the sender is really registered (an installer may have done it without touching
    /// `gui.autostart`).
    pub fn from_config_and_system(c: &Config) -> Self {
        let mut f = Self::from_config(c);
        f.autostart = super::autostart::is_enabled();
        f
    }

    /// The form for `c` as written (no system state); `to_config` gives `c` back.
    pub fn from_config(c: &Config) -> Self {
        let (iface_mode, iface_names) = match &c.network.broadcast_interfaces {
            BroadcastInterfaces::Named(v) => (IfaceMode::Named, v.clone()),
            BroadcastInterfaces::Mode(m) if m.eq_ignore_ascii_case("default") => {
                (IfaceMode::Default, vec![])
            }
            BroadcastInterfaces::Mode(_) => (IfaceMode::All, vec![]),
        };
        let mut metrics: Vec<(String, bool)> = Vec::new();
        for k in &c.metrics.enabled {
            let k = k.trim().to_ascii_lowercase();
            if !k.is_empty() && !metrics.iter().any(|(m, _)| *m == k) {
                metrics.push((k, true));
            }
        }
        for k in KNOWN_IDS {
            if !metrics.iter().any(|(m, _)| m == k) {
                metrics.push((k.to_string(), false));
            }
        }
        Self {
            host_name: c.general.host_name.clone(),
            interval_ms: c.general.interval_ms,
            udp_broadcast: c.network.udp_broadcast,
            udp_port: c.network.udp_port.to_string(),
            iface_mode,
            iface_names,
            skip_prefixes: c.network.skip_interface_prefixes.clone(),
            udp_unicast: c.network.udp_unicast.clone(),
            tcp_unicast: c.network.tcp_unicast.clone(),
            shared_secret: c.network.shared_secret.clone(),
            metrics,
            gpu_backend: c.metrics.gpu_backend.parse().unwrap_or_default(),
            gpu_adapter: c.metrics.gpu_adapter.clone(),
            disk_include: c.metrics.disk_include.clone(),
            start_minimized: c.gui.start_minimized,
            show_splash: c.gui.show_splash,
            autostart: c.gui.autostart,
            base: c.clone(),
        }
    }

    /// Port for unicast rows without one (`network.udp_port`, as typed so far).
    pub fn default_port(&self) -> u16 {
        self.udp_port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|&p| p > 0)
            .unwrap_or(FALLBACK_PORT)
    }

    /// Build and validate a `Config`; every problem found is returned.
    pub fn to_config(&self) -> Result<Config, Vec<String>> {
        let mut errs = Vec::new();
        let mut c = self.base.clone();
        c.general.host_name = self.host_name.trim().to_string();
        c.general.interval_ms = self.interval_ms;
        if self.interval_ms < MIN_INTERVAL_MS {
            errs.push(format!("interval must be at least {MIN_INTERVAL_MS} ms"));
        }
        c.network.udp_broadcast = self.udp_broadcast;
        match self.udp_port.trim().parse::<u16>() {
            Ok(p) if p > 0 => c.network.udp_port = p,
            _ => errs.push(format!("port '{}' must be 1-65535", self.udp_port.trim())),
        }
        c.network.broadcast_interfaces = match self.iface_mode {
            IfaceMode::All => BroadcastInterfaces::Mode("all".into()),
            IfaceMode::Default => BroadcastInterfaces::Mode("default".into()),
            IfaceMode::Named => {
                let names = rows("interfaces", &self.iface_names, check_name, &mut errs);
                if names.is_empty() && self.udp_broadcast {
                    errs.push("interfaces: name at least one interface, or pick All".into());
                }
                BroadcastInterfaces::Named(names)
            }
        };
        c.network.skip_interface_prefixes =
            rows("skip prefixes", &self.skip_prefixes, check_name, &mut errs);
        let port = self.default_port();
        let addr = |r: &str| check_addr(r, port);
        c.network.udp_unicast = rows("UDP unicast", &self.udp_unicast, addr, &mut errs);
        c.network.tcp_unicast = rows("TCP unicast", &self.tcp_unicast, addr, &mut errs);
        c.network.shared_secret = self.shared_secret.clone();
        c.metrics.enabled = self
            .metrics
            .iter()
            .filter(|(_, on)| *on)
            .map(|(k, _)| k.clone())
            .collect();
        c.metrics.gpu_backend = self.gpu_backend.as_str().to_string();
        c.metrics.gpu_adapter = match self.gpu_adapter.trim() {
            "" => "auto".to_string(),
            a => a.to_string(),
        };
        c.metrics.disk_include = rows("disks", &self.disk_include, check_name, &mut errs);
        c.gui.start_minimized = self.start_minimized;
        c.gui.show_splash = self.show_splash;
        c.gui.autostart = self.autostart;
        if errs.is_empty() {
            if let Err(e) = c.validate() {
                errs.push(e.to_string());
            }
        }
        if errs.is_empty() {
            Ok(c)
        } else {
            Err(errs)
        }
    }

    /// Swap metric `i` with its neighbour (`up` = towards the front).
    pub fn move_metric(&mut self, i: usize, up: bool) {
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        if let Some(j) = j.filter(|&j| j < self.metrics.len()) {
            self.metrics.swap(i, j);
        }
    }
}

/// Packets/s from the `sent` counter over a sliding window.
#[derive(Debug, Default)]
pub struct Rate {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl Rate {
    const WINDOW: Duration = Duration::from_secs(10);

    pub fn update(&mut self, now: Instant, sent: u64) -> Option<f64> {
        if self.samples.back().is_some_and(|&(_, s)| sent < s) {
            self.samples.clear(); // counter reset
        }
        self.samples.push_back((now, sent));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > Self::WINDOW {
            self.samples.pop_front();
        }
        let (t0, s0) = *self.samples.front()?;
        let dt = now.duration_since(t0).as_secs_f64();
        (dt >= 1.0).then(|| (sent - s0) as f64 / dt)
    }
}

// ---------------------------------------------------------------------------------------------
// Window

pub enum Outcome {
    /// The user closed the window; keep running in the tray.
    Closed,
    /// Quit was requested (tray menu, or the daemon exited) while the window was open.
    Quit,
}

/// Open the window and block until it is closed.
pub fn show(ctl: &Ctl, events: &Receiver<GuiEvent>) -> anyhow::Result<Outcome> {
    let quit = Rc::new(Cell::new(false));
    let (rgba, width, height) = icon::rgba(64, false);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rtm-sender settings")
            .with_app_id("rtm-sender")
            // Fits a 1366x768 laptop with a taskbar; everything scrolls below this.
            .with_inner_size([540.0, 620.0])
            .with_min_inner_size([400.0, 320.0])
            .with_icon(egui::IconData {
                rgba,
                width,
                height,
            }),
        renderer: eframe::Renderer::Glow,
        run_and_return: true,
        ..Default::default()
    };
    let app = SettingsApp::new(ctl.clone(), events.clone(), quit.clone());
    eframe::run_native(
        "rtm-sender",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(if quit.get() {
        Outcome::Quit
    } else {
        Outcome::Closed
    })
}

struct SettingsApp {
    ctl: Ctl,
    events: Receiver<GuiEvent>,
    quit: Rc<Cell<bool>>,
    form: Form,
    show_secret: bool,
    /// GPUs detected when the window opened.
    adapters: Vec<Adapter>,
    errors: Vec<String>,
    notice: Option<String>,
    rate: Rate,
    pps: Option<f64>,
}

impl SettingsApp {
    fn new(ctl: Ctl, events: Receiver<GuiEvent>, quit: Rc<Cell<bool>>) -> Self {
        let form = Form::from_config_and_system(&ctl.cfg.read());
        Self {
            ctl,
            events,
            quit,
            form,
            show_secret: false,
            adapters: gpu_adapters::list_adapters(),
            errors: vec![],
            notice: None,
            rate: Rate::default(),
            pps: None,
        }
    }

    fn save(&mut self) {
        self.notice = None;
        match self.form.to_config() {
            Err(errs) => self.errors = errs,
            Ok(cfg) => match self.ctl.save(cfg) {
                Ok(()) => {
                    self.errors.clear();
                    self.form = Form::from_config_and_system(&self.ctl.cfg.read());
                    self.notice = Some("Saved and applied.".into());
                }
                Err(e) => self.errors = vec![format!("{e:#}")],
            },
        }
    }

    fn revert(&mut self) {
        self.form = Form::from_config_and_system(&self.ctl.cfg.read());
        self.errors.clear();
        self.notice = Some("Reverted to the saved settings.".into());
    }
}

impl eframe::App for SettingsApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                GuiEvent::OpenSettings => ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                GuiEvent::Quit | GuiEvent::DaemonExited => {
                    self.quit.set(true);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let sent = self.ctl.status.read().sent;
        self.pps = self.rate.update(Instant::now(), sent);
        // Poll the event channel and status a few times per second.
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let status = self.ctl.status.read().clone();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            buttons(self, ui);
            ui.separator();
            status_panel(ui, &status, self.pps);
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                form_ui(ui, &mut self.form, &mut self.show_secret, &self.adapters);
            });
        });
    }
}

fn buttons(app: &mut SettingsApp, ui: &mut egui::Ui) {
    if !app.errors.is_empty() {
        let err = ui.visuals().error_fg_color;
        egui::ScrollArea::vertical()
            .id_salt("errors")
            .max_height(90.0)
            .show(ui, |ui| {
                for e in &app.errors {
                    ui.add(egui::Label::new(egui::RichText::new(e).small().color(err)).wrap());
                }
            });
    }
    if let Some(n) = &app.notice {
        ui.label(n);
    }
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            app.save();
        }
        if ui.button("Revert").clicked() {
            app.revert();
        }
        if ui.button("Close").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let path = app.ctl.path.display().to_string();
        ui.add(egui::Label::new(egui::RichText::new(&path).weak()).truncate())
            .on_hover_text(path);
    });
}

fn status_panel(ui: &mut egui::Ui, s: &Status, pps: Option<f64>) {
    // Not a Grid: grid cells do not wrap, and the target list can be long.
    let row = |ui: &mut egui::Ui, k: &str, v: egui::RichText| {
        ui.horizontal_top(|ui| {
            let w = ui.label(egui::RichText::new(k).weak()).rect.width();
            ui.add_space((80.0 - w).max(0.0));
            ui.add(egui::Label::new(v).wrap());
        });
    };
    row(ui, "Sending to", s.targets.clone().into());
    let state = if !s.running {
        "starting".to_string()
    } else if s.paused {
        "paused".to_string()
    } else {
        format!("{} every {}", s.host, interval_label(s.interval_ms))
    };
    row(ui, "State", state.into());
    row(
        ui,
        "Last packet",
        s.last_bytes
            .map_or("-".into(), |b| format!("{b} bytes, seq {}", s.seq))
            .into(),
    );
    row(
        ui,
        "Rate",
        pps.map_or("-".into(), |p| {
            format!("{p:.2} packets/s ({} sent)", s.sent)
        })
        .into(),
    );
    if let Some(e) = &s.last_error {
        row(
            ui,
            "Last error",
            egui::RichText::new(e).color(ui.visuals().error_fg_color),
        );
    }
}

fn form_ui(ui: &mut egui::Ui, f: &mut Form, show_secret: &mut bool, adapters: &[Adapter]) {
    ui.heading("General");
    egui::Grid::new("general").num_columns(2).show(ui, |ui| {
        ui.label("Host name");
        ui.add(egui::TextEdit::singleline(&mut f.host_name).hint_text("empty = system hostname"));
        ui.end_row();
        ui.label("Interval");
        ui.horizontal_wrapped(|ui| {
            for ms in INTERVALS_MS {
                ui.radio_value(&mut f.interval_ms, ms, interval_label(ms));
            }
            if !INTERVALS_MS.contains(&f.interval_ms) {
                let _ = ui.radio(
                    true,
                    format!("{} (from file)", interval_label(f.interval_ms)),
                );
            }
        });
        ui.end_row();
    });

    ui.separator();
    ui.heading("Network");
    ui.checkbox(&mut f.udp_broadcast, "UDP broadcast");
    ui.add_enabled_ui(f.udp_broadcast, |ui| {
        ui.horizontal(|ui| {
            ui.label("Broadcast interfaces");
            ui.radio_value(&mut f.iface_mode, IfaceMode::All, "All");
            ui.radio_value(&mut f.iface_mode, IfaceMode::Default, "Default route");
            ui.radio_value(&mut f.iface_mode, IfaceMode::Named, "Named");
        });
        if f.iface_mode == IfaceMode::Named {
            list_editor(
                ui,
                "ifaces",
                "Interface names",
                "eth0",
                &mut f.iface_names,
                no_hint(check_name),
            );
        }
        list_editor(
            ui,
            "skip",
            "Skip interfaces starting with",
            "docker",
            &mut f.skip_prefixes,
            no_hint(check_name),
        );
    });
    ui.horizontal(|ui| {
        ui.label("UDP broadcast port");
        let ok = f.udp_port.trim().parse::<u16>().is_ok_and(|p| p > 0);
        ui.add(egui::TextEdit::singleline(&mut f.udp_port).desired_width(70.0));
        if !ok {
            ui.colored_label(ui.visuals().error_fg_color, "1-65535");
        }
    });
    let port = f.default_port();
    let addr = |r: &str| check_addr(r, port).map(|s| s.and_then(|s| addr_hint(r, &s)));
    ui.weak(format!(
        "Unicast targets: IP or host name, optional :port (default {port})."
    ));
    list_editor(
        ui,
        "udpu",
        "UDP unicast targets",
        "192.168.1.50",
        &mut f.udp_unicast,
        addr,
    );
    list_editor(
        ui,
        "tcpu",
        "TCP unicast targets",
        "192.168.1.50:48000",
        &mut f.tcp_unicast,
        addr,
    );
    ui.horizontal(|ui| {
        ui.label("Shared secret");
        ui.add(
            egui::TextEdit::singleline(&mut f.shared_secret)
                .password(!*show_secret)
                .hint_text("empty = no auth"),
        );
        ui.checkbox(show_secret, "Show");
    });

    ui.separator();
    ui.heading("Metrics");
    ui.weak("Order is the order in the packet (a display hint for the viewer).");
    let n = f.metrics.len();
    let mut mv = None;
    egui::Grid::new("metrics").num_columns(3).show(ui, |ui| {
        for (i, (key, on)) in f.metrics.iter_mut().enumerate() {
            ui.checkbox(on, key.as_str());
            if ui
                .add_enabled(i > 0, egui::Button::new("Up").small())
                .clicked()
            {
                mv = Some((i, true));
            }
            if ui
                .add_enabled(i + 1 < n, egui::Button::new("Down").small())
                .clicked()
            {
                mv = Some((i, false));
            }
            ui.end_row();
        }
    });
    if let Some((i, up)) = mv {
        f.move_metric(i, up);
    }
    ui.horizontal(|ui| {
        ui.label("GPU backend");
        egui::ComboBox::from_id_salt("gpu_backend")
            .selected_text(gpu_label(f.gpu_backend).0)
            .width(260.0)
            .show_ui(ui, |ui| {
                for b in GPU_BACKENDS
                    .iter()
                    .filter_map(|s| s.parse::<GpuBackend>().ok())
                {
                    let (label, hint) = gpu_label(b);
                    ui.selectable_value(&mut f.gpu_backend, b, label)
                        .on_hover_text(hint);
                }
            });
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(gpu_label(f.gpu_backend).1)
                .small()
                .weak(),
        )
        .wrap(),
    );
    gpu_adapter_ui(ui, f, adapters);
    list_editor(
        ui,
        "disks",
        "Disks to include (empty = all physical disks)",
        "nvme0n1",
        &mut f.disk_include,
        no_hint(check_name),
    );

    ui.separator();
    ui.heading("Startup");
    ui.checkbox(
        &mut f.start_minimized,
        "Start minimized (tray only, no settings window)",
    );
    ui.add_enabled_ui(f.start_minimized, |ui| {
        ui.checkbox(&mut f.show_splash, "Show a splash on start")
            .on_hover_text("A small note for 3 s pointing at the tray icon.");
    });
    ui.checkbox(&mut f.autostart, "Start at login");
    ui.add_space(8.0);
}

/// GPU adapter: a combo of the detected adapters ("Auto" first, the reported one marked),
/// plus a text field for a name substring or `#N`.
fn gpu_adapter_ui(ui: &mut egui::Ui, f: &mut Form, adapters: &[Adapter]) {
    let reported = gpu_adapters::current_adapter();
    let is_reported = |a: &Adapter| {
        reported
            .as_ref()
            .is_some_and(|(i, n)| *i == Some(a.index) || (i.is_none() && *n == a.name))
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("GPU adapter");
        egui::ComboBox::from_id_salt("gpu_adapter")
            .selected_text(adapter_choice_label(adapters, &f.gpu_adapter))
            .width(300.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(gpu_adapters::is_auto(&f.gpu_adapter), "Auto (most VRAM)")
                    .clicked()
                {
                    f.gpu_adapter = "auto".into();
                }
                for a in adapters {
                    let mut label = a.label();
                    if is_reported(a) {
                        label.push_str(" (reported now)");
                    }
                    let pin = adapter_pin(adapters, a);
                    let selected = gpu_adapters::find(adapters, &f.gpu_adapter)
                        .is_some_and(|s| s.index == a.index);
                    if ui.selectable_label(selected, label).clicked() {
                        f.gpu_adapter = pin;
                    }
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut f.gpu_adapter)
                .hint_text("auto, #1 or part of a name")
                .desired_width(140.0),
        );
    });
    let note = match &reported {
        Some((_, name)) => format!("Reporting now: {name}. Save to apply a change."),
        None if adapters.is_empty() => "No GPUs detected.".to_string(),
        None => format!("{} GPU(s) detected.", adapters.len()),
    };
    ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
}

/// Adapt a row check to `list_editor` (no hint for valid rows).
fn no_hint<T>(
    check: fn(&str) -> Result<Option<T>, String>,
) -> impl Fn(&str) -> Result<Option<String>, String> {
    move |r| check(r).map(|_| None)
}

/// Rows of single-line text with a remove button each, an Add button, and per-row feedback
/// under the row: a short wrapped error (full text on the "!" tooltip), or `check`'s hint for a
/// valid row. Long lists scroll.
fn list_editor(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    hint: &str,
    list: &mut Vec<String>,
    check: impl Fn(&str) -> Result<Option<String>, String>,
) {
    ui.label(title);
    ui.indent(id, |ui| {
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, row) in list.iter_mut().enumerate() {
                    let res = check(row);
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 90.0).clamp(120.0, 260.0);
                        ui.add(
                            egui::TextEdit::singleline(row)
                                .hint_text(hint)
                                .desired_width(w),
                        );
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        if let Err(e) = &res {
                            // egui's built-in fonts have no ⚠ glyph.
                            ui.colored_label(ui.visuals().error_fg_color, "!")
                                .on_hover_text(format!("'{}': {e}", row.trim()));
                        }
                    });
                    let note = match &res {
                        Err(e) => Some(
                            egui::RichText::new(e.as_str())
                                .small()
                                .color(ui.visuals().error_fg_color),
                        ),
                        Ok(Some(h)) => Some(egui::RichText::new(h.as_str()).small().weak()),
                        Ok(None) => None,
                    };
                    if let Some(note) = note {
                        ui.add(egui::Label::new(note).wrap());
                    }
                }
            });
        if let Some(i) = remove {
            list.remove(i);
        }
        if ui.small_button("+ Add").clicked() {
            list.push(String::new());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> Config {
        let mut c = Config::default();
        c.general.id = "fixed-id".into();
        c.general.host_name = "desk".into();
        c.general.interval_ms = 5000;
        c.network.udp_broadcast = false;
        c.network.udp_port = 48123;
        c.network.broadcast_interfaces = BroadcastInterfaces::Named(vec!["eth0".into()]);
        c.network.skip_interface_prefixes = vec!["docker".into()];
        c.network.udp_unicast = vec!["192.168.1.50:48000".into()];
        c.network.tcp_unicast = vec!["10.0.0.2:48001".into(), "[::1]:48000".into()];
        c.network.shared_secret = "s3cret".into();
        c.metrics.enabled = vec!["mem".into(), "cpu".into(), "uptime".into()];
        c.metrics.gpu_backend = "nvml".into();
        c.metrics.disk_include = vec!["sda".into()];
        c.gui.start_minimized = false;
        c.gui.show_splash = false;
        c.gui.autostart = true;
        c
    }

    #[test]
    fn round_trip_default_and_custom() {
        let mut d = Config::default();
        d.general.id = "x".into();
        assert_eq!(Form::from_config(&d).to_config().unwrap(), d);
        let c = custom();
        let f = Form::from_config(&c);
        assert_eq!(f.iface_mode, IfaceMode::Named);
        assert_eq!(f.gpu_backend, GpuBackend::Nvml);
        assert_eq!(f.to_config().unwrap(), c);
    }

    #[test]
    fn metrics_list_has_enabled_first_then_known() {
        let f = Form::from_config(&custom());
        let keys: Vec<&str> = f.metrics.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            ["mem", "cpu", "uptime", "gpu", "disk", "net", "temp", "load"]
        );
        assert_eq!(f.metrics.iter().filter(|(_, on)| *on).count(), 3);
    }

    #[test]
    fn edits_are_applied() {
        let mut f = Form::from_config(&custom());
        f.move_metric(2, true); // uptime above cpu
        f.move_metric(0, true); // no-op at the top
        f.move_metric(7, false); // no-op at the bottom
        f.metrics[3].1 = true; // enable gpu
        f.iface_mode = IfaceMode::Default;
        f.udp_unicast.push("   ".into()); // empty rows are dropped
        f.tcp_unicast.remove(0);
        f.gpu_backend = GpuBackend::None;
        let c = f.to_config().unwrap();
        assert_eq!(c.metrics.enabled, ["mem", "uptime", "cpu", "gpu"]);
        assert_eq!(
            c.network.broadcast_interfaces,
            BroadcastInterfaces::Mode("default".into())
        );
        assert_eq!(c.network.udp_unicast, ["192.168.1.50:48000"]);
        assert_eq!(c.network.tcp_unicast, ["[::1]:48000"]);
        assert_eq!(c.metrics.gpu_backend, "none");
        assert_eq!(c.general.id, "fixed-id", "untouched fields survive");
    }

    #[test]
    fn invalid_rows_are_reported() {
        let mut f = Form::from_config(&custom());
        f.udp_unicast.push("192.168.1.300".into());
        f.tcp_unicast.push("host:port".into());
        f.udp_port = "0".into();
        f.disk_include.push("two words".into());
        let errs = f.to_config().unwrap_err();
        assert_eq!(errs.len(), 4, "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("not a valid IP")));
        assert!(errs.iter().any(|e| e.starts_with("TCP unicast")));

        let mut f = Form::from_config(&custom());
        f.udp_broadcast = true;
        f.iface_names = vec!["".into()];
        assert!(f.to_config().unwrap_err()[0].contains("at least one interface"));
    }

    #[test]
    fn addr_and_name_checks() {
        let addr = |s: &str| TargetSpec::Addr(s.parse().unwrap());
        assert_eq!(check_addr("", 48000), Ok(None));
        assert_eq!(
            check_addr(" 192.168.1.50:48000 ", 1),
            Ok(Some(addr("192.168.1.50:48000")))
        );
        assert!(check_addr("[fe80::1]:48000", 1).unwrap().is_some());
        // No port: the broadcast port is used, and the hint says so.
        let s = check_addr("192.168.1.60", 48000).unwrap().unwrap();
        assert_eq!(s, addr("192.168.1.60:48000"));
        assert_eq!(
            addr_hint("192.168.1.60", &s).as_deref(),
            Some("sends to 192.168.1.60:48000")
        );
        assert_eq!(addr_hint("192.168.1.60:48000", &s), None);
        let h = check_addr("phone.local", 48001).unwrap().unwrap();
        assert_eq!(h, TargetSpec::Host("phone.local".into(), 48001));
        assert!(addr_hint("phone.local", &h)
            .unwrap()
            .contains("phone.local:48001"));
        // Errors are short enough for a row.
        for bad in ["192.168.1.50:99999", "host:port", "a b", "1.2.3.4.5"] {
            let e = check_addr(bad, 48000).unwrap_err();
            assert!(e.len() <= 40, "{e}");
        }
        assert_eq!(check_name(" eth0 "), Ok(Some("eth0".into())));
        assert_eq!(check_name(""), Ok(None));
        assert!(check_name("a b").is_err());
    }

    #[test]
    fn bare_ip_row_saves_as_typed_and_follows_the_port_field() {
        let mut f = Form::from_config(&custom());
        f.udp_unicast = vec!["192.168.1.60".into()];
        f.udp_port = "48123".into();
        assert_eq!(f.default_port(), 48123);
        let c = f.to_config().unwrap();
        assert_eq!(c.network.udp_unicast, ["192.168.1.60"]);
        let t = c.to_targets().unwrap();
        assert!(t.contains(&rtm_net::Target::UdpUnicast(
            "192.168.1.60:48123".parse().unwrap()
        )));
        f.udp_port = "x".into();
        assert_eq!(f.default_port(), 48000);
    }

    #[test]
    fn gpu_adapter_choices() {
        let a = |i: usize, name: &str, mb: u64| Adapter {
            index: i,
            name: name.into(),
            id: format!("card{i}"),
            vendor: Some(0x1002),
            vram_total: Some(mb << 20),
            integrated: mb < 2048,
        };
        let list = vec![
            a(0, "AMD Radeon(TM) Graphics", 512),
            a(1, "AMD Radeon AI PRO R9700", 32768),
            a(2, "AMD Radeon AI PRO R9700 B", 32768),
        ];
        assert_eq!(adapter_pin(&list, &list[0]), "AMD Radeon(TM) Graphics");
        assert_eq!(adapter_pin(&list, &list[1]), "AMD Radeon AI PRO R9700");
        // "...R9700" is contained in "...R9700 B" too, but find() takes the first match, #1.
        assert_eq!(adapter_pin(&list, &list[2]), "AMD Radeon AI PRO R9700 B");
        let dup = vec![a(0, "Radeon", 512), a(1, "Radeon", 512)];
        assert_eq!(adapter_pin(&dup, &dup[1]), "#1", "same name: use the index");
        assert_eq!(
            adapter_choice_label(&list, "auto"),
            "Auto: AMD Radeon AI PRO R9700"
        );
        assert_eq!(
            adapter_choice_label(&list, "#0"),
            "#0 AMD Radeon(TM) Graphics (512 MB)"
        );
        assert!(adapter_choice_label(&list, "geforce").contains("not detected"));
        let mut f = Form::from_config(&custom());
        assert_eq!(f.gpu_adapter, "auto");
        f.gpu_adapter = " ".into();
        assert_eq!(f.to_config().unwrap().metrics.gpu_adapter, "auto");
        f.gpu_adapter = "R9700".into();
        assert_eq!(f.to_config().unwrap().metrics.gpu_adapter, "R9700");
    }

    #[test]
    fn gpu_labels_are_human_and_distinct() {
        let all: Vec<GpuBackend> = GPU_BACKENDS.iter().map(|s| s.parse().unwrap()).collect();
        let labels: Vec<&str> = all.iter().map(|&b| gpu_label(b).0).collect();
        assert_eq!(
            labels,
            [
                "Auto (recommended)",
                "NVIDIA (NVML)",
                "AMD (Linux sysfs)",
                "Intel (Linux sysfs, no utilisation)",
                "Windows performance counters (any GPU)",
                "Off"
            ]
        );
        // Config values are unchanged.
        assert_eq!(GpuBackend::Pdh.as_str(), "pdh");
    }

    #[test]
    fn packet_rate() {
        let mut r = Rate::default();
        let t = Instant::now();
        assert_eq!(r.update(t, 0), None);
        assert_eq!(r.update(t + Duration::from_millis(500), 1), None);
        let p = r.update(t + Duration::from_secs(4), 4).unwrap();
        assert!((p - 1.0).abs() < 1e-9);
        // Sliding window drops old samples.
        let p = r.update(t + Duration::from_secs(30), 4).unwrap();
        assert!(p.abs() < 1e-9);
        // Counter reset (daemon restarted) does not underflow.
        assert_eq!(r.update(t + Duration::from_secs(31), 0), None);
    }
}
