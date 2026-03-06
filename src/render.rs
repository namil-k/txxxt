use crate::charsets::{CharsetName, EDGE_CHARS};

/// Rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Normal,
    Outline,
}

/// Render configuration.
pub struct RenderConfig {
    pub mode: RenderMode,
    pub charset: CharsetName,
    pub color: bool,
    pub brightness_threshold: u8,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            mode: RenderMode::Normal,
            charset: CharsetName::Standard,
            color: false,
            brightness_threshold: 10,
        }
    }
}

/// A single rendered ASCII cell, optionally with color.
#[derive(Clone, Debug)]
pub struct AsciiCell {
    pub ch: char,
    pub color: Option<(u8, u8, u8)>, // RGB color if color mode is on
}

/// Render an RGB frame to ASCII.
/// Returns a 2D grid of AsciiCells (rows × cols).
pub fn render_frame(
    rgb: &[u8],
    img_width: u32,
    img_height: u32,
    cols: u16,
    rows: u16,
    config: &RenderConfig,
) -> Vec<Vec<AsciiCell>> {
    let cols = cols as u32;
    let rows = rows as u32;
    if cols == 0 || rows == 0 || img_width == 0 || img_height == 0 {
        return vec![];
    }

    // Each terminal cell is roughly 2x taller than wide, so we sample accordingly.
    let cell_w = img_width as f32 / cols as f32;
    let cell_h = img_height as f32 / rows as f32;

    let charset = config.charset.chars();

    // Build grayscale buffer for edge detection if needed.
    let gray: Vec<u8> = if config.mode == RenderMode::Outline {
        let raw = rgb_to_gray(rgb, img_width, img_height);
        gaussian_blur_3x3(&raw, img_width, img_height)
    } else {
        vec![]
    };

    let mut result = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut line = Vec::with_capacity(cols as usize);
        for col in 0..cols {
            // Compute cell pixel bounds.
            let x0 = ((col as f32) * cell_w) as u32;
            let y0 = ((row as f32) * cell_h) as u32;
            let x1 = (((col + 1) as f32) * cell_w) as u32;
            let y1 = (((row + 1) as f32) * cell_h) as u32;
            let x1 = x1.min(img_width);
            let y1 = y1.min(img_height);

            // RMS average of all pixels in the cell.
            let (r, g, b) = rms_sample(rgb, img_width, x0, y0, x1, y1);

            // Center pixel for edge detection.
            let px = ((x0 + x1) / 2).min(img_width - 1);
            let py = ((y0 + y1) / 2).min(img_height - 1);

            let brightness = luminance(r, g, b);

            let ch = if brightness < config.brightness_threshold {
                ' '
            } else {
                match config.mode {
                    RenderMode::Normal => brightness_to_char(brightness, charset),
                    RenderMode::Outline => {
                        edge_char(&gray, img_width, img_height, px, py, config.brightness_threshold)
                    }
                }
            };

            let color = if config.color && ch != ' ' {
                Some((r, g, b))
            } else {
                None
            };

            line.push(AsciiCell { ch, color });
        }
        result.push(line);
    }

    result
}

