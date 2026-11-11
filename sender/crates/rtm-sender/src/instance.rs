//! Single-instance guard: an exclusive lock on `<config path>.lock` (for example
//! `sender.toml.lock`) held for as long as the sender runs. A second sender with the same
//! config file exits with [`AlreadyRunning`] (exit code 3). The OS drops the lock when the
//! process ends, however it ends; the file itself is left in place.

use anyhow::Context;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// Process exit code for [`AlreadyRunning`].
pub const EXIT_ALREADY_RUNNING: u8 = 3;

/// Another process holds the lock for this config file.
#[derive(Debug)]
pub struct AlreadyRunning {
    pub config: PathBuf,
    pub lock: PathBuf,
}

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rtm-sender is already running with config {} (lock file {}); \
             quit that one first, or use another --config",
            self.config.display(),
            self.lock.display()
        )
    }
}

impl std::error::Error for AlreadyRunning {}

/// Keeps the lock while alive.
#[derive(Debug)]
pub struct InstanceLock {
    _file: Option<File>,
}

pub fn lock_path(config: &Path) -> PathBuf {
    let mut s = config.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

/// Take the lock for `config`. Fails with [`AlreadyRunning`] if another process has it. If
/// the file system does not support locking, warns and runs unguarded.
pub fn acquire(config: &Path) -> anyhow::Result<InstanceLock> {
    let lock = lock_path(config);
    if let Some(dir) = lock.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock)
        .with_context(|| format!("opening lock file {}", lock.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock { _file: Some(file) }),
        Err(TryLockError::WouldBlock) => Err(AlreadyRunning {
            config: config.to_path_buf(),
            lock,
        }
        .into()),
        Err(TryLockError::Error(e)) => {
            tracing::warn!(
                "cannot lock {} ({e}); not guarding against a second instance",
                lock.display()
            );
            Ok(InstanceLock { _file: None })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_on_the_same_config_fails_until_released() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("sub").join("sender.toml");
        let first = acquire(&cfg).unwrap();
        assert!(tmp.path().join("sub").join("sender.toml.lock").exists());

        let err = acquire(&cfg).unwrap_err();
        let ar = err
            .downcast_ref::<AlreadyRunning>()
            .expect("AlreadyRunning");
        assert_eq!(ar.config, cfg);
        assert!(err.to_string().contains("already running"), "{err}");

        // Another config in the same directory is independent.
        let other = acquire(&tmp.path().join("sub").join("other.toml")).unwrap();

        drop(first);
        let _again = acquire(&cfg).expect("released on drop");
        drop(other);
    }
}
