use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::charsets::CharsetName;
use crate::render::{BgMode, RenderConfig};
use crate::tui::VisualStyle;

/// Persisted user settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "default_false")]
    pub color: bool,
    /// Background mode: "off", "motion", or "person".
    #[serde(default = "default_bg_mode")]
    pub bg_mode: String,
    /// Legacy field — migrated to bg_mode on load.
    #[serde(default)]
    pub bg_removal: Option<bool>,
    #[serde(default = "default_true")]
    pub mirror: bool,
    #[serde(default = "default_brightness_threshold")]
    pub brightness_threshold: u8,
    #[serde(default = "default_style")]
    pub style: String,
    /// Custom save directory. None = ~/Downloads.
    #[serde(default)]
    pub save_dir: Option<String>,
}

fn default_false() -> bool {
    false
}
fn default_true() -> bool {
    true
}
fn default_brightness_threshold() -> u8 {
    10
}
fn default_style() -> String {
    "standard".into()
}
fn default_bg_mode() -> String {
    "off".into()
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            color: false,
            bg_mode: "off".into(),
            bg_removal: None,
            mirror: true,
            brightness_threshold: 10,
            style: "standard".into(),
            save_dir: None,
        }
    }
}

fn bg_mode_from_str(s: &str) -> BgMode {
    match s {
        "motion" => BgMode::Motion,
        "person" => BgMode::Person,
        _ => BgMode::Off,
    }
}

fn bg_mode_to_str(mode: BgMode) -> &'static str {
    match mode {
        BgMode::Off => "off",
        BgMode::Motion => "motion",
        BgMode::Person => "person",
    }
}

impl UserConfig {
    /// Resolve the effective BgMode, handling legacy migration.
    pub fn effective_bg_mode(&self) -> BgMode {
        // Migrate legacy bg_removal field.
        if let Some(true) = self.bg_removal {
            if self.bg_mode == "off" {
                return BgMode::Motion;
            }
        }
        bg_mode_from_str(&self.bg_mode)
    }

    /// Build a UserConfig from the current RenderConfig state and existing preferences.
    pub fn from_render_config(config: &RenderConfig, prev: &UserConfig) -> Self {
        let style = VisualStyle::from_config(config);
        Self {
            color: config.color,
            bg_mode: bg_mode_to_str(config.bg_mode).to_string(),
            bg_removal: None, // Don't save legacy field.
            mirror: config.mirror,
            brightness_threshold: config.brightness_threshold,
            style: style.label().to_string(),
            save_dir: prev.save_dir.clone(),
        }
    }

    /// Apply this UserConfig to a RenderConfig.
    pub fn apply_to(&self, config: &mut RenderConfig) {
        config.color = self.color;
        config.bg_mode = self.effective_bg_mode();
        config.mirror = self.mirror;
        config.brightness_threshold = self.brightness_threshold;

        // Resolve style name to VisualStyle.
        let style = VisualStyle::ALL
            .iter()
            .find(|s: &&VisualStyle| s.label() == self.style)
            .copied()
            .unwrap_or(VisualStyle::Charset(CharsetName::Standard));
        style.apply(config);
    }
}

/// Config file path: ~/.config/txxxt/config.toml
fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("txxxt").join("config.toml"))
}

/// Load config from disk. Returns default if file doesn't exist or is invalid.
pub fn load() -> UserConfig {
    let Some(path) = config_path() else {
        return UserConfig::default();
    };
    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

/// Save config to disk. Silently ignores errors.
pub fn save(config: &UserConfig) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = toml::to_string_pretty(config) {
        let _ = fs::write(&path, content);
    }
}
