use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::charsets::CharsetName;
use crate::render::RenderConfig;
use crate::tui::VisualStyle;

/// Persisted user settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "default_false")]
    pub color: bool,
    #[serde(default = "default_false")]
    pub bg_removal: bool,
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

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            color: false,
            bg_removal: false,
            mirror: true,
            brightness_threshold: 10,
            style: "standard".into(),
            save_dir: None,
        }
    }
}

impl UserConfig {
    /// Build a UserConfig from the current RenderConfig state and existing preferences.
    pub fn from_render_config(config: &RenderConfig, prev: &UserConfig) -> Self {
        let style = VisualStyle::from_config(config);
        Self {
            color: config.color,
            bg_removal: config.bg_removal,
            mirror: config.mirror,
            brightness_threshold: config.brightness_threshold,
            style: style.label().to_string(),
            save_dir: prev.save_dir.clone(),
        }
    }

    /// Apply this UserConfig to a RenderConfig.
    pub fn apply_to(&self, config: &mut RenderConfig) {
        config.color = self.color;
        config.bg_removal = self.bg_removal;
        config.mirror = self.mirror;
        config.brightness_threshold = self.brightness_threshold;

        // Resolve style name to VisualStyle.
        let style = VisualStyle::ALL
            .iter()
            .find(|s| s.label() == self.style)
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
