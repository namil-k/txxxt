mod audio;
mod background;
mod camera;
mod charsets;
mod config;
mod export;
mod net;
mod render;
mod segmentation;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "txxxt", version, about = "Terminal ASCII video — webcam viewer & video call")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Room code to join (e.g., txxxt AXBK)
    #[arg(index = 1)]
    code: Option<String>,

    /// Use a dummy test pattern instead of the real camera (for testing P2P locally)
    #[arg(long)]
    dummy: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Update txxxt to the latest version
    Update,
    /// Activate txxxt+ with a license key
    Activate {
        /// License key from txxxt.me/plus
        key: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Re-validate txxxt+ license if needed (every 7 days).
    config::revalidate_license();

    // Handle commands that don't need the camera first.
    match &cli.command {
        Some(Commands::Update) => {
            return self_update();
        }
        Some(Commands::Activate { key }) => {
            return activate_plus(key);
        }
        _ => {}
    }

    // Camera-dependent paths below.
    #[cfg(target_os = "macos")]
    nokhwa::nokhwa_initialize(|granted| {
        if !granted {
            eprintln!("Camera permission denied");
            std::process::exit(1);
        }
    });

    let camera = if cli.dummy {
        camera::CameraCapture::dummy(640, 480)
    } else {
        camera::CameraCapture::new(640, 480)?
    };

    match cli.command {
        None if cli.code.is_some() => {
            // Check for updates before joining a call.
            if let Some(latest) = check_version() {
                eprintln!("update available: v{} → v{}", env!("CARGO_PKG_VERSION"), latest);
                eprintln!("run 'txxxt update' to upgrade.\n");
            }
            // Join relay room directly: txxxt AXBK
            let code = cli.code.unwrap();
            tui::run_viewer_with_code(camera, &code)?;
        }
        None => {
            // Default: local ASCII webcam viewer (can start calls from TUI).
            tui::run_viewer(camera)?;
        }
        _ => unreachable!(), // Update/Activate handled above.
    }

    Ok(())
}

/// Check if an update is available. Returns Some(latest_version) if update needed.
/// Does NOT auto-install — callers should notify the user.
pub fn check_version() -> Option<String> {
    use std::process::Command;

    let current = env!("CARGO_PKG_VERSION");

    // Fetch latest tag from GitHub API (2 second timeout).
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "2",
               "https://api.github.com/repos/namil-k/txxxt/releases/latest"])
        .output();

    let latest = match output {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            body.split("\"tag_name\"")
                .nth(1)
                .and_then(|s| s.split('"').nth(1))
                .map(|v| v.trim_start_matches('v').to_string())
        }
        _ => None,
    };

    let latest = latest?;

    if latest == current {
        None
    } else {
        Some(latest)
    }
}

/// Activate txxxt+ by validating a license key via Lemon Squeezy and downloading the model.
fn activate_plus(key: &str) -> Result<()> {
    use std::process::Command;

    println!("activating txxxt+...");

    // 1. Validate license key via Lemon Squeezy API.
    let output = Command::new("curl")
        .args([
            "-sSL", "--max-time", "10",
            "-X", "POST",
            "-H", "Content-Type: application/x-www-form-urlencoded",
            "-d", &format!("license_key={}", config::url_encode(key)),
            "https://api.lemonsqueezy.com/v1/licenses/validate",
        ])
        .output()?;

    let body = String::from_utf8_lossy(&output.stdout);

    if body.is_empty() {
        anyhow::bail!("failed to reach license server. check your internet connection.");
    }

    // Parse "valid": true/false from JSON response.
    let valid = body.contains("\"valid\":true") || body.contains("\"valid\": true");

    if !valid {
        // Try to extract error message.
        let error = body
            .split("\"error\"")
            .nth(1)
            .and_then(|s| s.split('"').nth(1))
            .unwrap_or("invalid license key");
        anyhow::bail!("activation failed: {}", error);
    }

    println!("license valid ✓");

    // Save license key to config.
    config::save_license_key(key);

    // 2. Check if model is already downloaded.
    let model_path = segmentation::default_model_path();

    if model_path.exists() {
        println!("txxxt+ is already activated!");
        println!("model: {}", model_path.display());
        return Ok(());
    }

    // 3. Download the ONNX model from HuggingFace.
    println!("downloading segmentation model...");

    let model_dir = model_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid model path"))?;
    std::fs::create_dir_all(model_dir)?;

    let model_url = "https://huggingface.co/onnx-community/mediapipe_selfie_segmentation/resolve/main/selfie_segmentation.onnx";

    let status = Command::new("curl")
        .args([
            "-fSL", "--max-time", "60",
            "-o", &model_path.to_string_lossy(),
            model_url,
        ])
        .status()?;

    if !status.success() {
        // Clean up partial download.
        let _ = std::fs::remove_file(&model_path);
        anyhow::bail!("failed to download model. check your internet connection.");
    }

    // 4. Verify the file was actually written.
    let metadata = std::fs::metadata(&model_path)?;
    if metadata.len() < 1000 {
        let _ = std::fs::remove_file(&model_path);
        anyhow::bail!("downloaded file appears corrupt (too small). try again.");
    }

    println!("txxxt+ activated! 🎉");
    println!("model saved to: {}", model_path.display());
    println!("\nrestart txxxt to use background (advanced) and contour features.");

    Ok(())
}

fn self_update() -> Result<()> {
    use std::process::Command;

    println!("current version: {}", env!("CARGO_PKG_VERSION"));
    println!("checking for updates...");

    let install_script = "https://raw.githubusercontent.com/namil-k/txxxt/main/install.sh";

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL {} | bash", install_script))
        .status()?;

    if status.success() {
        println!("update complete!");
    } else {
        eprintln!("update failed");
    }

    Ok(())
}
