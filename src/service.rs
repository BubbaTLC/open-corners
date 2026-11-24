use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SERVICE_NAME: &str = "open-corners.service";

pub fn pid_file_path() -> PathBuf {
    let base = dirs::runtime_dir().unwrap_or_else(env::temp_dir);
    base.join("open-corners.pid")
}

pub fn write_pid_file() -> Result<()> {
    let path = pid_file_path();
    fs::write(&path, std::process::id().to_string())
        .with_context(|| format!("Failed to write PID file {}", path.display()))?;
    Ok(())
}

pub fn remove_pid_file() {
    let _ = fs::remove_file(pid_file_path());
}

fn is_pid_open_corners(pid: u32) -> bool {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    if let Ok(bytes) = fs::read(cmdline_path) {
        let text = String::from_utf8_lossy(&bytes);
        text.contains("open-corners") && text.contains("daemon")
    } else {
        false
    }
}

pub fn running_daemon_pid() -> Option<u32> {
    let raw = fs::read_to_string(pid_file_path()).ok()?;
    let pid: u32 = raw.trim().parse().ok()?;
    if is_pid_open_corners(pid) {
        Some(pid)
    } else {
        None
    }
}

pub fn is_daemon_running() -> bool {
    if running_daemon_pid().is_some() {
        return true;
    }
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn service_file_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("Could not determine XDG config directory")?;
    Ok(base.join("systemd").join("user").join(SERVICE_NAME))
}

pub fn is_autostart_enabled() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn install_systemd_service() -> Result<PathBuf> {
    let exe = env::current_exe().context("Could not determine current executable path")?;
    let service_path = service_file_path()?;
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let unit = format!(
        "[Unit]\n\
         Description=Open Corners - Wayland Hot Corners Daemon\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} daemon\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe.display()
    );

    fs::write(&service_path, unit)
        .with_context(|| format!("Failed to write {}", service_path.display()))?;

    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();

    Ok(service_path)
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    if enabled {
        install_systemd_service()?;
        let status = Command::new("systemctl")
            .args(["--user", "enable", "--now", SERVICE_NAME])
            .status()
            .context("Failed to run systemctl --user enable")?;
        if !status.success() {
            anyhow::bail!("systemctl --user enable failed with status {status}");
        }
    } else {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", SERVICE_NAME])
            .status();
    }
    Ok(())
}

pub fn start_daemon() -> Result<()> {
    if is_daemon_running() {
        return Ok(());
    }

    // If systemd user service is installed, prefer starting via systemd
    if service_file_path().map(|p| p.exists()).unwrap_or(false) {
        let ok = Command::new("systemctl")
            .args(["--user", "start", SERVICE_NAME])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
    }

    let exe = env::current_exe().context("Could not determine current executable path")?;
    Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn open-corners daemon")?;
    Ok(())
}

pub fn stop_daemon() -> Result<()> {
    // Stop systemd service if active
    let _ = Command::new("systemctl")
        .args(["--user", "stop", SERVICE_NAME])
        .status();

    if let Some(pid) = running_daemon_pid() {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        remove_pid_file();
    }
    Ok(())
}

pub fn install_desktop_entry() -> Result<PathBuf> {
    let exe = env::current_exe().context("Could not determine current executable path")?;
    let data_dir = dirs::data_local_dir().context("Could not determine XDG data directory")?;
    let apps_dir = data_dir.join("applications");
    fs::create_dir_all(&apps_dir)?;
    let desktop_path = apps_dir.join("open-corners.desktop");

    let content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Open Corners\n\
         Comment=Configure macOS-style hot corners for Wayland\n\
         Exec={}\n\
         Icon=preferences-desktop-display\n\
         Categories=Settings;DesktopSettings;\n\
         Terminal=false\n",
        exe.display()
    );
    fs::write(&desktop_path, content)?;
    Ok(desktop_path)
}
