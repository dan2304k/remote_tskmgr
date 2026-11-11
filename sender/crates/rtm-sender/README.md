# rtm-sender

The RTM sender: samples CPU / GPU / memory / disk (and optional extras) on a fixed interval and
sends one JSON packet per tick to RTM viewers on the LAN (UDP broadcast, UDP unicast, TCP
unicast). Wire format: `protocol/PROTOCOL.md`.

## Cargo features

| Feature | Default | What it adds |
|---|---|---|
| *(none)* | yes | Headless daemon. No system libraries needed. |
| `gui` | no | Tray icon (`tray-icon`) and a settings window (`eframe`/`egui`, OpenGL via glow). |

```sh
cargo build --release -p rtm-sender                  # headless, ~1.8 MB on Linux x86_64
cargo build --release -p rtm-sender --features gui   # tray + settings, ~8.5 MB
```

The `gui` feature needs a newer compiler than the headless build (eframe 0.37 declares
rust-version 1.95).

### Linux build dependencies for `gui`

```sh
sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libxkbcommon-dev libwayland-dev
```

At run time it needs GTK 3, libayatana-appindicator3 (or libappindicator3) and an OpenGL
driver (Mesa is fine). The GTK libraries are linked into the `gui` binary, so they must be
installed even to run it with `--no-gui` or without a display; on a server use the headless
build (release asset `rtm-sender-linux-x86_64`) instead. GNOME needs the "AppIndicator and KStatusNotifierItem Support" extension
to show tray icons; KDE, XFCE, Cinnamon and most others show them natively.

Windows needs nothing extra. The `gui` build uses the Windows GUI subsystem, so no console
window opens when it starts at login.

## Running

```
rtm-sender                 # run (tray + settings when built with gui)
rtm-sender --once          # sample once, print the packet JSON, exit
rtm-sender --print-config  # show the effective config (file + flags)
rtm-sender --no-gui        # gui build: behave exactly like the headless build
rtm-sender --help
```

Config: `~/.config/rtm/sender.toml` (Linux), `%APPDATA%\rtm\sender.toml` (Windows); created
with comments on first run and hot-reloaded when edited. Command-line flags (`--interval`,
`--udp-unicast`, `--tcp-unicast`, `--no-broadcast`) override the file for that run only, and
keep overriding it after edits from the GUI.

On Windows the `gui` build attaches to the parent console when started with `--once`,
`--print-config`, `--help` or `--version`, so their output still appears in the terminal.

## Tray and settings window (`gui`)

Threads: the daemon loop runs on its own thread, exactly as in the headless build. The tray
icon has its own thread (GTK main loop on Linux, Win32 message pump on Windows). The main
thread opens the settings window when asked. The GUI changes the daemon only through the
config file and the daemon's control channel (reload, pause, shutdown).

Tray menu:

* **status line**: host · interval · number of targets (plus "paused" or "send error").
  On Windows the same text is the icon's tooltip. Linux AppIndicator has no tooltips.
* **Interval 10/s (100 ms) / 5/s (200 ms) / 1 s / 5 s / 15 s / 30 s**: writes
  `general.interval_ms` to the TOML and applies it immediately. The same choices are in
  Settings → General. Any value from 100 ms to 1 hour is accepted in the file or with
  `--interval` (for example `--interval 100ms`). CPU usage and disk/network rates are
  computed at most every 200 ms. At 10/s, every other packet repeats the previous reading
  instead of showing an empty CPU gauge or spiky rates. Each packet still has its own `seq`
  and timestamp.
* **Pause sending / Resume sending**: while paused the loop keeps sampling (so the first packet
  after resuming has correct rates) but sends nothing. Pause is not saved: a restart always
  sends. The icon is greyed out while paused.
* **Settings…** opens the settings window. On Windows a left click on the icon does the same.
* **Quit** stops the daemon cleanly and exits.

