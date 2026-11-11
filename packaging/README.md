# Installing rtm-sender and running it at login

## Which binary

Each GitHub release has these sender assets:

| Asset | Use it for |
|---|---|
| `rtm-sender-linux-x86_64` | Headless Linux (servers, no desktop). No GTK, no display needed. |
| `rtm-sender-gui-linux-x86_64` | Linux desktops: tray icon + settings window. Needs GTK 3 and libayatana-appindicator3 installed; without them it does not start at all, not even with `--no-gui`. |
| `rtm-sender-windows-x86_64.zip` | Windows: `rtm-sender.exe` (tray + settings), `install.bat`, `uninstall.bat`, `LICENSE`, this file as `INSTALL.md`. |
| `rtm-sender-windows-x86_64.exe` | Windows: the same exe alone, to run in place. |
| `rtm-sender-setup.exe` | Windows: Inno Setup installer for the same exe (see below). |
| `SHA256SUMS` | SHA-256 of every asset above and `rtm-viewer.apk`. Check with `sha256sum -c SHA256SUMS --ignore-missing` (PowerShell: `Get-FileHash <file>`). |

The Windows exe carries a version resource (Explorer → Properties → Details) and the tray icon
as its file icon.

Or build it yourself: `sender/build.sh` / `sender\build.bat` (tray build into `dist/<os>-<arch>/`,
add `headless` for the no-GUI build), or `cd sender && cargo build --release -p rtm-sender`
(headless) / `... --features gui` (tray).

## Config

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

Only one sender runs per config file. It holds a lock on `sender.toml.lock` next to the config;
a second copy started with the same config exits with exit code 3 and "already running" (on
stderr, in the log, and on Windows in a message box). `--once` and `--print-config` do not take
the lock. A sender with a different `--config` is independent.

## Log file

The tray build (except `--once` / `--print-config`) writes its log to `sender.log` in the same
folder as `sender.toml` (`~/.config/rtm/sender.log`, `%APPDATA%\rtm\sender.log`) as well as to
stderr. A log over 1 MB is emptied when the sender starts. The tray menu has "Open log file"
and "Open config folder". On Windows the tray exe has no console, so when it cannot start (an
invalid `sender.toml`, already running) it shows a message box with the error and the log path;
a bad command-line flag is printed to the terminal it was started from, or shown in a message
box if there is none. The headless build logs to stderr only (the journal under systemd).

## Linux desktop: tray build

Tick Settings → Startup → "Start at login" and press Save. That writes
`~/.config/autostart/rtm-sender.desktop`; unticking and saving removes it.

## Linux server: systemd user service

No root needed. Use the headless binary (the unit passes `--no-gui`, so the tray build also
works if its GTK libraries are installed).

```sh
install -Dm755 rtm-sender-linux-x86_64 ~/.local/bin/rtm-sender   # or sender/target/release/rtm-sender
install -Dm644 packaging/rtm-sender.service ~/.config/systemd/user/rtm-sender.service
systemctl --user daemon-reload
systemctl --user enable --now rtm-sender
journalctl --user -u rtm-sender -f          # logs; set RTM_LOG=debug in the unit for more
```

To keep it running when you are not logged in (headless boxes):
`loginctl enable-linger "$USER"` (may need an admin once).

Uninstall: `systemctl --user disable --now rtm-sender` and delete the two files.

Do not enable both the service and "Start at login": two senders would send duplicate packets.

## Windows: install.bat

Unpack `rtm-sender-windows-x86_64.zip` (or use `dist\windows-x86_64\` after `sender\build.bat`)
and run `install.bat` from that folder.

```
install.bat                  per-user, no admin prompt  ->  %LOCALAPPDATA%\Programs\rtm-sender
install.bat /system          all users, asks for admin  ->  %ProgramFiles%\rtm-sender
install.bat /noautostart     skip start at login (on by default)
install.bat /nostart         do not launch after installing
uninstall.bat [/purge]       removes the program (and with /purge the config in %APPDATA%\rtm)
```

It stops a running sender, copies the files, adds a Start Menu shortcut and an "Apps & features"
entry, registers start at login and starts the tray app. Start at login is the value
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\rtm-sender`, the same one the tray's
Settings → Startup → "Start at login" checkbox shows and changes. `uninstall.bat` (also run
from "Apps & features") asks for admin when the program is under Program Files.

## Windows: setup.exe (Inno Setup)

Each release has `rtm-sender-setup.exe`, built by the release workflow on the Windows runner. To
build it yourself: install Inno Setup 6, build with `sender\build.bat`, then
`iscc /DAppVersion=1.2.3 packaging\windows\rtm-sender.iss` (without `/DAppVersion` the version is
read from `sender\Cargo.toml`). The result is `dist\windows-x86_64\rtm-sender-setup.exe`:
per-user by default, offers all users, has a "Start rtm-sender when I log in" checkbox (HKCU Run
for a per-user install, HKLM Run, i.e. every user, for an all-users install) and an uninstaller
that also removes the HKCU Run value the tray may have written.

## Windows: Task Scheduler instead

If you prefer a logon task over the Run value (for example for the headless build), name it
`rtm-sender`; `uninstall.bat` removes a task with that name.

```bat
schtasks /create /tn rtm-sender /tr "\"%LOCALAPPDATA%\Programs\rtm-sender\rtm-sender.exe\"" /sc onlogon /rl limited /f
schtasks /run /tn rtm-sender
schtasks /delete /tn rtm-sender /f
```

The headless build opens a console window when started this way; the tray build does not.
Use either the task or "Start at login", not both.

Allow the sender through Windows Defender Firewall if prompted (outbound UDP/TCP 48000 is
normally allowed; the prompt appears only for private-network profiles with strict rules).

## macOS

Not packaged yet. A `launchd` user agent (`~/Library/LaunchAgents/*.plist` with
`RunAtLoad` and `KeepAlive`) will be added later; for now run `rtm-sender` from a terminal.
