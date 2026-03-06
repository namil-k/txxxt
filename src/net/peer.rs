use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute,
    terminal,
};
use std::io::{self, Write as IoWrite};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::camera::CameraCapture;
use crate::net::protocol::{decode_frame, encode_frame, AsciiFrame};
use crate::render::{render_frame, AsciiCell, RenderConfig};

/// Shared remote frame buffer (populated by the receive task).
type RemoteFrame = Arc<Mutex<Option<AsciiFrame>>>;

/// Connect to `addr` and start a bidirectional ASCII video call.
pub async fn run_caller(addr: &str, camera: CameraCapture, config: RenderConfig) -> Result<()> {
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("Failed to connect to {addr}"))?;
    let peer_addr = stream.peer_addr().unwrap_or_else(|_| "unknown".parse().unwrap());
    run_call_loop(stream, camera, config, peer_addr, "caller").await
}

/// Listen on `port` and accept the first incoming connection.
pub async fn run_listener(port: u16, camera: CameraCapture, config: RenderConfig) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .with_context(|| format!("Failed to bind port {port}"))?;
    eprintln!("Listening on port {port}...");
    let (stream, peer_addr) = listener.accept().await.context("Accept failed")?;
    eprintln!("Connected: {peer_addr}");
    run_call_loop(stream, camera, config, peer_addr, "listener").await
}

/// Core bidirectional video call loop shared by caller and listener.
async fn run_call_loop(
    stream: TcpStream,
    mut camera: CameraCapture,
    config: RenderConfig,
    peer_addr: SocketAddr,
    mode: &'static str,
) -> Result<()> {
    let remote_frame: RemoteFrame = Arc::new(Mutex::new(None));
    let remote_clone = Arc::clone(&remote_frame);

    let (mut reader, mut writer) = tokio::io::split(stream);

    // Receive task: reads frames from peer and stores in shared buffer.
    let recv_task = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut tmp = [0u8; 8192];
        loop {
            match reader.read(&mut tmp).await {
                Ok(0) => break, // connection closed
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    // Drain complete frames from buffer.
                    loop {
                        match decode_frame(&buf) {
                            Some((frame, consumed)) => {
                                *remote_clone.lock().unwrap() = Some(frame);
                                buf.drain(..consumed);
                            }
                            None => break,
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Terminal setup.
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = call_render_loop(
        &mut camera,
        &config,
        &remote_frame,
        &mut writer,
        peer_addr,
        mode,
    )
    .await;

    // Restore terminal.
    execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    recv_task.abort();
    result
}

/// Main render/send loop: capture → render → send, display split-screen.
async fn call_render_loop(
    camera: &mut CameraCapture,
    config: &RenderConfig,
    remote_frame: &RemoteFrame,
    writer: &mut (impl AsyncWriteExt + Unpin),
    peer_addr: SocketAddr,
    mode: &str,
) -> Result<()> {
    let mut stdout = io::stdout();
    let mut fps_counter = 0u32;
    let mut fps_display = 0u32;
    let mut fps_timer = Instant::now();

    loop {
        // Check for quit key (non-blocking).
        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    break;
                }
            }
        }

        // Get terminal size.
        let (term_cols, term_rows) = terminal::size()?;
        if term_cols < 4 || term_rows < 4 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        // Split screen: left = local, right = remote.
        let half_cols = term_cols / 2;
        let video_rows = term_rows.saturating_sub(2); // reserve 2 rows for status bar

        // Capture and render local frame.
        let local_grid = match camera.frame_rgb() {
            Ok((rgb, w, h)) => render_frame(&rgb, w, h, half_cols, video_rows, config, None),
            Err(_) => vec![],
        };

        // Get the latest remote frame.
        let remote_grid: Vec<Vec<AsciiCell>> = {
            let guard = remote_frame.lock().unwrap();
            match &*guard {
                Some(frame) => decode_ascii_frame(frame, half_cols, video_rows),
                None => vec![],
            }
        };

        // Send local frame to peer.
        let encoded = encode_frame(&local_grid);
        if writer.write_all(&encoded).await.is_err() {
            break;
        }

        // Render split-screen to terminal.
        render_split_screen(
            &mut stdout,
            &local_grid,
            &remote_grid,
            half_cols,
            video_rows,
            term_cols,
            term_rows,
            peer_addr,
            fps_display,
            mode,
        )?;

        fps_counter += 1;
        if fps_timer.elapsed() >= Duration::from_secs(1) {
            fps_display = fps_counter;
            fps_counter = 0;
            fps_timer = Instant::now();
        }

        // ~15 fps cap.
        tokio::time::sleep(Duration::from_millis(66)).await;
    }

    Ok(())
}

/// Decode an `AsciiFrame` into a 2D grid of AsciiCells (no color info).
fn decode_ascii_frame(frame: &AsciiFrame, cols: u16, rows: u16) -> Vec<Vec<AsciiCell>> {
    let fw = frame.width as usize;
    let fh = frame.height as usize;
    if fw == 0 || fh == 0 {
        return vec![];
    }

    // Scale-fit the frame data to the available display area.
    let disp_cols = cols as usize;
    let disp_rows = rows as usize;

    let mut grid = Vec::with_capacity(disp_rows);
    for row in 0..disp_rows {
        let mut line = Vec::with_capacity(disp_cols);
        for col in 0..disp_cols {
            let src_r = (row * fh / disp_rows).min(fh - 1);
            let src_c = (col * fw / disp_cols).min(fw - 1);
            let idx = src_r * fw + src_c;
            let b = frame.data.get(idx).copied().unwrap_or(b' ');
            let ch = if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                ' '
            };
            line.push(AsciiCell { ch, color: None });
        }
        grid.push(line);
    }
    grid
}