Settings window: General (host name, interval), Network (broadcast on/off and port, interfaces
All / Default route / Named list, skip prefixes, UDP and TCP unicast targets, shared secret
with Show), Metrics (enable and order every known metric, GPU backend, disk include list),
Startup (start minimized, show a splash on start, start at login).

* **Unicast targets** accept `192.168.1.50`, `192.168.1.50:48001`, `[fe80::1]:48000`,
  `viewer.lan` or `viewer.lan:48001`. Without a port the UDP broadcast port (`udp_port`,
  48000 by default) is used. The address that packets will go to is shown under each row,
  e.g. "sends to 192.168.1.50:48000". Rows are stored as typed. Host names are looked up when
  the config is applied, with the first IPv4 address preferred. A name that does not resolve
  is logged and skipped, and it is looked up again on the next config reload (Save, or an
  edit to the file). Errors are short, wrap under the row, and the full text is on the
  red "!" tooltip.
* **GPU backend** shows readable names. The values in the file stay the same:

  | Shown as | `gpu_backend` |
  |---|---|
  | Auto (recommended) | `auto` |
  | NVIDIA (NVML) | `nvml` |
  | AMD (Linux sysfs) | `amdgpu` |
  | Intel (Linux sysfs, no utilisation) | `intel` |
  | Windows performance counters (any GPU) | `pdh` |
  | Off | `none` |

* **Save** validates everything and shows any errors above the buttons. If valid, it writes
  the TOML (comments in the file are lost, as with any `Config::save`) and applies it with no
  restart. The status panel then shows the result.
* **Revert** drops unsaved edits and reloads the saved settings.
* **Close**, or the window's close button, hides the settings window; the sender keeps running
  in the tray.
* The bottom panel is read-only: where packets go, last packet size and sequence number,
  packets per second, last send error.

`gui.start_minimized = true` (the default) starts with only the tray icon. Set it to `false`
to open the settings window at launch.

### Start-up splash

When the sender starts minimized, a small borderless note appears for about 3 seconds in the
bottom-right corner of the primary screen, next to the Windows notification area. It shows
the icon, "rtm-sender is running", where to find the tray icon, and a bar that runs down.
Click it to close it early. The daemon is already sending while it shows. Quit or Settings…
in the tray also work during that time. Turn it off with `gui.show_splash = false`, or untick
Settings → Startup → "Show a splash on start". No splash appears when `start_minimized =
false`, because the settings window opens instead. On Linux the work area is not known, so
the note is placed 48 px above the bottom-right corner. Wayland compositors choose the
position themselves. If a borderless window cannot be created, a normal small window with a
title is used.

### Autostart

The "Start at login" checkbox shows whether the sender is registered right now (by this
checkbox, `install.bat` or the setup.exe), not the `gui.autostart` value in the file. Ticking
it and pressing Save registers the sender; unticking it and saving removes the registration,
including one an installer made. Nothing changes until you save. Changing the interval from
the tray leaves the registration as it is.

* Linux: writes `~/.config/autostart/rtm-sender.desktop` (or under `$XDG_CONFIG_HOME`), with
  `Exec=` set to the running executable and `X-GNOME-Autostart-enabled=true`.
* Windows: sets `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\rtm-sender` to the quoted
  executable path.
* If the sender was started with `--config <path>`, the registered command includes it.
* If you move the executable, toggle autostart off and on again to update the path.
* macOS: not implemented (Save reports an error if you enable it).

For a headless machine use the systemd user unit in `packaging/` instead.

### No display

The Linux `gui` binary is dynamically linked against GTK 3 and libayatana-appindicator3: if
those libraries are missing it does not start at all, `--no-gui` included (the loader fails
before `main`). Use the headless build on machines without them. With the libraries present,
when the `gui` build cannot start its GUI, it logs a warning and runs exactly like the headless
build; the GUI never takes the daemon down with it. This happens when:

