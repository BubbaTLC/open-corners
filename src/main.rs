use anyhow::Result;
use clap::{Parser, Subcommand};
use open_corners::actions;
use open_corners::config::{AppConfig, Corner};
use open_corners::{daemon, gui, service};

#[derive(Parser)]
#[command(
    name = "open-corners",
    version,
    about = "macOS-style hot corners for Wayland Linux desktops"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the Hot Corners settings GUI (default when no subcommand is given)
    Gui,
    /// Run the Wayland hot-corners daemon in the foreground
    Daemon,
    /// Start the background daemon
    Start,
    /// Stop the running background daemon
    Stop,
    /// Show whether the daemon is running and print current config path
    Status,
    /// Test-trigger the configured action for a corner (top-left, top-right, bottom-left, bottom-right)
    Trigger { corner: Corner },
    /// Install & enable the systemd user service and desktop application entry
    InstallService,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Gui) {
        Command::Gui => gui::run_gui(),
        Command::Daemon => daemon::run_daemon(),
        Command::Start => {
            service::start_daemon()?;
            println!("open-corners daemon started.");
            Ok(())
        }
        Command::Stop => {
            service::stop_daemon()?;
            println!("open-corners daemon stopped.");
            Ok(())
        }
        Command::Status => {
            let running = service::is_daemon_running();
            let autostart = service::is_autostart_enabled();
            let cfg_path = AppConfig::config_path()?;
            println!(
                "Daemon:    {}",
                if running { "running" } else { "stopped" }
            );
            println!(
                "Autostart: {}",
                if autostart { "enabled" } else { "disabled" }
            );
            println!("Desktop:   {}", actions::DesktopEnv::detect().name());
            println!("Config:    {}", cfg_path.display());
            Ok(())
        }
        Command::Trigger { corner } => {
            let cfg = AppConfig::load()?;
            let action = cfg.action_for(corner);
            println!("Triggering {} -> {}", corner.label(), action.label());
            actions::execute_action(action)
        }
        Command::InstallService => {
            service::set_autostart(true)?;
            let desktop = service::install_desktop_entry()?;
            println!("Enabled systemd user service: open-corners.service");
            println!("Installed desktop entry:      {}", desktop.display());
            Ok(())
        }
    }
}
