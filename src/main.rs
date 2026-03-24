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
    /// Register a username (txxxt+ only)
    Register {
        /// Desired username (3-20 alphanumeric chars)
        username: String,
    },
    /// Log in and save session token
    Login,
    /// Manage friends
    Friends {
        #[command(subcommand)]
        action: FriendsAction,
    },
    /// Convert an image to ASCII art
    Convert {
        /// Path to image file (png, jpg, webp, etc.)
        file: String,

        /// Visual style
        #[arg(short, long, default_value = "standard",
              value_parser = ["standard", "letters", "dots", "digits", "blocks",
                              "hangul", "hiragana", "katakana", "hanja"])]
        style: String,

        /// Output file (html, txt, or ansi). Prints to terminal if omitted.
        #[arg(short, long)]
        output: Option<String>,

        /// Width in columns (default: terminal width)
        #[arg(short, long)]
        width: Option<u16>,

        /// Enable color output
        #[arg(short, long)]
        color: bool,

        /// Rotate image (0, 90, 180, 270)
        #[arg(long, default_value = "0")]
        rotate: u16,

        /// Mirror (flip horizontally)
        #[arg(long)]
        mirror: bool,

        /// Enable background removal (requires txxxt+)
        #[arg(long)]
        bg: bool,
    },
}

