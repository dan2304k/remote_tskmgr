//! Logging: always to stderr. In GUI mode (the `gui` build, except `--once` and
//! `--print-config`) also to `sender.log` next to the config file, because a Windows GUI
//! build has no visible stderr and a tray app started at login has no terminal.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

pub const LOG_FILE: &str = "sender.log";
/// A log file bigger than this at startup is truncated (the only rotation there is).
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// `sender.log` in the directory of `config`.
pub fn log_path(config: &Path) -> PathBuf {
    config.parent().unwrap_or(Path::new("")).join(LOG_FILE)
}

/// Open `path` for appending, creating it and its directory. A file over `max` bytes is
/// truncated first. On Unix a new file gets mode 0600 and a new directory 0700.
pub fn open_log(path: &Path, max: u64) -> io::Result<File> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        rtm_config::create_private_dir(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max) {
        File::create(path)?;
    }
    rtm_config::private_open_options()
        .create(true)
        .append(true)
        .open(path)
}

/// Install the global subscriber: `RTM_LOG` or `default_level`, to stderr and, if given, to
/// the log file (no colour codes there). A log file that cannot be opened is reported on
/// stderr and otherwise ignored.
pub fn init(default_level: &str, file: Option<&Path>) {
    let filter =
        EnvFilter::try_from_env("RTM_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));
    let stderr = fmt::layer()
        .with_writer(io::stderr)
        // No colour codes in the systemd journal / redirected logs.
        .with_ansi(io::IsTerminal::is_terminal(&io::stderr()));
    let mut file_err = None;
    let file_layer = file.and_then(|p| match open_log(p, MAX_LOG_BYTES) {
        Ok(f) => Some(fmt::layer().with_writer(Arc::new(f)).with_ansi(false)),
        Err(e) => {
            file_err = Some(format!("cannot write log file {}: {e}", p.display()));
            None
        }
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file_layer)
        .init();
    if let Some(e) = file_err {
        tracing::warn!("{e}");
    } else if let Some(p) = file {
        tracing::debug!(path = %p.display(), "logging to file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn log_lives_next_to_the_config() {
        let dir = Path::new("some").join("dir");
        assert_eq!(log_path(&dir.join("sender.toml")), dir.join("sender.log"));
        assert_eq!(log_path(&dir.join("other.toml")), dir.join("sender.log"));
        assert_eq!(
            log_path(Path::new("sender.toml")),
            PathBuf::from("sender.log")
        );
    }

    #[test]
    fn appends_and_truncates_when_too_big() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("new").join(LOG_FILE);
        open_log(&p, 100).unwrap().write_all(b"first\n").unwrap();
        open_log(&p, 100).unwrap().write_all(b"second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "first\nsecond\n");

        std::fs::write(&p, vec![b'x'; 101]).unwrap();
        open_log(&p, 100).unwrap().write_all(b"fresh\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "fresh\n");

        // Exactly at the limit is kept.
        std::fs::write(&p, vec![b'x'; 100]).unwrap();
        open_log(&p, 100).unwrap().write_all(b"!").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 101);
    }

    #[cfg(unix)]
    #[test]
    fn new_log_file_and_dir_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("new").join(LOG_FILE);
        open_log(&p, 100).unwrap();
        assert_eq!(mode(&p) & 0o077, 0, "log mode {:o}", mode(&p));
        assert_eq!(mode(p.parent().unwrap()) & 0o077, 0);
    }
}