/// Draw the split-screen layout to stdout.
#[allow(clippy::too_many_arguments)]
fn render_split_screen(
    stdout: &mut io::Stdout,
    local: &[Vec<AsciiCell>],
    remote: &[Vec<AsciiCell>],
    half_cols: u16,
    video_rows: u16,
    term_cols: u16,
    term_rows: u16,
    peer_addr: SocketAddr,
    fps: u32,
    mode: &str,
) -> Result<()> {
    execute!(stdout, cursor::MoveTo(0, 0))?;

    for row in 0..video_rows as usize {
        // Left panel (local).
        let left_line: String = if row < local.len() {
            local[row].iter().map(|c| c.ch).collect()
        } else {
            " ".repeat(half_cols as usize)
        };

        // Right panel (remote).
        let right_line: String = if row < remote.len() {
            remote[row].iter().map(|c| c.ch).collect()
        } else {
            " ".repeat(half_cols as usize)
        };

        let left_padded = format!("{:<width$}", left_line, width = half_cols as usize);
        let right_padded = format!("{:<width$}", right_line, width = half_cols as usize);

        execute!(
            stdout,
            cursor::MoveTo(0, row as u16),
            crossterm::style::Print(&left_padded),
            cursor::MoveTo(half_cols, row as u16),
            crossterm::style::Print(&right_padded),
        )?;
    }

    // Draw divider line.
    for row in 0..video_rows {
        execute!(
            stdout,
            cursor::MoveTo(half_cols.saturating_sub(1), row),
            crossterm::style::Print('|'),
        )?;
    }

    // Status bar (second-to-last row): labels.
    let label_row = term_rows.saturating_sub(2);
    let local_label = format!("{:^width$}", "[ Local ]", width = half_cols as usize);
    let remote_label = format!("{:^width$}", "[ Remote ]", width = half_cols as usize);
    execute!(
        stdout,
        cursor::MoveTo(0, label_row),
        crossterm::style::Print(&local_label),
        cursor::MoveTo(half_cols, label_row),
        crossterm::style::Print(&remote_label),
    )?;

    // Status bar (last row): connection info.
    let status = format!(
        " {mode} | peer: {peer_addr} | {fps} fps | press q to quit{pad}",
        pad = " ".repeat(term_cols.saturating_sub(60) as usize),
    );
    let status = &status[..status.len().min(term_cols as usize)];
    execute!(
        stdout,
        cursor::MoveTo(0, term_rows.saturating_sub(1)),
        crossterm::style::Print(status),
    )?;

    stdout.flush()?;
    Ok(())
}
