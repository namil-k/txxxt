/// Running average background model for foreground detection.
///
/// Maintains a per-pixel background estimate using exponential moving average.
/// Pixels that differ significantly from the background are flagged as foreground.
///
/// Usage:
/// 1. Feed frames via `update()` for the first ~30 frames to initialize the background.
/// 2. After `is_ready()` returns true, call `foreground_mask()` to get a per-pixel mask.
pub struct BackgroundModel {
    /// Background as f32 grayscale buffer (width * height).
    background: Vec<f32>,
    /// Image dimensions.
    width: u32,
    height: u32,
    /// Exponential moving average alpha (0.0–1.0).
    /// Lower = slower background adaptation (0.05 = ~20 frame lag).
    alpha: f32,
    /// Number of frames fed so far.
    frame_count: u32,
    /// Frames needed before the model is considered initialized.
    warmup_frames: u32,
    /// Pixel difference threshold to classify as foreground (0–255 scale).
    fg_threshold: f32,
}

impl BackgroundModel {
    /// Create a new background model.
    ///
    /// - `alpha`: background learning rate (0.02–0.1 is typical; 0.05 is a good default)
    /// - `fg_threshold`: pixel difference to count as foreground (default 25)
    pub fn new(width: u32, height: u32, alpha: f32, fg_threshold: f32) -> Self {
        let size = (width * height) as usize;
        Self {
            background: vec![128.0; size],
            width,
            height,
            alpha,
            frame_count: 0,
            warmup_frames: 20,
            fg_threshold,
        }
    }

    /// Returns true once enough frames have been fed to initialize the background.
    pub fn is_ready(&self) -> bool {
        self.frame_count >= self.warmup_frames
    }

    /// Feed a new RGB frame to update the background model.
    pub fn update(&mut self, rgb: &[u8]) {
        let size = (self.width * self.height) as usize;
        if rgb.len() < size * 3 {
            return;
        }
        if self.frame_count == 0 {
            // Initialize background with first frame.
            for i in 0..size {
                let idx = i * 3;
                self.background[i] = luminance(rgb[idx], rgb[idx + 1], rgb[idx + 2]) as f32;
            }
        } else {
            // Exponential moving average update.
            for i in 0..size {
                let idx = i * 3;
                let gray = luminance(rgb[idx], rgb[idx + 1], rgb[idx + 2]) as f32;
                self.background[i] = self.background[i] * (1.0 - self.alpha) + gray * self.alpha;
            }
        }
        self.frame_count += 1;
    }

    /// Returns a boolean mask (true = foreground) for the current RGB frame.
    /// Call `update()` before this each frame.
    pub fn foreground_mask(&self, rgb: &[u8]) -> Vec<bool> {
        let size = (self.width * self.height) as usize;
        let mut mask = vec![false; size];
        if rgb.len() < size * 3 || !self.is_ready() {
            // During warmup: treat everything as foreground so the screen isn't blank.
            return vec![true; size];
        }
        for i in 0..size {
            let idx = i * 3;
            let gray = luminance(rgb[idx], rgb[idx + 1], rgb[idx + 2]) as f32;
            let diff = (gray - self.background[i]).abs();
            mask[i] = diff > self.fg_threshold;
        }
        // Dilate the mask slightly so thin outlines aren't clipped.
        dilate_mask(&mask, self.width, self.height, 2)
    }

    /// Resize the model when terminal dimensions change.
    pub fn reset_if_size_changed(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            *self = BackgroundModel::new(width, height, self.alpha, self.fg_threshold);
        }
    }
}

/// Rec. 709 luminance.
fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) as u8
}

/// Binary dilation: expand each `true` pixel by `radius` pixels.
/// This prevents thin edges from being cut off at the foreground boundary.
fn dilate_mask(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    let size = (w * h) as usize;
    let mut out = vec![false; size];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if mask[(y * w as i32 + x) as usize] {
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx >= 0 && nx < w as i32 && ny >= 0 && ny < h as i32 {
                            out[(ny * w as i32 + nx) as usize] = true;
                        }
                    }
                }
            }
        }
    }
    out
}
