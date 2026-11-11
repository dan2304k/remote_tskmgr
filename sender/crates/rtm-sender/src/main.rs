//! rtm-sender: samples system stats and sends them to RTM viewers.
//! Headless by default; the `gui` feature adds a tray icon and a settings window.

// GUI builds on Windows do not open a console window; see gui::console for `--once` etc.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

mod app;
mod cli;
#[cfg(feature = "gui")]
mod gui;
mod reload;
mod status;

use anyhow::Context;
use clap::Parser;
use rtm_config::Config;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    #[cfg(all(windows, feature = "gui"))]
    gui::console::attach_if_printing();
    let cli = cli::Cli::parse();
    let filter =
        EnvFilter::try_from_env("RTM_LOG").unwrap_or_else(|_| EnvFilter::new(cli.log_level()));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        // No colour codes in the systemd journal / redirected logs.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    match real_main(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// File config + CLI overrides, validated.
fn effective_config(cli: &cli::Cli, mut cfg: Config) -> anyhow::Result<Config> {
    cli.apply_overrides(&mut cfg);
    cfg.validate()?;
    Ok(cfg)
}

fn real_main(cli: &cli::Cli) -> anyhow::Result<()> {
    let path = cli.config_path();
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
