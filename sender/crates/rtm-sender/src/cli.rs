//! Command line interface. Flags override the config file for one run and are
//! re-applied on every hot reload.

use clap::{ArgAction, Parser};
use rtm_config::Config;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, Default, PartialEq)]
#[command(
    name = "rtm-sender",
    version,
    about = "Samples system stats and sends them to RTM viewers on the LAN"
)]
pub struct Cli {
    /// Config file (default: OS config dir, e.g. ~/.config/rtm/sender.toml). Created if missing.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Sample interval: milliseconds, or with a suffix ("500ms", "5s"). Minimum 100 ms.
    #[arg(long, value_name = "MS", value_parser = parse_interval)]
    pub interval: Option<u64>,

    /// Send UDP datagrams to this ip:port (repeatable). Replaces the file's list.
    #[arg(long = "udp-unicast", value_name = "IP:PORT")]
    pub udp_unicast: Vec<SocketAddr>,

    /// Stream newline-delimited JSON over TCP to this ip:port (repeatable). Replaces the file's list.
    #[arg(long = "tcp-unicast", value_name = "IP:PORT")]
    pub tcp_unicast: Vec<SocketAddr>,

    /// Disable UDP broadcast.
    #[arg(long)]
    pub no_broadcast: bool,

    /// Print the effective configuration (file + flags) as TOML and exit. The shared secret
    /// is shown as "********" unless --show-secret is given too.
    #[arg(long)]
    pub print_config: bool,

    /// With --print-config: print the shared secret itself.
    #[arg(long, requires = "print_config")]
    pub show_secret: bool,

    /// Sample once (two reads 200 ms apart), print the JSON packet to stdout, exit without sending.
    #[arg(long)]
    pub once: bool,

    /// Do not start the tray icon / settings window; run like the headless build.
    /// Accepted (and ignored) by builds without the `gui` feature.
    #[arg(long)]
    pub no_gui: bool,

    /// More logging (-v debug, -vv trace). RTM_LOG overrides, e.g. RTM_LOG=rtm_net=trace.
    #[arg(short, long, action = ArgAction::Count)]
    pub verbose: u8,
}

impl Cli {
    pub fn config_path(&self) -> PathBuf {
        self.config.clone().unwrap_or_else(Config::default_path)
    }

    /// Apply the one-run overrides on top of a config loaded from file.
    pub fn apply_overrides(&self, cfg: &mut Config) {
        if let Some(ms) = self.interval {
            cfg.general.interval_ms = ms;
        }
        if !self.udp_unicast.is_empty() {
            cfg.network.udp_unicast = self.udp_unicast.iter().map(|a| a.to_string()).collect();
        }
        if !self.tcp_unicast.is_empty() {
            cfg.network.tcp_unicast = self.tcp_unicast.iter().map(|a| a.to_string()).collect();
        }
        if self.no_broadcast {
            cfg.network.udp_broadcast = false;
        }
    }

    /// Default tracing filter when RTM_LOG is unset.
    pub fn log_level(&self) -> &'static str {
        match self.verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    }
}

/// The `--print-config` output: a `# <path>` line, then the TOML, with a non-empty
/// `shared_secret` masked unless `show_secret`.
pub fn render_config(
    path: &std::path::Path,
    cfg: &Config,
    show_secret: bool,
) -> anyhow::Result<String> {
    let text = if show_secret {
        cfg.to_toml_string()?
    } else {
        cfg.redacted().to_toml_string()?
    };
    Ok(format!("# {}\n{text}", path.display()))
}

/// "1000" | "1000ms" | "5s" -> milliseconds.
pub fn parse_interval(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1000)
    } else {
        (s, 1)
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("'{s}' is not an interval (try 1000, 500ms or 5s)"))?;
    n.checked_mul(mult)
        .ok_or_else(|| format!("'{s}' is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("rtm-sender").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn no_args_is_default() {
        assert_eq!(parse(&[]), Cli::default());
    }

    #[test]
    fn all_flags() {
        let c = parse(&[
            "--config",
            "/tmp/x.toml",
            "--interval",
            "5s",
            "--udp-unicast",
            "192.168.1.50:48000",
            "--udp-unicast",
            "192.168.1.51:48000",
            "--tcp-unicast",
            "10.0.0.2:48001",
            "--no-broadcast",
            "--print-config",
            "--show-secret",
            "--once",
            "--no-gui",
            "-vv",
        ]);
        assert_eq!(c.config_path(), PathBuf::from("/tmp/x.toml"));
        assert_eq!(c.interval, Some(5000));
        assert_eq!(c.udp_unicast.len(), 2);
        assert_eq!(c.tcp_unicast, vec!["10.0.0.2:48001".parse().unwrap()]);
        assert!(c.no_broadcast && c.print_config && c.show_secret && c.once && c.no_gui);
        assert_eq!(c.verbose, 2);
        assert_eq!(c.log_level(), "trace");
    }

    #[test]
    fn bad_values_rejected() {
        let p = |a: &[&str]| {
            Cli::try_parse_from(std::iter::once("rtm-sender").chain(a.iter().copied())).is_err()
        };
        assert!(p(&["--interval", "fast"]));
        assert!(p(&["--udp-unicast", "nope"]));
        assert!(p(&["--tcp-unicast", "1.2.3.4"]));
        assert!(p(&["--bogus"]));
        assert!(p(&["--show-secret"]), "only together with --print-config");
    }

    #[test]
    fn print_config_masks_the_secret_unless_asked() {
        let mut cfg = Config::default();
        cfg.network.shared_secret = "hunter2".into();
        let path = std::path::Path::new("x.toml");
        let masked = render_config(path, &cfg, false).unwrap();
        assert!(masked.starts_with("# x.toml\n"), "{masked}");
        assert!(masked.contains("shared_secret = \"********\""), "{masked}");
        assert!(!masked.contains("hunter2"));
        let shown = render_config(path, &cfg, true).unwrap();
        assert!(shown.contains("shared_secret = \"hunter2\""), "{shown}");
        // Both parse back; the shown one is the config itself.
        let body = |s: &str| s.split_once('\n').unwrap().1.to_string();
        assert_eq!(Config::from_toml(&body(&shown)).unwrap(), cfg);
        cfg.network.shared_secret.clear();
        assert!(render_config(path, &cfg, false)
            .unwrap()
            .contains("shared_secret = \"\""));
    }

    #[test]
    fn interval_parsing() {
        assert_eq!(parse_interval("1000"), Ok(1000));
        assert_eq!(parse_interval("250ms"), Ok(250));
        assert_eq!(parse_interval("15s"), Ok(15000));
        assert!(parse_interval("").is_err());
        assert!(parse_interval("-1").is_err());
        assert!(parse_interval("1m").is_err());
    }

    #[test]
    fn overrides_apply() {
        let c = parse(&[
            "--interval",
            "30000",
            "--udp-unicast",
            "192.168.1.50:48000",
            "--no-broadcast",
        ]);
        let mut cfg = Config::default();
        cfg.network.tcp_unicast = vec!["10.0.0.1:48000".into()];
        c.apply_overrides(&mut cfg);
        assert_eq!(cfg.general.interval_ms, 30000);
        assert_eq!(cfg.network.udp_unicast, vec!["192.168.1.50:48000"]);
        assert_eq!(
            cfg.network.tcp_unicast,
            vec!["10.0.0.1:48000"],
            "kept when flag absent"
        );
        assert!(!cfg.network.udp_broadcast);
        cfg.validate().unwrap();

        let mut cfg = Config::default();
        Cli::default().apply_overrides(&mut cfg);
        assert_eq!(cfg, Config::default());
    }
}
