//! ONNX-based person segmentation using MediaPipe Selfie Segmentation.
//!
//! Runs inference on a dedicated thread. The main render loop sends camera frames
//! and polls for the latest segmentation mask without blocking.

use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result};
use ndarray::Array4;
use ort::session::Session;

/// Frame data sent from the main thread to the ONNX inference thread.
struct FrameData {
    rgb: Vec<u8>,
    width: u32,
    height: u32,
}

/// Person segmenter backed by ONNX Runtime.
///
/// Spawns a background thread that runs inference on incoming camera frames
/// and produces boolean foreground masks.
pub struct Segmenter {
    frame_tx: mpsc::SyncSender<FrameData>,
    mask_rx: mpsc::Receiver<Vec<bool>>,
    _thread: std::thread::JoinHandle<()>,
}

/// Model input size (MediaPipe Selfie Segmentation).
const MODEL_SIZE: u32 = 256;

impl Segmenter {
    /// Create a new segmenter, loading the ONNX model from `model_path`.
    ///
    /// Returns `Err` if the model file is missing or cannot be loaded.
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("ONNX session builder: {}", e))?
            .with_intra_threads(2)
            .map_err(|e| anyhow::anyhow!("ONNX set threads: {}", e))?
            .commit_from_file(model_path)
            .map_err(|e| anyhow::anyhow!("ONNX load model {}: {}", model_path.display(), e))?;

        // Capacity 1: if inference is busy, old frames are dropped (latest-wins).
        let (frame_tx, frame_rx) = mpsc::sync_channel::<FrameData>(1);
        let (mask_tx, mask_rx) = mpsc::sync_channel::<Vec<bool>>(1);

        let handle = std::thread::Builder::new()
            .name("onnx-segmentation".into())
            .spawn(move || {
                inference_loop(session, frame_rx, mask_tx);
            })
            .context("failed to spawn segmentation thread")?;

        Ok(Self {
            frame_tx,
            mask_rx,
            _thread: handle,
        })
    }

    /// Send a new frame for segmentation. Non-blocking: drops the frame if
    /// the inference thread is still busy with the previous one.
    pub fn send_frame(&self, rgb: &[u8], width: u32, height: u32) {
        let _ = self.frame_tx.try_send(FrameData {
            rgb: rgb.to_vec(),
            width,
            height,
        });
    }

    /// Poll for the latest segmentation mask. Returns `None` if no new mask
    /// is available yet.
    pub fn try_recv_mask(&self) -> Option<Vec<bool>> {
        let mut latest = None;
        // Drain all pending masks, keep only the newest.
        while let Ok(mask) = self.mask_rx.try_recv() {
            latest = Some(mask);
        }
        latest
    }
}

/// Background inference loop.
fn inference_loop(
    mut session: Session,
    frame_rx: mpsc::Receiver<FrameData>,
    mask_tx: mpsc::SyncSender<Vec<bool>>,
) {
    while let Ok(frame) = frame_rx.recv() {
        // 1. Resize to MODEL_SIZE x MODEL_SIZE.
        let resized = bilinear_resize_rgb(
            &frame.rgb,
            frame.width,
            frame.height,
            MODEL_SIZE,
            MODEL_SIZE,
        );

        // 2. Normalize to [0, 1] float32, NCHW layout: [1, 3, 256, 256].
        let input = rgb_to_nchw_f32(&resized, MODEL_SIZE, MODEL_SIZE);

        // 3. Build input value from ndarray.
        let input_value = match ort::value::Value::from_array(input) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("segmentation input error: {}", e);
                continue;
            }
        };

        // 4. Run inference.
        let outputs = match session.run(ort::inputs![input_value]) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("segmentation inference error: {}", e);
                continue;
            }
        };

        // 5. Extract output tensor → probability map.
        let output_tensor = match outputs[0].try_extract_tensor::<f32>() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("segmentation output error: {}", e);
                continue;
            }
        };

        // 6. Threshold at 0.5 → bool mask at model resolution.
        let model_mask: Vec<bool> = output_tensor
            .1
            .iter()
            .map(|&p| p > 0.5)
            .collect();

        // 7. Resize mask back to original frame dimensions.
        let mask = resize_mask(
            &model_mask,
            MODEL_SIZE,
            MODEL_SIZE,
            frame.width,
            frame.height,
        );

        // 8. Send mask to main thread (drop if main thread hasn't consumed previous).
        let _ = mask_tx.try_send(mask);
    }
}

