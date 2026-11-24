use crate::config::Action;
use anyhow::{Context, Result};
use log::{info, warn};
use std::env;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEnv {
    Cosmic,
    Hyprland,
    Sway,
    Kde,
    Gnome,
    Other,
}

impl DesktopEnv {
    pub fn detect() -> Self {
        let raw = env::var("XDG_CURRENT_DESKTOP")
            .or_else(|_| env::var("DESKTOP_SESSION"))
            .unwrap_or_default()
            .to_ascii_lowercase();

        if raw.contains("cosmic") {
            DesktopEnv::Cosmic
        } else if raw.contains("hyprland") {
            DesktopEnv::Hyprland
        } else if raw.contains("sway") {
            DesktopEnv::Sway
        } else if raw.contains("kde") || raw.contains("plasma") {
            DesktopEnv::Kde
        } else if raw.contains("gnome") {
            DesktopEnv::Gnome
        } else {
            DesktopEnv::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DesktopEnv::Cosmic => "COSMIC",
            DesktopEnv::Hyprland => "Hyprland",
            DesktopEnv::Sway => "Sway",
            DesktopEnv::Kde => "KDE Plasma",
            DesktopEnv::Gnome => "GNOME",
            DesktopEnv::Other => "Wayland",
        }
    }
}

/// Spawn a command detached so it never blocks the Wayland event loop or GUI.
pub fn spawn_detached(program: &str, args: &[&str]) -> Result<()> {
    info!("Running action command: {} {}", program, args.join(" "));
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn '{program}'"))?;
    Ok(())
}

fn spawn_shell(command: &str) -> Result<()> {
    info!("Running custom shell command: {command}");
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to run shell command: {command}"))?;
    Ok(())
}

fn try_first_available(candidates: &[(&str, &[&str])]) -> Result<()> {
    let mut last_err = None;
    for (program, args) in candidates {
        match spawn_detached(program, args) {
            Ok(()) => return Ok(()),
            Err(err) => {
                warn!("Candidate '{program}' unavailable: {err}");
                last_err = Some(err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("No command candidates available")))
}

/// Execute an action via desktop commands.
/// Note: The daemon intercepts `Action::ShowDesktop` when the compositor supports
/// Wayland foreign-toplevel minimization; this function provides the fallback/CLI path.
pub fn execute_action(action: &Action) -> Result<()> {
    let de = DesktopEnv::detect();

    match action {
        Action::None => Ok(()),
        Action::ShowWindows => match de {
            DesktopEnv::Cosmic => spawn_detached("cosmic-workspaces", &[]),
            DesktopEnv::Kde => try_first_available(&[
                (
                    "qdbus6",
                    &[
                        "org.kde.kglobalaccel",
                        "/component/kwin",
                        "invokeShortcut",
                        "Overview",
                    ],
                ),
                (
                    "qdbus",
                    &[
                        "org.kde.kglobalaccel",
                        "/component/kwin",
                        "invokeShortcut",
                        "Overview",
                    ],
                ),
            ]),
            DesktopEnv::Hyprland => {
                spawn_detached("hyprctl", &["dispatch", "togglespecialworkspace"])
            }
            DesktopEnv::Gnome => spawn_detached(
                "dbus-send",
                &[
                    "--session",
                    "--type=method_call",
                    "--dest=org.gnome.Shell",
                    "/org/gnome/Shell",
                    "org.gnome.Shell.Eval",
                    "string:Main.overview.toggle();",
                ],
            ),
            _ => try_first_available(&[
                ("cosmic-workspaces", &[]),
                ("rofi", &["-show", "window"]),
                ("wofi", &["--show", "drun"]),
            ]),
        },
        Action::ShowDesktop => match de {
            DesktopEnv::Kde => try_first_available(&[
                (
                    "qdbus6",
                    &[
                        "org.kde.kglobalaccel",
                        "/component/kwin",
                        "invokeShortcut",
                        "Show Desktop",
                    ],
                ),
                (
                    "qdbus",
                    &[
                        "org.kde.kglobalaccel",
                        "/component/kwin",
                        "invokeShortcut",
                        "Show Desktop",
                    ],
                ),
            ]),
            DesktopEnv::Sway => spawn_detached("swaymsg", &["workspace", "number", "99"]),
            DesktopEnv::Hyprland => {
                spawn_detached("hyprctl", &["dispatch", "workspace", "empty"])
            }
            DesktopEnv::Cosmic => {
                // Fallback if Wayland foreign-toplevel protocol wasn't used by daemon
                spawn_detached("cosmic-workspaces", &[])
            }
            _ => try_first_available(&[
                ("wmctrl", &["-k", "on"]),
                ("xdotool", &["key", "Super_L+d"]),
            ]),
        },
        Action::Launcher => match de {
            DesktopEnv::Cosmic => spawn_detached("cosmic-launcher", &[]),
            DesktopEnv::Kde => try_first_available(&[
                (
                    "qdbus6",
                    &[
                        "org.kde.kglobalaccel",
                        "/component/kwin",
                        "invokeShortcut",
                        "ShowDesktopGrid",
                    ],
                ),
                ("krunner", &[]),
            ]),
            _ => try_first_available(&[
                ("cosmic-launcher", &[]),
                ("fuzzel", &[]),
                ("wofi", &["--show", "drun"]),
                ("rofi", &["-show", "drun"]),
            ]),
        },
        Action::AppLibrary => match de {
            DesktopEnv::Cosmic => spawn_detached("cosmic-app-library", &[]),
            _ => execute_action(&Action::Launcher),
        },
        Action::LockScreen => try_first_available(&[
            ("loginctl", &["lock-session"]),
            ("swaylock", &[]),
            ("hyprlock", &[]),
        ]),
        Action::Screenshot => match de {
            DesktopEnv::Cosmic => spawn_detached("cosmic-screenshot", &[]),
            DesktopEnv::Kde => spawn_detached("spectacle", &[]),
            DesktopEnv::Gnome => spawn_detached("gnome-screenshot", &["-i"]),
            _ => try_first_available(&[
                ("cosmic-screenshot", &[]),
                ("spectacle", &[]),
                ("flameshot", &["gui"]),
                ("grim", &[]),
            ]),
        },
        Action::Custom { command } => {
            let trimmed = command.trim();
            if trimmed.is_empty() {
                Ok(())
            } else {
                spawn_shell(trimmed)
            }
        }
    }
}
