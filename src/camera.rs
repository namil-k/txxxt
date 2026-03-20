use anyhow::{Context, Result};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

/// Camera source — either a real hardware camera or a dummy test pattern.
enum CameraSource {
    Real(Camera),
    Dummy { frame_counter: u64 },
}

/// Wraps nokhwa Camera for simplified frame capture.
pub struct CameraCapture {
    source: CameraSource,
    width: u32,
    height: u32,
}

impl CameraCapture {
    /// Open default camera (index 0). Lets the camera pick the best format.
    pub fn new(_width: u32, _height: u32) -> Result<Self> {
        let index = CameraIndex::Index(0);
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);

        let mut camera =
            Camera::new(index, requested).context("Failed to open camera")?;
        camera.open_stream().context("Failed to open camera stream")?;

        let actual_res = camera.resolution();
        Ok(Self {
            source: CameraSource::Real(camera),
            width: actual_res.width(),
            height: actual_res.height(),
        })
    }

    /// Create a dummy camera that generates animated test patterns.
    /// Useful for testing P2P calls on a single machine.
    pub fn dummy(width: u32, height: u32) -> Self {
        Self {
            source: CameraSource::Dummy { frame_counter: 0 },
            width,
            height,
        }
    }

    /// Capture a single frame as RGB bytes (width * height * 3).
    pub fn frame_rgb(&mut self) -> Result<(Vec<u8>, u32, u32)> {
        match &mut self.source {
            CameraSource::Real(camera) => {
                let buffer = camera.frame().context("Failed to capture frame")?;
                let decoded = buffer
                    .decode_image::<RgbFormat>()
                    .context("Failed to decode frame")?;
                let (w, h) = (decoded.width(), decoded.height());
                Ok((decoded.into_raw(), w, h))
            }
            CameraSource::Dummy { frame_counter } => {
                let (w, h) = (self.width, self.height);
                let frame = *frame_counter;
                *frame_counter += 1;
                Ok((generate_test_pattern(w, h, frame), w, h))
            }
        }
    }

    #[allow(dead_code)]
    pub fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns true if this is a dummy (test pattern) camera.
    #[allow(dead_code)]
    pub fn is_dummy(&self) -> bool {
        matches!(self.source, CameraSource::Dummy { .. })
    }
}

/// Generate an animated RGB test pattern.
/// Creates moving diagonal bars with varying brightness and color.
fn generate_test_pattern(w: u32, h: u32, frame: u64) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let offset = frame as u32;

    for y in 0..h {
        for x in 0..w {
            // Moving diagonal bands
            let diag = (x + y + offset * 3) % 80;
            let intensity = if diag < 40 {
                (diag as f32 / 40.0 * 255.0) as u8
            } else {
                ((80 - diag) as f32 / 40.0 * 255.0) as u8
            };

            // Add some color variation based on position
            let zone = ((x + offset * 2) / (w / 4).max(1)) % 4;
            let (r, g, b) = match zone {
                0 => (intensity, intensity / 3, intensity / 5),     // warm
                1 => (intensity / 5, intensity, intensity / 3),     // green
                2 => (intensity / 3, intensity / 4, intensity),     // blue
                _ => (intensity, intensity / 2, intensity),         // purple
            };
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
    }
    rgb
}
