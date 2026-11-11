//! rtm-sender: samples system stats and sends them to RTM viewers.
//! Headless by default; the `gui` feature adds a tray icon and a settings window.

// GUI builds on Windows do not open a console window; see gui::console for `--once` etc.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

mod app;
mod cli;
#[cfg(feature = "gui")]
mod gui;
mod instance;
mod logging;
mod reload;
mod status;

use anyhow::Context;
use clap::Parser;
use rtm_config::Config;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};

fn main() -> ExitCode {
    #[cfg(all(windows, feature = "gui"))]
    gui::console::attach_if_printing();
    let cli = match cli::Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Windows GUI build: make usage errors visible (parent console or a message box).
            #[cfg(all(windows, feature = "gui"))]
            gui::console::report_usage_error(&e);
            e.exit();
        }
    };
    let path = cli.config_path();
    let log_file = log_file(&cli, &path);
    logging::init(cli.log_level(), log_file.as_deref());
    match real_main(&cli, &path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            #[cfg(all(windows, feature = "gui"))]
            if let Some(log) = &log_file {
                gui::console::report_fatal(&e, log);
            }
            if e.downcast_ref::<instance::AlreadyRunning>().is_some() {
                ExitCode::from(instance::EXIT_ALREADY_RUNNING)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

/// GUI mode (the `gui` build, except the one-shot `--once` / `--print-config`) also logs to
/// `sender.log` next to the config.
fn log_file(cli: &cli::Cli, config: &Path) -> Option<PathBuf> {
    let gui_mode = cfg!(feature = "gui") && !cli.once && !cli.print_config;
    gui_mode.then(|| logging::log_path(config))
}

/// File config + CLI overrides, validated.
fn effective_config(cli: &cli::Cli, mut cfg: Config) -> anyhow::Result<Config> {
    cli.apply_overrides(&mut cfg);
    cfg.validate()?;
    Ok(cfg)
}

fn real_main(cli: &cli::Cli, path: &Path) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    // Taken before the config is loaded (loading may write it). Not for the one-shot modes.
    let _instance = if cli.once || cli.print_config {
        None
    } else {
        Some(instance::acquire(&path)?)
    };
    let file_cfg = Config::load_or_create(&path)
        .with_context(|| format!("loading config {}", path.display()))?;

    if cli.print_config {
        let mut cfg = file_cfg;
        cli.apply_overrides(&mut cfg);
        println!("# {}", path.display());
        print!("{}", cfg.to_toml_string()?);
        cfg.validate()?;
        return Ok(());
    }

    #[cfg(feature = "gui")]
    let shared_cfg: gui::SharedConfig =
        std::sync::Arc::new(parking_lot::RwLock::new(file_cfg.clone()));
    let cfg =
        effective_config(cli, file_cfg).with_context(|| format!("config {}", path.display()))?;

    if cli.once {
        println!("{}", app::run_once(&cfg)?);
        return Ok(());
    }

    let (tx, rx) = crossbeam_channel::unbounded();
    {
        let tx = tx.clone();
        let presses = AtomicU32::new(0);
        ctrlc::set_handler(move || {
            if presses.fetch_add(1, Ordering::SeqCst) > 0 {
                std::process::exit(130); // second Ctrl-C: give up waiting
            }
            let _ = tx.send(app::Control::Shutdown);
        })
        .context("installing Ctrl-C handler")?;
    }

    let watch_cli = cli.clone();
    let watch_path = path.clone();
    #[cfg(feature = "gui")]
    let watch_shared = shared_cfg.clone();
    let _watcher = reload::watch_into(&path, tx.clone(), move || {
        if !watch_path.exists() {
            tracing::warn!("config file removed; keeping current config");
            return None;
        }
        // load_or_create so that an edited `id = "auto"` is resolved and persisted again.
        let loaded = Config::load_or_create(&watch_path).map_err(anyhow::Error::from);
        #[cfg(feature = "gui")]
        if let Some(c) = loaded.as_ref().ok().filter(|c| c.validate().is_ok()) {
            *watch_shared.write() = c.clone(); // what the settings window shows on open/revert
        }
        match loaded.and_then(|c| effective_config(&watch_cli, c)) {
            Ok(c) => Some(app::Control::Reload(Box::new(c))),
            Err(e) => {
                tracing::error!("config reload rejected, keeping current config: {e:#}");
                None
            }
        }
    })
    .map_err(|e| tracing::warn!("config hot reload disabled: {e}"))
    .ok();

    #[cfg(feature = "gui")]
    if !cli.no_gui {
        match gui::unavailable_reason() {
            Some(why) => tracing::warn!("GUI unavailable ({why}); running headless"),
            None => {
                let eff_cli = cli.clone();
                return gui::run(gui::Launch {
                    cfg,
                    shared_cfg,
                    path,
                    control_tx: tx,
                    control_rx: rx,
                    effective: std::sync::Arc::new(move |c| effective_config(&eff_cli, c)),
                });
            }
        }
    }

    let res = app::run(cfg, rx);
    drop(tx);
    res
}
