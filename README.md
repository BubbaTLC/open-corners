# Open Corners

macOS-style hot corners for Wayland Linux desktops (COSMIC, Sway, Hyprland, KDE Plasma 6, River, Labwc, etc.).

## How it works

Open Corners runs a lightweight Wayland background daemon using `zwlr_layer_shell_v1` to place tiny, invisible overlay sensor zones at the active corners of every connected display—requiring **no root privileges** and working cleanly with multi-monitor setups and display scaling.

When your pointer enters a corner and stays for the configured dwell time, Open Corners triggers the assigned action:

- **Show Windows / Workspaces** — opens COSMIC Workspaces (or Overview on KDE/GNOME/Hyprland)
- **Show Desktop** — natively minimizes all visible windows (and restores them on the next trigger) via COSMIC's `zcosmic_toplevel_manager_v1` or wlroots' `zwlr_foreign_toplevel_management_v1`
- **Application Launcher** — opens COSMIC Launcher (or fuzzel/wofi/rofi)
- **App Library** — opens COSMIC App Library
- **Lock Screen** — locks the session via `loginctl lock-session`
- **Screenshot** — opens COSMIC Screenshot (or Spectacle/Flameshot)
- **Custom Command** — runs any shell command

## Building & Running

```bash
# Build release binary
cargo build --release

# Launch the Settings GUI
./target/release/open-corners

# Or install to ~/.cargo/bin
cargo install --path .
open-corners
```

## CLI Commands

```bash
open-corners                  # Open the Settings GUI
open-corners daemon           # Run the Wayland daemon in the foreground
open-corners start            # Start the background daemon
open-corners stop             # Stop the background daemon
open-corners status           # Show daemon & autostart status
open-corners trigger top-left # Test-trigger a corner's configured action
open-corners install-service  # Enable systemd user autostart + install .desktop entry
```

## Configuration

Settings are stored at `~/.config/open-corners/config.toml` and automatically live-reloaded by the daemon whenever you change them in the GUI or edit the file manually.

```toml
dwell_ms = 150
cooldown_ms = 600
corner_size_px = 8
debug_overlay = false

[top_left]
type = "show-windows"

[top_right]
type = "none"

[bottom_left]
type = "none"

[bottom_right]
type = "show-desktop"
```