/// Convert RGB byte buffer to NCHW float32 tensor normalized to [0, 1].
fn rgb_to_nchw_f32(rgb: &[u8], width: u32, height: u32) -> Array4<f32> {
    let w = width as usize;
    let h = height as usize;
    let mut tensor = Array4::<f32>::zeros((1, 3, h, w));

    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * 3;
            tensor[[0, 0, y, x]] = rgb[idx] as f32 / 255.0;     // R
            tensor[[0, 1, y, x]] = rgb[idx + 1] as f32 / 255.0; // G
            tensor[[0, 2, y, x]] = rgb[idx + 2] as f32 / 255.0; // B
        }
    }

    tensor
}

/// Bilinear resize an RGB buffer.
fn bilinear_resize_rgb(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    let mut dst = vec![0u8; (dst_w * dst_h * 3) as usize];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let sx = dx as f32 * x_ratio;
            let sy = dy as f32 * y_ratio;

            let x0 = sx as u32;
            let y0 = sy as u32;
            let x1 = (x0 + 1).min(src_w - 1);
            let y1 = (y0 + 1).min(src_h - 1);

            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let dst_idx = (dy * dst_w + dx) as usize * 3;

            for c in 0..3 {
                let p00 = src[(y0 * src_w + x0) as usize * 3 + c] as f32;
                let p10 = src[(y0 * src_w + x1) as usize * 3 + c] as f32;
                let p01 = src[(y1 * src_w + x0) as usize * 3 + c] as f32;
                let p11 = src[(y1 * src_w + x1) as usize * 3 + c] as f32;

                let v = p00 * (1.0 - fx) * (1.0 - fy)
                    + p10 * fx * (1.0 - fy)
                    + p01 * (1.0 - fx) * fy
                    + p11 * fx * fy;

                dst[dst_idx + c] = v as u8;
            }
        }
    }

    dst
}

/// Resize a boolean mask using nearest-neighbor.
fn resize_mask(
    mask: &[bool],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<bool> {
    let mut dst = vec![false; (dst_w * dst_h) as usize];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let sx = (dx as f32 * x_ratio) as u32;
            let sy = (dy as f32 * y_ratio) as u32;
            let src_idx = (sy * src_w + sx) as usize;
            let dst_idx = (dy * dst_w + dx) as usize;
            if src_idx < mask.len() {
                dst[dst_idx] = mask[src_idx];
            }
        }
    }

    dst
}

/// Returns the default model file path.
pub fn default_model_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("txxxt")
        .join("models")
        .join("selfie_segmentation.onnx")
}

const MODEL_URL: &str = "https://huggingface.co/onnx-community/mediapipe_selfie_segmentation/resolve/main/selfie_segmentation.onnx";

/// Download the segmentation model in a background thread.
pub fn download_model_bg() {
    std::thread::spawn(|| {
        let path = default_model_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !path.exists() {
            download_file(MODEL_URL, &path, "segmentation");
        }
    });
}

fn download_file(url: &str, path: &std::path::Path, name: &str) {
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(path)
        .arg(url)
        .output();
    match output {
        Ok(o) if o.status.success() => {
            eprintln!("{} model downloaded", name);
        }
        _ => {
            eprintln!("failed to download {} model", name);
            let _ = std::fs::remove_file(path);
        }
    }
}
