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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Check for updates before doing anything (skip for `txxxt update` itself).
    if !matches!(cli.command, Some(Commands::Update)) {
        check_version();
    }

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
            // Join relay room directly: txxxt AXBK
            let code = cli.code.unwrap();
            tui::run_viewer_with_code(camera, &code)?;
        }
        None => {
            // Default: local ASCII webcam viewer (can start calls from TUI).
            tui::run_viewer(camera)?;
        }
        Some(Commands::Update) => {
            self_update()?;
        }
    }

    Ok(())
}

/// Check if current version is latest. If not, auto-update and re-exec.
fn check_version() {
    use std::process::Command;

    // Prevent infinite re-exec loop: if we already checked, skip.
    if std::env::var("TXXXT_UPDATED").is_ok() {
        return;
    }

    let current = env!("CARGO_PKG_VERSION");

    // Fetch latest tag from GitHub API (2 second timeout).
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "2",
               "https://api.github.com/repos/namil-k/txxxt/releases/latest"])
        .output();

    let latest = match output {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            // Parse "tag_name": "v0.4.0" from JSON.
            // After splitting on "tag_name", remainder is: `: "v0.4.0", ...`
            // split('"') → [`: `, `v0.4.0`, `, ...`] → nth(1) = version
            body.split("\"tag_name\"")
                .nth(1)
                .and_then(|s| s.split('"').nth(1))
                .map(|v| v.trim_start_matches('v').to_string())
        }
        _ => None,
    };

    let Some(latest) = latest else { return };

    if latest == current {
        return;
    }

    eprintln!("update available: v{} → v{}", current, latest);
    eprintln!("updating...");

    let status = Command::new("sh")
        .arg("-c")
        .arg("curl -fsSL https://raw.githubusercontent.com/namil-k/txxxt/main/install.sh | bash")
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!("updated! restarting...");
            // Re-exec with same arguments, with guard env to prevent loop.
            let args: Vec<String> = std::env::args().collect();
            let err = Command::new(&args[0])
                .args(&args[1..])
                .env("TXXXT_UPDATED", "1")
                .status();
            std::process::exit(err.map(|s| s.code().unwrap_or(0)).unwrap_or(1));
        }
        _ => {
            eprintln!("update failed, continuing with v{}...", current);
        }
    }
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
