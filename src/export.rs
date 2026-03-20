use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::render::AsciiCell;

/// Convert grid to plain text (chars only, newline-separated rows).
pub fn grid_to_text(grid: &[Vec<AsciiCell>]) -> String {
    let mut out = String::new();
    for (i, row) in grid.iter().enumerate() {
        for cell in row {
            out.push(cell.ch);
        }
        if i + 1 < grid.len() {
            out.push('\n');
        }
    }
    out
}

/// Convert grid to ANSI 24-bit colored text.
/// Consecutive cells with the same color share a single escape prefix.
pub fn grid_to_ansi(grid: &[Vec<AsciiCell>]) -> String {
    let mut out = String::new();
    for (i, row) in grid.iter().enumerate() {
        let mut current_color: Option<(u8, u8, u8)> = None;
        for cell in row {
            match (cell.color, current_color) {
                (Some(rgb), cur) if Some(rgb) != cur => {
                    if current_color.is_some() {
                        out.push_str("\x1b[0m");
                    }
                    out.push_str(&format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2));
                    current_color = Some(rgb);
                }
                (None, Some(_)) => {
                    out.push_str("\x1b[0m");
                    current_color = None;
                }
                _ => {}
            }
            out.push(cell.ch);
        }
        if current_color.is_some() {
            out.push_str("\x1b[0m");
        }
        if i + 1 < grid.len() {
            out.push('\n');
        }
    }
    out
}

/// Copy grid content to system clipboard.
/// If `color` is true and grid has color data, copies ANSI-colored text.
pub fn yank_to_clipboard(
    clipboard: &mut arboard::Clipboard,
    grid: &[Vec<AsciiCell>],
    color: bool,
) -> bool {
    if grid.is_empty() {
        return false;
    }
    let text = if color {
        grid_to_ansi(grid)
    } else {
        grid_to_text(grid)
    };
    clipboard.set_text(text).is_ok()
}

/// Generate a timestamp string for filenames: YYYYMMDD_HHMMSS
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Manual UTC conversion (avoids chrono dependency for now)
    let secs_per_day = 86400u64;
    let days = now / secs_per_day;
    let day_secs = now % secs_per_day;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Days since epoch to Y/M/D (simplified)
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", year, month, day, hours, minutes, seconds)
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let months_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in &months_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Output directory: ~/Downloads/txxxt/
fn output_dir() -> Option<PathBuf> {
    dirs::download_dir().map(|d| d.join("txxxt"))
}

/// Save grid to a file in ~/Downloads/txxxt/.
/// Returns the filename on success.
pub fn save_to_file(grid: &[Vec<AsciiCell>], color: bool) -> Result<String, String> {
    if grid.is_empty() {
        return Err("No frame to save".into());
    }

    let dir = output_dir().ok_or("Cannot determine Downloads directory")?;
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create directory: {}", e))?;

    let filename = format!("txxxt_{}.txt", timestamp());
    let path = dir.join(&filename);

    let content = if color {
        grid_to_ansi(grid)
    } else {
        grid_to_text(grid)
    };

    fs::write(&path, content).map_err(|e| format!("Write failed: {}", e))?;
    Ok(filename)
}
