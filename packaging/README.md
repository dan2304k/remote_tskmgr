# Running rtm-sender at login

The sender reads `sender.toml` from the OS config dir (created with defaults on first run):

| OS | Config file |
|---|---|
| Linux | `~/.config/rtm/sender.toml` |
| Windows | `%APPDATA%\rtm\sender.toml` |
| macOS | `~/Library/Application Support/rtm/sender.toml` |

Edits to the file are applied while the sender runs (interval, targets, host name,
secret, metrics). Check a config without sending anything:

```sh
rtm-sender --print-config     # effective config (file + flags)
rtm-sender --once             # one JSON packet on stdout
```

Build: `cd sender && cargo build --release -p rtm-sender` (binary at
`sender/target/release/rtm-sender`, `.exe` on Windows).

## Linux (systemd user service)

No root needed.

```sh
install -Dm755 sender/target/release/rtm-sender ~/.local/bin/rtm-sender
install -Dm644 packaging/rtm-sender.service ~/.config/systemd/user/rtm-sender.service
systemctl --user daemon-reload
systemctl --user enable --now rtm-sender
journalctl --user -u rtm-sender -f          # logs; set RTM_LOG=debug in the unit for more
```

To keep it running when you are not logged in (headless boxes):
`loginctl enable-linger "$USER"` (may need an admin once).

Uninstall: `systemctl --user disable --now rtm-sender` and delete the two files.

## Windows (Task Scheduler at logon)

A real Windows service is out of scope for v0.1; a logon task works well.

```bat
mkdir "%LOCALAPPDATA%\rtm"
copy sender\target\release\rtm-sender.exe "%LOCALAPPDATA%\rtm\rtm-sender.exe"
schtasks /create /tn "RTM Sender" /tr "\"%LOCALAPPDATA%\rtm\rtm-sender.exe\"" /sc onlogon /rl limited /f
schtasks /run /tn "RTM Sender"
```

The headless build opens a console window when started this way. To hide it, in Task
Scheduler set the task to "Run whether user is logged on or not" (no window), or use the
`gui` build (milestone M3), which has no console and lives in the tray.

Allow the sender through Windows Defender Firewall if prompted (outbound UDP/TCP 48000 is
normally allowed; the prompt appears only for private-network profiles with strict rules).

Remove: `schtasks /delete /tn "RTM Sender" /f`.

## macOS

Not packaged yet. A `launchd` user agent (`~/Library/LaunchAgents/*.plist` with
`RunAtLoad` and `KeepAlive`) will be added later; for now run `rtm-sender` from a terminal.


## Windows: install.bat / setup.exe

After `sender\build.bat`, `dist\windows-x86_64\` holds `rtm-sender.exe`, `install.bat` and `uninstall.bat`.

```
install.bat                  per-user, no admin prompt  ->  %LOCALAPPDATA%\Programs\rtm-sender
install.bat /system          all users, asks for admin  ->  %ProgramFiles%\rtm-sender
install.bat /noautostart     skip start-at-logon (on by default; HKCU Run, same switch as the tray's "Start with login")
install.bat /nostart         do not launch after installing
uninstall.bat [/purge]       removes the program (and with /purge the config in %APPDATA%\rtm)
```

It stops a running sender, copies the files, adds a Start Menu shortcut and an "Apps & features"
entry, and starts the tray app. For a conventional setup.exe, install Inno Setup 6 and run
`iscc packaging\windows\rtm-sender.iss`; the result is `dist\windows-x86_64\rtm-sender-setup.exe`
(per-user by default, offers all-users, with an autostart checkbox and uninstaller).