/// RMS average RGB over a rectangular cell region.
fn rms_sample(rgb: &[u8], w: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> (u8, u8, u8) {
    if x0 >= x1 || y0 >= y1 {
        let cx = x0.min(w - 1);
        let idx = (y0 * w + cx) as usize * 3;
        return (rgb[idx], rgb[idx + 1], rgb[idx + 2]);
    }
    let mut sum_r: f32 = 0.0;
    let mut sum_g: f32 = 0.0;
    let mut sum_b: f32 = 0.0;
    let mut count: f32 = 0.0;
    for py in y0..y1 {
        for px in x0..x1 {
            let idx = (py * w + px) as usize * 3;
            let r = rgb[idx] as f32;
            let g = rgb[idx + 1] as f32;
            let b = rgb[idx + 2] as f32;
            sum_r += r * r;
            sum_g += g * g;
            sum_b += b * b;
            count += 1.0;
        }
    }
    (
        (sum_r / count).sqrt() as u8,
        (sum_g / count).sqrt() as u8,
        (sum_b / count).sqrt() as u8,
    )
}

/// 3×3 Gaussian blur (σ ≈ 6.4, artem-style) on a grayscale buffer.
fn gaussian_blur_3x3(gray: &[u8], w: u32, h: u32) -> Vec<u8> {
    // Kernel: [1, 2, 1] / 4  (separable)
    let len = (w * h) as usize;
    let mut tmp = vec![0u8; len];
    let mut out = vec![0u8; len];

    // Horizontal pass
    for y in 0..h {
        for x in 0..w {
            let l = if x > 0 { gray[(y * w + x - 1) as usize] as u32 } else { gray[(y * w + x) as usize] as u32 };
            let c = gray[(y * w + x) as usize] as u32;
            let r = if x + 1 < w { gray[(y * w + x + 1) as usize] as u32 } else { c };
            tmp[(y * w + x) as usize] = ((l + 2 * c + r) / 4) as u8;
        }
    }
    // Vertical pass
    for y in 0..h {
        for x in 0..w {
            let u = if y > 0 { tmp[((y - 1) * w + x) as usize] as u32 } else { tmp[(y * w + x) as usize] as u32 };
            let c = tmp[(y * w + x) as usize] as u32;
            let d = if y + 1 < h { tmp[((y + 1) * w + x) as usize] as u32 } else { c };
            out[(y * w + x) as usize] = ((u + 2 * c + d) / 4) as u8;
        }
    }
    out
}

/// Convert brightness (0–255) to a character from the charset.
fn brightness_to_char(brightness: u8, charset: &[char]) -> char {
    let idx = (brightness as usize * (charset.len() - 1)) / 255;
    charset[idx]
}

/// Rec. 709 luminance.
fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) as u8
}

/// Convert RGB buffer to grayscale.
fn rgb_to_gray(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let len = (w * h) as usize;
    let mut gray = Vec::with_capacity(len);
    for i in 0..len {
        let idx = i * 3;
        gray.push(luminance(rgb[idx], rgb[idx + 1], rgb[idx + 2]));
    }
    gray
}

/// Sobel edge detection at a single pixel, returning a direction-mapped character.
fn edge_char(gray: &[u8], w: u32, h: u32, x: u32, y: u32, threshold: u8) -> char {
    if x == 0 || y == 0 || x >= w - 1 || y >= h - 1 {
        return ' ';
    }

    let g = |dx: i32, dy: i32| -> f32 {
        let px = (x as i32 + dx) as u32;
        let py = (y as i32 + dy) as u32;
        gray[(py * w + px) as usize] as f32
    };

    // Sobel kernels
    let gx = -g(-1, -1) + g(1, -1) - 2.0 * g(-1, 0) + 2.0 * g(1, 0) - g(-1, 1) + g(1, 1);
    let gy = -g(-1, -1) - 2.0 * g(0, -1) - g(1, -1) + g(-1, 1) + 2.0 * g(0, 1) + g(1, 1);

    let magnitude = (gx * gx + gy * gy).sqrt();
    if magnitude < threshold as f32 * 2.0 {
        return ' ';
    }

    // Map gradient direction to edge character.
    // atan2 gives angle, we quantize to 4 directions.
    let angle = gy.atan2(gx).to_degrees();
    let angle = if angle < 0.0 { angle + 180.0 } else { angle };

    match angle as u32 {
        0..=22 | 158..=180 => EDGE_CHARS[0],   // ─ horizontal
        23..=67 => EDGE_CHARS[1],                // ╱ diagonal
        68..=112 => EDGE_CHARS[2],               // │ vertical
        113..=157 => EDGE_CHARS[3],              // ╲ diagonal
        _ => EDGE_CHARS[0],
    }
}