* `--no-gui` is given;
* on Linux, neither `DISPLAY` nor `WAYLAND_DISPLAY` is set;
* GTK or the tray icon fails to initialise.

If only the settings window fails (for example, no OpenGL), the tray keeps working without it.

## GPU backend and adapter selection

### Which GPU is reported: `metrics.gpu_adapter`

```toml
[metrics]
gpu_adapter = "auto"     # or "#1", or part of the adapter name such as "R9700"
```

* `auto` (default) reports the adapter with the most dedicated VRAM. A discrete card beats
  an integrated GPU, even when the integrated GPU drives the display. VRAM ties go to the
  non-integrated adapter, then to the lowest index.
* `"#N"` is the index in the detected list. A name substring is matched case-insensitively,
  and the first match wins. A value that matches nothing is logged and treated as `auto`.
* Settings → Metrics → **GPU adapter** lists the detected adapters with their index and VRAM,
  marks the one being reported now, and also has a free-text field. Picking an adapter stores
  its name, or `#N` if another adapter has the same name.
* Detection on Windows uses DXGI (`EnumAdapters1`). It gives each adapter's name, the LUID
  that the performance-counter instances carry, and the dedicated VRAM. Compute-only cards
  with no display attached are included. If DXGI fails, the display-class registry
  `DriverDesc` names are used. Those have no LUID, so `auto` falls back to counter
  heuristics.
* Detection on Linux reads each `/sys/class/drm/cardN`. The name comes from
  `device/product_name` when present, otherwise from the PCI vendor and device ids. VRAM
  comes from `device/mem_info_vram_total`.

To see what was detected and chosen, run `rtm-sender --once` and look at `metrics.gpu.detail`:
`name` and `adapter_index` give the reported adapter. `adapters` maps `"#N"` to `name`,
`vram_total_mb`, the per-adapter load and `chosen: true|false`. On Windows it also shows
`adapter` (the LUID) and `selected_by` (`gpu_adapter`, `largest_vram`, or a fallback
heuristic).

### What the percentage means

* **Windows performance counters (`pdh`):** instances are grouped per adapter. The value is
  the busiest engine type on that adapter: 3D, Compute_0/1, Copy, VideoDecode/Encode and so
  on. Each type is summed over processes and capped at 100. This is Task Manager's
  semantics, so a card running only AI/compute work shows its load.
  `detail.engines` has the per-type numbers for the chosen adapter. An idle GPU reports 0.
  A tick whose counter data is invalid repeats the previous value, and only three failed
  queries in a row give an empty value.
* **AMD on Linux (`amdgpu`):** `gpu_busy_percent` of the chosen card, which includes compute.
  `detail.adapters` gives every card's `busy`, `vram_used_mb` and `vram_total_mb`.
* **NVIDIA (`nvml`):** the device that matches the chosen adapter. On hybrid laptops the
  dGPU sleeps at idle and cannot be read. It then reports 0 with `detail.state = "asleep"`
  and keeps retrying, instead of being dropped for the integrated GPU.

### Backend choice

`auto` tries NVIDIA (NVML), then AMD and Intel sysfs (Linux), then Windows performance
counters. In `auto` mode, a backend skips itself if the target adapter is not its vendor's.
On an all-AMD Windows PC that means the counters are used, and they report the target
adapter. A backend is chosen if its first reading has a value. A backend that has never
produced a value may be replaced once by the next one, after 5 empty reads. A backend that
has worked is never replaced. Every backend except Intel sysfs repeats its last value for up
to 3 empty ticks. An explicitly chosen backend never switches, and it reports its own default
adapter if the target is not one of its own.

## Tests

```sh
cargo test -p rtm-sender                  # headless
cargo test -p rtm-sender --features gui   # also runs the GUI model tests; needs no display
```

The tray icons in `assets/` are generated by `gui::icon::render`. After changing the drawing,
regenerate them with `RTM_REGEN_ICONS=1 cargo test -p rtm-sender --features gui icon`.
