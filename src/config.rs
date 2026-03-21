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
    /// txxxt+ license key.
    #[serde(default)]
    pub license_key: Option<String>,
    /// Unix timestamp of last successful license validation.
    #[serde(default)]
    pub license_validated_at: Option<u64>,
    /// Registered username (txxxt+).
    #[serde(default)]
    pub username: Option<String>,
    /// Session token for authenticated relay commands.
    #[serde(default)]
    pub session_token: Option<String>,
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
            license_key: None,
            license_validated_at: None,
            username: None,
            session_token: None,
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
            license_key: prev.license_key.clone(),
            license_validated_at: prev.license_validated_at,
            username: prev.username.clone(),
            session_token: prev.session_token.clone(),
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

/// Check if the user has txxxt+ activated (valid license key saved).
pub fn is_plus() -> bool {
    let config = load();
    config.license_key.is_some() && config.license_validated_at.is_some()
}

/// Save a license key to config with current timestamp.
pub fn save_license_key(key: &str) {
    let mut config = load();
    config.license_key = Some(key.to_string());
    config.license_validated_at = Some(now_unix());
    save(&config);
}

/// Save username and session token to config (after successful LOGIN).
pub fn save_account(username: &str, token: &str) {
    let mut config = load();
    config.username = Some(username.to_string());
    config.session_token = Some(token.to_string());
    save(&config);
}

/// Get saved account (username, token). Returns None if not logged in.
pub fn get_account() -> Option<(String, String)> {
    let config = load();
    match (config.username, config.session_token) {
        (Some(u), Some(t)) => Some((u, t)),
        _ => None,
    }
}

/// Remove license key from config (invalid key).
pub fn revoke_license() {
    let mut config = load();
    config.license_key = None;
    config.license_validated_at = None;
    save(&config);
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Re-validate license key if older than 7 days. Runs on startup.
/// If invalid, revokes the key. If offline, trusts previous validation up to 30 days.
pub fn revalidate_license() {
    let config = load();
    let Some(key) = &config.license_key else { return };
    let Some(validated_at) = config.license_validated_at else {
        // Key exists but was never validated — revoke it.
        revoke_license();
        return;
    };

    let now = now_unix();
    let age_days = (now.saturating_sub(validated_at)) / 86400;

    // Still fresh — no need to re-check.
    if age_days < 7 {
        return;
    }

    // Try to validate via API.
    use std::process::Command;
    let output = Command::new("curl")
        .args([
            "-sSL", "--max-time", "5",
            "-X", "POST",
            "-H", "Content-Type: application/x-www-form-urlencoded",
            "-d", &format!("license_key={}", url_encode(key)),
            "https://api.lemonsqueezy.com/v1/licenses/validate",
        ])
        .output();

    match output {
        Ok(o) => {
            let body = String::from_utf8_lossy(&o.stdout);
            if body.is_empty() {
                // Offline — trust previous validation up to 30 days.
                if age_days > 30 {
                    eprintln!("txxxt+ license expired (offline too long). run: txxxt activate <KEY>");
                    revoke_license();
                }
                return;
            }
            let valid = body.contains("\"valid\":true") || body.contains("\"valid\": true");
            if valid {
                // Refresh timestamp.
                let mut config = load();
                config.license_validated_at = Some(now);
                save(&config);
            } else {
                eprintln!("txxxt+ license no longer valid.");
                revoke_license();
            }
        }
        Err(_) => {
            // Offline — trust up to 30 days.
            if age_days > 30 {
                eprintln!("txxxt+ license expired (offline too long). run: txxxt activate <KEY>");
                revoke_license();
            }
        }
    }
}

/// Simple URL encoding for form data values.
pub fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
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
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!("[config] parse error: {}", e);
            UserConfig::default()
        }),
        Err(_) => UserConfig::default(),
    }
}

/// Save config to disk. Silently ignores errors.
/// Sets restrictive file permissions (0600) to protect license key.
pub fn save(config: &UserConfig) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = toml::to_string_pretty(config) {
        let _ = fs::write(&path, &content);
        // Restrict file permissions (owner-only read/write).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
}
