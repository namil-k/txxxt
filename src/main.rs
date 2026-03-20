mod audio;
mod background;
mod camera;
mod charsets;
mod config;
mod export;
mod net;
mod render;
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
    /// Start a 1:1 video call (caller)
    Call {
        /// Address to connect to (e.g., 192.168.1.100:7878)
        addr: String,
    },
    /// Listen for incoming video calls
    Listen {
        /// Port to listen on
        #[arg(short, long, default_value_t = 7878)]
        port: u16,
    },
    /// Update txxxt to the latest version
    Update,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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
        Some(Commands::Call { addr }) => {
            // Connect directly from CLI, then enter TUI.
            net::peer::run_caller(&addr, camera)?;
        }
        Some(Commands::Listen { port }) => {
            // Listen from CLI, then enter TUI on connection.
            net::peer::run_listener(port, camera)?;
        }
        Some(Commands::Update) => {
            self_update()?;
        }
    }

    Ok(())
}

fn self_update() -> Result<()> {
    use std::process::Command;

    println!("current version: {}", env!("CARGO_PKG_VERSION"));
    println!("checking for updates...");

    let install_script = "https://raw.githubusercontent.com/kimnam1/txxxt/main/install.sh";

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
