mod background;
mod camera;
mod charsets;
mod render;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "txxxt", version, about = "Terminal ASCII video — webcam viewer & video call")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            // Default: local ASCII webcam viewer
            println!("Starting txxxt webcam viewer...");
            #[cfg(target_os = "macos")]
            nokhwa::nokhwa_initialize(|granted| {
                if !granted {
                    eprintln!("Camera permission denied");
                    std::process::exit(1);
                }
            });

            let camera = camera::CameraCapture::new(640, 480)?;
            tui::run_viewer(camera)?;
        }
        Some(Commands::Call { addr }) => {
            eprintln!("Video call to {} — not yet implemented (Phase 2)", addr);
        }
        Some(Commands::Listen { port }) => {
            eprintln!("Listening on port {} — not yet implemented (Phase 2)", port);
        }
    }

    Ok(())
}
