use anyhow::{Context, Result};
use std::net::{TcpListener, TcpStream};

use crate::camera::CameraCapture;

/// Connect to `addr` and start a video call.
pub fn run_caller(addr: &str, camera: CameraCapture) -> Result<()> {
    eprintln!("Connecting to {addr}...");
    let stream = TcpStream::connect(addr)
        .with_context(|| format!("Failed to connect to {addr}"))?;
    let peer_addr = stream.peer_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    eprintln!("Connected to {peer_addr}");

    crate::tui::run_call(camera, stream, peer_addr)
}

/// Listen on `port` and accept the first incoming connection.
pub fn run_listener(port: u16, camera: CameraCapture) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .with_context(|| format!("Failed to bind port {port}"))?;
    eprintln!("Listening on port {port}... waiting for connection");
    let (stream, peer_addr) = listener.accept().context("Accept failed")?;
    eprintln!("Connected: {peer_addr}");

    crate::tui::run_call(camera, stream, peer_addr)
}
