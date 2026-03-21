# ONNX Person Segmentation

## Overview

Add neural network-based person segmentation to txxxt using MediaPipe Selfie Segmentation via ONNX Runtime. This replaces the motion-based background model (EMA) with pixel-accurate person detection for background removal.

## Architecture

### Threading Model

Segmentation runs on a dedicated thread, decoupled from the main render loop:

```
Main Thread                      ONNX Thread
─────────                        ───────────
camera frame ──send_frame()──→   receive frame
render with previous mask        resize to 256x256
draw to terminal                 normalize to [0,1] f32
                                 ONNX inference (~20ms)
next frame                       threshold → bool mask
poll get_mask() ←────────────── resize to 640x480, send mask
render with new mask
```

The main thread never blocks on inference. If no new mask is ready, the previous one is reused. Mask latency is 1 frame (~66ms at 15fps), imperceptible to the user.

### New Module: `src/segmentation.rs`

```rust
pub struct Segmenter {
    frame_tx: mpsc::SyncSender<FrameData>,
    mask_rx: mpsc::Receiver<Vec<bool>>,
    _thread: std::thread::JoinHandle<()>,
}

struct FrameData {
    rgb: Vec<u8>,
    width: u32,
    height: u32,
}

impl Segmenter {
    /// Load ONNX model and start inference thread.
    /// Returns Err if model file missing or invalid.
    pub fn new(model_path: &Path) -> Result<Self>;

    /// Send a new frame for segmentation (non-blocking, drops if busy).
    pub fn send_frame(&self, rgb: &[u8], width: u32, height: u32);

    /// Poll for the latest mask. Returns None if no new mask available.
    pub fn try_recv_mask(&self) -> Option<Vec<bool>>;
}
```

Key details:
- `SyncSender` with capacity 1: if the ONNX thread is busy, new frames are dropped (latest-wins).
- Model path: `~/.cache/txxxt/models/selfie_segmentation.onnx`

### Inference Pipeline

1. Receive RGB frame (640x480)
2. Bilinear resize to 256x256
3. Normalize to `[0.0, 1.0]` float32, shape `[1, 3, 256, 256]` (NCHW)
4. Run ONNX inference → output shape `[1, 1, 256, 256]` (probability map)
5. Threshold at 0.5 → binary mask
6. Nearest-neighbor resize back to 640x480
7. Send `Vec<bool>` mask to main thread

### BgMode Enum

Replace `RenderConfig.bg_removal: bool` with:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BgMode {
    Off,     // No background removal
    Motion,  // EMA-based foreground detection (free)
    Person,  // ONNX person segmentation (pro)
}
```

### Settings Panel

```
 color:      ON
 background: OFF        ← cycle with ←→
 background: MOTION
 background: PERSON (pro)   ← DarkGray, dimmed
 mirror:     ON
 brightness: ██████░░ 150
```

- Off / Motion: freely selectable
- Person: if model file exists → usable. If not → flash message: `"pro feature — run: txxxt activate <KEY>"`
- `(pro)` label always shown in DarkGray regardless of activation status

### Config Persistence

`~/.config/txxxt/config.toml`:

```toml
bg_mode = "off"  # "off" | "motion" | "person"
```

Replaces the current `bg_removal = true/false`.

## File Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `ort` and `ndarray` dependencies |
| `src/main.rs` | Add `mod segmentation` |
| `src/segmentation.rs` | New: ONNX session, inference thread, Segmenter API |
| `src/render.rs` | `bg_removal: bool` → `bg_mode: BgMode` |
| `src/config.rs` | Serialize/deserialize BgMode, migrate old `bg_removal` field |
| `src/tui.rs` | Settings panel: 3-way background toggle, manage Segmenter lifecycle, poll masks |
| `src/background.rs` | No changes |

## Dependencies

```toml
ort = "2"         # ONNX Runtime bindings (~15MB binary size increase)
ndarray = "0.16"  # Tensor manipulation for model I/O
```

Binary size: 3.5MB → ~18MB. Acceptable for a desktop application.

## Model Details

- Model: MediaPipe Selfie Segmentation (landscape variant)
- Format: ONNX
- Input: `[1, 3, 256, 256]` float32 (RGB, normalized to [0,1])
- Output: `[1, 1, 256, 256]` float32 (per-pixel person probability)
- Size: ~200KB
- Location: `~/.cache/txxxt/models/selfie_segmentation.onnx`

## Pro Feature Gating

Current phase (development):
- Model file present in cache → Person mode works
- Model file absent → Person mode locked with flash message

Future phase (monetization):
- `txxxt activate <KEY>` → server validates key → downloads model to cache
- License stored in `~/.config/txxxt/license`
- No model bundled in binary or source

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Model file missing | Person option dimmed with `(pro)`, flash on select |
| Model file corrupt / load fails | Flash: `"model load failed"`, fall back to Motion |
| ONNX thread crashes | Fall back to Motion, flash: `"segmentation error"` |
| First few frames (no mask yet) | Use all-foreground mask (same as Motion warmup) |

## Testing

- Unit test: `Segmenter::new()` with valid/invalid model paths
- Unit test: mask resize logic (256x256 → 640x480)
- Integration: run with `--dummy` camera flag, verify Person mode produces non-trivial mask
