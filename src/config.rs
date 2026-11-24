use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Corner::TopLeft => "Top Left",
            Corner::TopRight => "Top Right",
            Corner::BottomLeft => "Bottom Left",
            Corner::BottomRight => "Bottom Right",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Corner::TopLeft => "top-left",
            Corner::TopRight => "top-right",
            Corner::BottomLeft => "bottom-left",
            Corner::BottomRight => "bottom-right",
        }
    }
}

impl std::str::FromStr for Corner {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top-left" | "topleft" | "tl" => Ok(Corner::TopLeft),
            "top-right" | "topright" | "tr" => Ok(Corner::TopRight),
            "bottom-left" | "bottomleft" | "bl" => Ok(Corner::BottomLeft),
            "bottom-right" | "bottomright" | "br" => Ok(Corner::BottomRight),
            other => anyhow::bail!(
                "Unknown corner '{other}'. Expected one of: top-left, top-right, bottom-left, bottom-right"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    None,
    ShowWindows,
    ShowDesktop,
    Launcher,
    AppLibrary,
    LockScreen,
    Screenshot,
    Custom { command: String },
}

impl Action {
    pub fn presets() -> Vec<Action> {
        vec![
            Action::None,
            Action::ShowWindows,
            Action::ShowDesktop,
            Action::Launcher,
            Action::AppLibrary,
            Action::LockScreen,
            Action::Screenshot,
            Action::Custom {
                command: String::new(),
            },
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Action::None => "Disabled",
            Action::ShowWindows => "Show Windows / Workspaces",
            Action::ShowDesktop => "Show Desktop",
            Action::Launcher => "Application Launcher",
            Action::AppLibrary => "App Library",
            Action::LockScreen => "Lock Screen",
            Action::Screenshot => "Screenshot",
            Action::Custom { .. } => "Custom Command",
        }
    }

    pub fn is_same_variant(&self, other: &Action) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub fn is_enabled(&self) -> bool {
        match self {
            Action::None => false,
            Action::Custom { command } => !command.trim().is_empty(),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// How long (in milliseconds) the pointer must stay in a corner before triggering.
    pub dwell_ms: u64,
    /// Minimum time (in milliseconds) before the same corner can trigger again.
    pub cooldown_ms: u64,
    /// Size (width and height in logical pixels) of each corner trigger zone.
    pub corner_size_px: u32,
    /// When true, renders subtle tinted squares in active corners for visual testing.
    pub debug_overlay: bool,
    pub top_left: Action,
    pub top_right: Action,
    pub bottom_left: Action,
    pub bottom_right: Action,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dwell_ms: 150,
            cooldown_ms: 600,
            corner_size_px: 8,
            debug_overlay: false,
            top_left: Action::ShowWindows,
            top_right: Action::None,
            bottom_left: Action::None,
            bottom_right: Action::ShowDesktop,
        }
    }
}

impl AppConfig {
    pub fn action_for(&self, corner: Corner) -> &Action {
        match corner {
            Corner::TopLeft => &self.top_left,
            Corner::TopRight => &self.top_right,
            Corner::BottomLeft => &self.bottom_left,
            Corner::BottomRight => &self.bottom_right,
        }
    }

    pub fn action_for_mut(&mut self, corner: Corner) -> &mut Action {
        match corner {
            Corner::TopLeft => &mut self.top_left,
            Corner::TopRight => &mut self.top_right,
            Corner::BottomLeft => &mut self.bottom_left,
            Corner::BottomRight => &mut self.bottom_right,
        }
    }

    pub fn config_path() -> Result<PathBuf> {
        let base = dirs::config_dir().context("Could not determine XDG config directory")?;
        Ok(base.join("open-corners").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;
        let mut cfg: AppConfig = toml::from_str(&raw)
            .with_context(|| format!("Failed to parse config at {}", path.display()))?;
        cfg.corner_size_px = cfg.corner_size_px.clamp(2, 64);
        Ok(cfg)
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::config_path()?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }
        let serialized = toml::to_string_pretty(self).context("Failed to serialize config")?;
        let tmp_path = path.with_extension("toml.tmp");
        fs::write(&tmp_path, serialized)
            .with_context(|| format!("Failed to write temp config {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to move config into place at {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml_config() {
        let mut cfg = AppConfig::default();
        cfg.dwell_ms = 200;
        cfg.top_right = Action::Custom {
            command: "notify-send 'Hello corner'".into(),
        };
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn parse_corner_aliases() {
        assert_eq!("top-left".parse::<Corner>().unwrap(), Corner::TopLeft);
        assert_eq!("BR".parse::<Corner>().unwrap(), Corner::BottomRight);
        assert!("middle".parse::<Corner>().is_err());
    }
}
