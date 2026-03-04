use anyhow::{Context, Result};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

/// Wraps nokhwa Camera for simplified frame capture.
pub struct CameraCapture {
    camera: Camera,
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
            camera,
            width: actual_res.width(),
            height: actual_res.height(),
        })
    }

    /// Capture a single frame as RGB bytes (width * height * 3).
    pub fn frame_rgb(&mut self) -> Result<(Vec<u8>, u32, u32)> {
        let buffer = self.camera.frame().context("Failed to capture frame")?;
        let decoded = buffer
            .decode_image::<RgbFormat>()
            .context("Failed to decode frame")?;
        let (w, h) = (decoded.width(), decoded.height());
        Ok((decoded.into_raw(), w, h))
    }

    pub fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
