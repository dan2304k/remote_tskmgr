//! Start-at-login registration. The real state (`is_enabled`) is the source of truth: the
//! installers register it without touching `gui.autostart`, so Settings shows and changes
//! the registration itself, and Save applies it whenever it differs from the wish.
//! Linux: an XDG autostart entry `~/.config/autostart/rtm-sender.desktop`.
//! Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\rtm-sender`.
//! The registered command is this executable plus `--config <path>` when one was given, so a
//! non-default config keeps being used after login.

use anyhow::Context;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Register (`true`) or unregister (`false`) the sender to start at login.
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("locating the running executable")?;
    let config =
        config_arg(std::env::args_os().skip(1)).map(|p| std::path::absolute(&p).unwrap_or(p));
    platform::set(enabled, &exe, config.as_deref())
}

/// Whether the sender is currently registered to start at login (Windows: the HKCU Run
/// value exists; Linux: the XDG autostart desktop file exists; elsewhere: never).
pub fn is_enabled() -> bool {
    platform::is_enabled()
}

/// The value of `--config <p>` / `--config=<p>` in `args`, if any.
pub fn config_arg(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--config" {
            return it.next().map(PathBuf::from);
        }
        if let Some(v) = a.to_str().and_then(|s| s.strip_prefix("--config=")) {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// Linux XDG autostart entry. Compiled on every OS so its tests run anywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod xdg {
    use anyhow::Context;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    pub const DESKTOP_FILE: &str = "rtm-sender.desktop";

    /// `$XDG_CONFIG_HOME/autostart`, else `$HOME/.config/autostart`. Relative values are ignored,
    /// as the XDG base directory spec requires.
    pub fn xdg_autostart_dir(
        xdg_config_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Option<PathBuf> {
        let abs = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
        abs(xdg_config_home)
            .or_else(|| abs(home).map(|h| h.join(".config")))
            .map(|c| c.join("autostart"))
    }

    /// Quote one Exec argument per the Desktop Entry spec, then escape it as a string value.
    pub fn exec_arg(arg: &str) -> String {
        const RESERVED: &[char] = &[
            ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#',
            '(', ')', '`',
        ];
        let quoted = if arg.is_empty() || arg.contains(RESERVED) {
            let mut q = String::from("\"");
            for c in arg.chars() {
                if matches!(c, '"' | '`' | '$' | '\\') {
                    q.push('\\');
                }
                q.push(c);
            }
            q.push('"');
            q
        } else {
            arg.to_string()
        };
        // String-value escaping happens after quoting; `%` introduces field codes.
        quoted.replace('\\', "\\\\").replace('%', "%%")
    }

    pub fn desktop_entry(exe: &Path, config: Option<&Path>) -> String {
        let mut exec = exec_arg(&exe.to_string_lossy());
        if let Some(c) = config {
            exec.push_str(" --config ");
            exec.push_str(&exec_arg(&c.to_string_lossy()));
        }
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=RTM Sender\n\
             Comment=Send system stats to RTM viewers on the LAN\n\
             Exec={exec}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        )
    }

    /// Whether `<dir>/rtm-sender.desktop` exists.
    pub fn desktop_entry_exists(dir: &Path) -> bool {
        dir.join(DESKTOP_FILE).is_file()
    }

    /// Write or remove `<dir>/rtm-sender.desktop`. Removing a missing file is fine.
    pub fn set_desktop_entry(
        dir: &Path,
        enabled: bool,
        exe: &Path,
        config: Option<&Path>,
    ) -> anyhow::Result<()> {
        let file = dir.join(DESKTOP_FILE);
        if enabled {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            std::fs::write(&file, desktop_entry(exe, config))
                .with_context(|| format!("writing {}", file.display()))?;
            tracing::info!(file = %file.display(), "autostart enabled");
        } else {
            match std::fs::remove_file(&file) {
                Ok(()) => tracing::info!(file = %file.display(), "autostart disabled"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", file.display())),
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::xdg::{desktop_entry_exists, set_desktop_entry, xdg_autostart_dir};
    use super::*;

    fn dir() -> Option<PathBuf> {
        xdg_autostart_dir(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    pub fn is_enabled() -> bool {
        dir().is_some_and(|d| desktop_entry_exists(&d))
    }

    pub fn set(enabled: bool, exe: &Path, config: Option<&Path>) -> anyhow::Result<()> {
        let dir = dir().context("no XDG_CONFIG_HOME or HOME to put the autostart entry in")?;
        set_desktop_entry(&dir, enabled, exe, config)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "rtm-sender";

    pub fn is_enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_raw_value(VALUE))
            .is_ok()
    }

    pub fn set(enabled: bool, exe: &Path, config: Option<&Path>) -> anyhow::Result<()> {
        let (run, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .context("opening HKCU Run key")?;
        if enabled {
            let mut cmd = format!("\"{}\"", exe.display());
            if let Some(c) = config {
                cmd.push_str(&format!(" --config \"{}\"", c.display()));
            }
            run.set_value(VALUE, &cmd)
                .context("writing HKCU Run value")?;
            tracing::info!(%cmd, "autostart enabled");
        } else {
            match run.delete_value(VALUE) {
                Ok(()) => tracing::info!("autostart disabled"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("deleting HKCU Run value"),
            }
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::*;
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_enabled: bool, _exe: &Path, _config: Option<&Path>) -> anyhow::Result<()> {
        anyhow::bail!("autostart is not implemented on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::xdg::*;
    use super::*;

    // Unix-style absolute paths; the XDG logic is only used on Linux.
    #[cfg(unix)]
    #[test]
    fn autostart_dir_resolution() {
        let o = |s: &str| Some(OsString::from(s));
        assert_eq!(
            xdg_autostart_dir(o("/x/cfg"), o("/home/u")),
            Some(PathBuf::from("/x/cfg/autostart"))
        );
        assert_eq!(
            xdg_autostart_dir(None, o("/home/u")),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        assert_eq!(
            xdg_autostart_dir(o("relative"), o("/home/u")),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        assert_eq!(xdg_autostart_dir(o(""), None), None);
    }

    #[test]
    fn desktop_file_write_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        // Same resolution the real call does, with a temp XDG_CONFIG_HOME.
        let dir = xdg_autostart_dir(Some(tmp.path().into()), None).unwrap();
        let exe = Path::new("/opt/rtm/rtm-sender");
        set_desktop_entry(&dir, true, exe, None).unwrap();
        let text = std::fs::read_to_string(dir.join(DESKTOP_FILE)).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nExec=/opt/rtm/rtm-sender\n"), "{text}");
        assert!(text.contains("\nX-GNOME-Autostart-enabled=true\n"));
        assert!(text.contains("\nType=Application\n"));

        // Re-enabling overwrites; disabling removes; disabling again is a no-op.
        set_desktop_entry(&dir, true, exe, Some(Path::new("/etc/rtm/s.toml"))).unwrap();
        let text = std::fs::read_to_string(dir.join(DESKTOP_FILE)).unwrap();
        assert!(text.contains("Exec=/opt/rtm/rtm-sender --config /etc/rtm/s.toml\n"));
        set_desktop_entry(&dir, false, exe, None).unwrap();
        assert!(!dir.join(DESKTOP_FILE).exists());
        set_desktop_entry(&dir, false, exe, None).unwrap();
    }

    /// The real `is_enabled`/`set_autostart` path, pointed at a temp XDG_CONFIG_HOME. This is
    /// the only test that sets XDG_CONFIG_HOME.
    #[cfg(target_os = "linux")]
    #[test]
    fn is_enabled_follows_desktop_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let file = tmp.path().join("autostart").join(DESKTOP_FILE);
        assert!(!is_enabled());
        set_autostart(true).unwrap();
        assert!(file.is_file());
        assert!(is_enabled());
        set_autostart(false).unwrap();
        assert!(!file.exists());
        assert!(!is_enabled());
        // A desktop file someone else wrote counts too.
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[Desktop Entry]\n").unwrap();
        assert!(is_enabled());
        // XDG_CONFIG_HOME is left pointing into the temp dir rather than removed, so a racing
        // test can never reach the developer's real ~/.config/autostart.
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(exec_arg("/usr/bin/rtm-sender"), "/usr/bin/rtm-sender");
        assert_eq!(exec_arg("/home/u/My Apps/rtm"), "\"/home/u/My Apps/rtm\"");
        assert_eq!(exec_arg("/a/$x"), "\"/a/\\\\$x\"");
        assert_eq!(exec_arg("/a/100%"), "/a/100%%");
    }

    #[test]
    fn config_arg_forms() {
        let v = |a: &[&str]| config_arg(a.iter().map(OsString::from));
        assert_eq!(v(&["--once"]), None);
        assert_eq!(v(&["--config", "/a.toml"]), Some(PathBuf::from("/a.toml")));
        assert_eq!(
            v(&["-v", "--config=/b.toml"]),
            Some(PathBuf::from("/b.toml"))
        );
        assert_eq!(v(&["--config"]), None);
    }
}