#[derive(Subcommand)]
enum FriendsAction {
    /// Add a friend by username
    Add { username: String },
    /// Remove a friend by username
    Remove { username: String },
    /// List all friends
    List,
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
        Some(Commands::Register { username }) => {
            return cmd_register(username);
        }
        Some(Commands::Login) => {
            return cmd_login();
        }
        Some(Commands::Friends { action }) => {
            return cmd_friends(action);
        }
        Some(Commands::Convert { file, style, output, width, color, rotate, mirror, bg }) => {
            return cmd_convert(file, style, output, width, color, rotate, mirror, bg);
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

    // Start presence listener if logged in.
    let incoming_rx = config::get_account()
        .map(|(_, token)| net::presence::start_presence(&token));

    match cli.command {
        None if cli.code.is_some() => {
            // Check for updates before joining a call.
            if let Some(latest) = check_version() {
                eprintln!("update available: v{} → v{}", env!("CARGO_PKG_VERSION"), latest);
                eprintln!("run 'txxxt update' to upgrade.\n");
            }
            let code = cli.code.unwrap();
            // Direct call: txxxt @username
            if let Some(target) = code.strip_prefix('@') {
                let account = config::get_account()
                    .ok_or_else(|| anyhow::anyhow!("not logged in. run: txxxt login"))?;
                let (_username, token) = account;
                tui::run_viewer_with_code(camera, &net::relay::call_user(&token, target)?, incoming_rx)?;
            } else {
                tui::run_viewer_with_code(camera, &code, incoming_rx)?;
            }
        }
        None => {
            // Default: local ASCII webcam viewer (can start calls from TUI).
            tui::run_viewer(camera, incoming_rx)?;
        }
        _ => unreachable!(), // All commands handled above.
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
        println!("txxxt+ activated ✓");
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

/// Register a username with the relay server.
fn cmd_register(username: &str) -> Result<()> {
    let key = config::load()
        .license_key
        .ok_or_else(|| anyhow::anyhow!("no license key found. run: txxxt activate <KEY>"))?;

    println!("registering username @{}...", username);
    let (un, tok) = net::relay::register(&key, username)?;
    config::save_account(&un, &tok);
    println!("registered and logged in as @{}", un);
    Ok(())
}

/// Log in with the saved license key.
fn cmd_login() -> Result<()> {
    let key = config::load()
        .license_key
        .ok_or_else(|| anyhow::anyhow!("no license key found. run: txxxt activate <KEY>"))?;

    let (username, token) = net::relay::login(&key)?;
    config::save_account(&username, &token);
    println!("logged in as @{}", username);
    Ok(())
}

/// Manage friends.
fn cmd_friends(action: &FriendsAction) -> Result<()> {
    let (_, token) = config::get_account()
        .ok_or_else(|| anyhow::anyhow!("not logged in. run: txxxt login"))?;

    match action {
        FriendsAction::List => {
            let friends = net::relay::friends_list(&token)?;
            if friends.is_empty() {
                println!("no friends yet");
            } else {
                for name in &friends {
                    println!("@{}", name);
                }
            }
        }
        FriendsAction::Add { username } => {
            net::relay::friends_add(&token, username)?;
            println!("added @{}", username);
        }
        FriendsAction::Remove { username } => {
            net::relay::friends_remove(&token, username)?;
            println!("removed @{}", username);
        }
    }
    Ok(())
}

fn cmd_convert(
    file: &str, style: &str, output: &Option<String>, width: &Option<u16>,
    color: &bool, rotate: &u16, mirror: &bool, bg: &bool,
) -> Result<()> {
    use render::{render_frame, RenderConfig, RenderMode, BgMode};
    use charsets::CharsetName;

    // Load image.
    let path = std::path::Path::new(file);
    if !path.exists() {
        anyhow::bail!("file not found: {}", file);
    }
    let img = image::open(path)?;

    // Apply EXIF orientation.
    let img = tui::App::apply_exif_orientation(path, img);

    // Apply rotation.
    let img = match rotate {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    };

    // Apply mirror.
    let img = if *mirror { img.fliph() } else { img };

    let rgb_img = img.to_rgb8();
    let (w, h) = rgb_img.dimensions();
    let rgb = rgb_img.into_raw();

    // Parse charset.
    let charset = match style {
        "standard" => CharsetName::Standard,
        "letters" => CharsetName::Letters,
        "dots" => CharsetName::Dots,
        "digits" => CharsetName::Digits,
        "blocks" => CharsetName::Blocks,
        "hangul" => CharsetName::Hangul,
        "hiragana" => CharsetName::Hiragana,
        "katakana" => CharsetName::Katakana,
        "hanja" => CharsetName::Hanja,
        _ => anyhow::bail!("unknown style: {}", style),
    };

    // Determine output width.
    let cols = width.unwrap_or_else(|| {
        crossterm::terminal::size().map(|(c, _)| c).unwrap_or(120)
    });
    let view_cols = if charset.is_wide() { cols / 2 } else { cols };

    // Calculate rows from aspect ratio.
    let cell_aspect = 2.0f32;
    let img_aspect = w as f32 / h as f32;
    let view_rows = (view_cols as f32 / img_aspect / cell_aspect).round() as u16;

    let config = RenderConfig {
        mode: RenderMode::Normal,
        charset,
        color: *color,
        brightness_threshold: 85,
        gamma: 1.0,
        bg_mode: if *bg { BgMode::Person } else { BgMode::Off },
        mirror: false,
        contour: false,
    };

    // Background removal.
    let fg_mask: Option<Vec<bool>> = if *bg {
        let model_path = segmentation::default_model_path();
        if !model_path.exists() {
            anyhow::bail!("segmentation model not found. run txxxt first to download it.");
        }
        let seg = segmentation::Segmenter::new(&model_path)?;
        seg.send_frame(&rgb, w, h);
        // Wait for mask.
        std::thread::sleep(std::time::Duration::from_millis(500));
        seg.try_recv_mask()
    } else {
        None
    };

    let grid = render_frame(&rgb, w, h, view_cols, view_rows, &config, fg_mask.as_deref());

    // Output.
    match output {
        Some(out_path) => {
            let content = if out_path.ends_with(".html") {
                export::grid_to_html(&grid)
            } else if out_path.ends_with(".txt") {
                export::grid_to_text(&grid)
            } else if out_path.ends_with(".ansi") {
                export::grid_to_ansi(&grid)
            } else {
                // Default to HTML.
                export::grid_to_html(&grid)
            };
            std::fs::write(out_path, content)?;
            println!("saved: {}", out_path);
        }
        None => {
            // Print to terminal.
            if *color {
                print!("{}", export::grid_to_ansi(&grid));
            } else {
                print!("{}", export::grid_to_text(&grid));
            }
        }
    }

    Ok(())
}

