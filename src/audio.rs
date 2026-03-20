use std::sync::mpsc;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

/// Start capturing audio from the default input device using its default config.
/// Sends PCM i16 mono chunks to the returned receiver.
/// Returns (stream_handle, receiver, sample_rate).
pub fn start_capture() -> Result<(Stream, mpsc::Receiver<Vec<i16>>, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no audio input device found")?;

    let default_config = device
        .default_input_config()
        .context("failed to get default input config")?;

    let sample_rate = default_config.sample_rate().0;
    let channels = default_config.channels() as usize;
    let sample_format = default_config.sample_format();

    let config: cpal::StreamConfig = default_config.into();
    let (tx, rx) = mpsc::channel::<Vec<i16>>();

    let stream = match sample_format {
        cpal::SampleFormat::I16 => {
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    // Downmix to mono if multi-channel.
                    let mono = downmix_i16(data, channels);
                    let _ = tx.send(mono);
                },
                |err| eprintln!("audio input error: {}", err),
                None,
            )
        }
        cpal::SampleFormat::F32 => {
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let mono = downmix_f32_to_i16(data, channels);
                    let _ = tx.send(mono);
                },
                |err| eprintln!("audio input error: {}", err),
                None,
            )
        }
        _ => {
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let mono = downmix_f32_to_i16(data, channels);
                    let _ = tx.send(mono);
                },
                |err| eprintln!("audio input error: {}", err),
                None,
            )
        }
    }
    .context("failed to build input stream")?;

    stream.play().context("failed to start audio capture")?;
    Ok((stream, rx, sample_rate))
}

/// Start audio playback on the default output device.
/// Returns (stream_handle, sender, sample_rate).
pub fn start_playback() -> Result<(Stream, mpsc::Sender<Vec<i16>>, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no audio output device found")?;

    let default_config = device
        .default_output_config()
        .context("failed to get default output config")?;

    let sample_rate = default_config.sample_rate().0;
    let channels = default_config.channels() as usize;
    let config: cpal::StreamConfig = default_config.into();

    let (tx, rx) = mpsc::channel::<Vec<i16>>();

    // Ring buffer for smooth playback.
    let ring = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::<i16>::with_capacity(sample_rate as usize),
    ));
    let ring_writer = ring.clone();

    // Consumer thread: move received chunks into ring buffer.
    std::thread::spawn(move || {
        while let Ok(samples) = rx.recv() {
            if let Ok(mut buf) = ring_writer.lock() {
                buf.extend(samples);
                // Cap buffer to ~500ms to avoid growing unbounded.
                let max = sample_rate as usize / 2;
                while buf.len() > max {
                    buf.pop_front();
                }
            }
        }
    });

    let ring_reader = ring;
    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                if let Ok(mut buf) = ring_reader.lock() {
                    for frame in data.chunks_mut(channels) {
                        let sample = buf
                            .pop_front()
                            .map(|s| s as f32 / 32767.0)
                            .unwrap_or(0.0);
                        // Upmix mono to all output channels.
                        for ch in frame.iter_mut() {
                            *ch = sample;
                        }
                    }
                } else {
                    for sample in data.iter_mut() {
                        *sample = 0.0;
                    }
                }
            },
            |err| eprintln!("audio output error: {}", err),
            None,
        )
        .context("failed to build output stream")?;

    stream.play().context("failed to start audio playback")?;
    Ok((stream, tx, sample_rate))
}

// ─── Echo Canceller ─────────────────────────────────────────────────────────

use std::sync::Arc;
use webrtc_audio_processing::Processor;

/// WebRTC-based echo canceller.
///
/// Feed render (speaker) frames via `process_render()` and
/// process capture (mic) frames via `process_capture()`.
/// Both methods handle buffering into the required 10ms frames internally.
pub struct EchoCanceller {
    processor: Arc<Processor>,
    #[allow(dead_code)]
    sample_rate: u32,
    frame_size: usize, // samples per 10ms frame
    capture_buf: Vec<f32>,
    render_buf: Vec<f32>,
}

impl EchoCanceller {
    /// Create a new echo canceller at the given sample rate.
    pub fn new(sample_rate: u32) -> Result<Self> {
        use webrtc_audio_processing::Config;

        let processor = Processor::new(sample_rate)
            .map_err(|e| anyhow::anyhow!("AEC init error: {:?}", e))?;

        processor.set_config(Config {
            echo_canceller: Some(Default::default()),
            ..Default::default()
        });

        let frame_size = (sample_rate / 100) as usize; // 10ms

        Ok(Self {
            processor: Arc::new(processor),
            sample_rate,
            frame_size,
            capture_buf: Vec::with_capacity(frame_size * 2),
            render_buf: Vec::with_capacity(frame_size * 2),
        })
    }

    /// Feed speaker (render) audio so AEC knows what's being played.
    /// Input: mono i16 samples. Can be any length — internally buffered to 10ms frames.
    pub fn analyze_render(&mut self, samples: &[i16]) {
        // Convert i16 → f32 and append to buffer.
        for &s in samples {
            self.render_buf.push(s as f32 / 32767.0);
        }

        // Process complete 10ms frames.
        while self.render_buf.len() >= self.frame_size {
            let frame_data: Vec<f32> = self.render_buf.drain(..self.frame_size).collect();
            let frame = vec![frame_data]; // mono: 1 channel
            // analyze_render_frame doesn't modify the data.
            let _ = self.processor.process_render_frame(&mut frame.clone());
        }
    }

    /// Process mic (capture) audio to remove echo.
    /// Input: mono i16 samples. Returns echo-cancelled mono i16 samples.
    /// May return fewer samples than input (buffering), or more (flushing previous buffer).
    pub fn process_capture(&mut self, samples: &[i16]) -> Vec<i16> {
        // Convert i16 → f32 and append to buffer.
        for &s in samples {
            self.capture_buf.push(s as f32 / 32767.0);
        }

        let mut output = Vec::with_capacity(samples.len());

        // Process complete 10ms frames.
        while self.capture_buf.len() >= self.frame_size {
            let mut frame_data: Vec<f32> = self.capture_buf.drain(..self.frame_size).collect();
            let mut frame = vec![frame_data];
            // process_capture_frame modifies in-place to remove echo.
            if self.processor.process_capture_frame(&mut frame).is_ok() {
                frame_data = frame.into_iter().next().unwrap();
            } else {
                frame_data = self.capture_buf.drain(..0).collect(); // empty on error
                continue;
            }
            // Convert back f32 → i16.
            for &s in &frame_data {
                output.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }

        output
    }
}

// ─── Utility ────────────────────────────────────────────────────────────────

/// Downmix multi-channel i16 to mono by averaging.
fn downmix_i16(data: &[i16], channels: usize) -> Vec<i16> {
    if channels == 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

/// Downmix multi-channel f32 to mono i16.
fn downmix_f32_to_i16(data: &[f32], channels: usize) -> Vec<i16> {
    data.chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().sum();
            let avg = sum / channels as f32;
            (avg.clamp(-1.0, 1.0) * 32767.0) as i16
        })
        .collect()
}
