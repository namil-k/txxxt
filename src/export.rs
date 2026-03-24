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

/// Convert grid to ANSI 24-bit colored text (reserved for future terminal-aware export).
#[allow(dead_code)]
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

/// Copy grid content to system clipboard as plain text.
#[allow(dead_code)]
pub fn yank_to_clipboard(
    clipboard: &mut arboard::Clipboard,
    grid: &[Vec<AsciiCell>],
) -> bool {
    if grid.is_empty() {
        return false;
    }
    clipboard.set_text(grid_to_text(grid)).is_ok()
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

/// Public wrapper for days_to_ymd (used by config.rs).
pub fn days_to_ymd_pub(days: u64) -> (u64, u64, u64) {
    days_to_ymd(days)
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

/// Output directory: ~/Downloads/ (default), configurable in the future.
fn output_dir() -> Option<PathBuf> {
    dirs::download_dir()
}

/// Convert grid to an HTML document with colored spans.
pub fn grid_to_html(grid: &[Vec<AsciiCell>]) -> String {
    let mut out = String::from(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
         <title>txxxt</title></head>\n\
         <body style=\"margin:0;padding:16px;background:#000\">\n\
         <pre style=\"font-family:'Courier New',monospace;font-size:14px;line-height:1.1;color:#ccc\">\n",
    );
    for (i, row) in grid.iter().enumerate() {
        let mut current_color: Option<(u8, u8, u8)> = None;
        for cell in row {
            // HTML-escape the character
            let ch_str = match cell.ch {
                '<' => "&lt;".to_string(),
                '>' => "&gt;".to_string(),
                '&' => "&amp;".to_string(),
                '"' => "&quot;".to_string(),
                c => c.to_string(),
            };
            match (cell.color, current_color) {
                (Some(rgb), cur) if Some(rgb) != cur => {
                    if current_color.is_some() {
                        out.push_str("</span>");
                    }
                    out.push_str(&format!(
                        "<span style=\"color:rgb({},{},{})\">",
                        rgb.0, rgb.1, rgb.2
                    ));
                    current_color = Some(rgb);
                }
                (None, Some(_)) => {
                    out.push_str("</span>");
                    current_color = None;
                }
                _ => {}
            }
            out.push_str(&ch_str);
        }
        if current_color.is_some() {
            out.push_str("</span>");
        }
        if i + 1 < grid.len() {
            out.push('\n');
        }
    }
    out.push_str("\n</pre>\n</body></html>\n");
    out
}

/// Composite a PIP overlay onto a base grid (like the call screen).
/// Returns a new grid with the PIP baked in at the specified position and size.
pub fn composite_pip(
    base: &[Vec<AsciiCell>],
    pip: &[Vec<AsciiCell>],
    pip_x: usize,
    pip_y: usize,
    pip_w: usize,
    pip_h: usize,
) -> Vec<Vec<AsciiCell>> {
    let mut result: Vec<Vec<AsciiCell>> = base.to_vec();

    // Rescale PIP content to fit inside pip area (minus 2 for border).
    let inner_w = pip_w.saturating_sub(2);
    let inner_h = pip_h.saturating_sub(2);
    if inner_w == 0 || inner_h == 0 || pip.is_empty() {
        return result;
    }
    let scaled_pip = crate::net::protocol::rescale_grid(pip, inner_w, inner_h);

    let base_h = result.len();
    let base_w = result.first().map(|r| r.len()).unwrap_or(0);

    // Draw border.
    for dy in 0..pip_h {
        let y = pip_y + dy;
        if y >= base_h {
            break;
        }
        for dx in 0..pip_w {
            let x = pip_x + dx;
            if x >= base_w {
                break;
            }
            let ch = if dy == 0 && dx == 0 {
                '╭'
            } else if dy == 0 && dx == pip_w - 1 {
                '╮'
            } else if dy == pip_h - 1 && dx == 0 {
                '╰'
            } else if dy == pip_h - 1 && dx == pip_w - 1 {
                '╯'
            } else if dy == 0 || dy == pip_h - 1 {
                '─'
            } else if dx == 0 || dx == pip_w - 1 {
                '│'
            } else {
                // Inner area: fill from scaled PIP.
                let iy = dy - 1;
                let ix = dx - 1;
                if iy < scaled_pip.len() && ix < scaled_pip[iy].len() {
                    result[y][x] = scaled_pip[iy][ix].clone();
                    continue;
                } else {
                    ' '
                }
            };
            result[y][x] = AsciiCell { ch, color: None };
        }
    }

    result
}

/// Save grid as HTML.
/// Uses custom `save_dir` if provided, otherwise ~/Downloads.
/// Returns the display path string on success.
pub fn save_to_file(grid: &[Vec<AsciiCell>], save_dir: Option<&str>) -> Result<String, String> {
    if grid.is_empty() {
        return Err("No frame to save".into());
    }

    let dir = match save_dir {
        Some(d) => {
            let expanded = if d.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    PathBuf::from(d.replacen('~', home.to_str().unwrap_or("~"), 1))
                } else {
                    PathBuf::from(d)
                }
            } else {
                PathBuf::from(d)
            };
            expanded
        }
        None => output_dir().ok_or("Cannot determine Downloads directory")?,
    };
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create directory: {}", e))?;

    let filename = format!("txxxt_{}.html", timestamp());
    let path = dir.join(&filename);

    fs::write(&path, grid_to_html(grid)).map_err(|e| format!("Write failed: {}", e))?;

    // Return display path with ~/Downloads shorthand.
    let display = path
        .to_str()
        .map(|s| {
            if let Some(home) = dirs::home_dir().and_then(|h| h.to_str().map(String::from)) {
                s.replace(&home, "~")
            } else {
                s.to_string()
            }
        })
        .unwrap_or(filename);
    Ok(display)
}
