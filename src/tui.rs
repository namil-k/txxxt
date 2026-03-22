use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Terminal;

use crate::background::BackgroundModel;
use crate::camera::CameraCapture;
use crate::charsets::CharsetName;
use crate::render::{render_frame, AsciiCell, RenderConfig, RenderMode};

/// Which overlay panel is currently open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    StylePicker,
    Settings,
    Preference,
    Connect,
    Account,
    Friends,
}

/// App mode: local viewer or in-call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Local,
    Call {
        peer_addr: SocketAddr,
    },
    /// Waiting for peer to join relay room.
    RelayWaiting,
    /// Joining a relay room.
    RelayJoining,
}

/// PIP corner position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipCorner {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl PipCorner {
    fn next(self) -> Self {
        match self {
            PipCorner::TopRight => PipCorner::TopLeft,
            PipCorner::TopLeft => PipCorner::BottomLeft,
            PipCorner::BottomLeft => PipCorner::BottomRight,
            PipCorner::BottomRight => PipCorner::TopRight,
        }
    }

    #[allow(dead_code)]
    fn label(self) -> &'static str {
        match self {
            PipCorner::TopRight => "top-right",
            PipCorner::TopLeft => "top-left",
            PipCorner::BottomRight => "bottom-right",
            PipCorner::BottomLeft => "bottom-left",
        }
    }
}

/// PIP size as a fraction of the screen.
const PIP_SCALES: &[u8] = &[15, 20, 25, 33, 50];
const PIP_DEFAULT_SCALE_IDX: usize = 2; // 25%

/// Settings panel items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsItem {
    Color,
    Background,
    Contour,
    Mirror,
    Brightness,
}

impl SettingsItem {
    const ALL: &'static [SettingsItem] = &[
        SettingsItem::Color,
        SettingsItem::Background,
        SettingsItem::Contour,
        SettingsItem::Mirror,
        SettingsItem::Brightness,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsItem::Color => "color",
            SettingsItem::Background => "background",
            SettingsItem::Contour => "contour",
            SettingsItem::Mirror => "mirror",
            SettingsItem::Brightness => "bright threshold",
        }
    }
}


/// Preference panel items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefItem {
    SaveDir,
}

impl PrefItem {
    const ALL: &'static [PrefItem] = &[PrefItem::SaveDir];

    fn label(self) -> &'static str {
        match self {
            PrefItem::SaveDir => "save folder",
        }
    }
}

/// Unified visual style: charsets + outline in one list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualStyle {
    Charset(CharsetName),
    Outline,
}

impl VisualStyle {
    /// All available styles in display order.
    pub const ALL: &'static [VisualStyle] = &[
        VisualStyle::Charset(CharsetName::Standard),
        VisualStyle::Charset(CharsetName::Letters),
        VisualStyle::Charset(CharsetName::Dots),
        VisualStyle::Charset(CharsetName::Digits),
        VisualStyle::Charset(CharsetName::Blocks),
        VisualStyle::Charset(CharsetName::Hangul),
        VisualStyle::Charset(CharsetName::Hiragana),
        VisualStyle::Charset(CharsetName::Katakana),
        VisualStyle::Charset(CharsetName::Hanja),
        VisualStyle::Outline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VisualStyle::Charset(cs) => cs.label(),
            VisualStyle::Outline => "lines",
        }
    }

    /// Apply this style to a RenderConfig.
    pub fn apply(self, config: &mut RenderConfig) {
        match self {
            VisualStyle::Charset(cs) => {
                config.mode = RenderMode::Normal;
                config.charset = cs;
            }
            VisualStyle::Outline => {
                config.mode = RenderMode::Outline;
            }
        }
    }

    /// Determine current style from config.
    pub fn from_config(config: &RenderConfig) -> Self {
        match config.mode {
            RenderMode::Outline => VisualStyle::Outline,
            RenderMode::Normal => VisualStyle::Charset(config.charset),
        }
    }

    /// Index of this style in ALL.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }
}

const FLASH_DISPLAY_SECS: u64 = 5;

/// Action returned from key handler that needs resources from main loop.
#[derive(Debug)]
pub(crate) enum ExportAction {
    Save,
}

/// Application state.
pub struct App {
    pub config: RenderConfig,
    pub running: bool,
    pub fps: f32,
    frame_count: u64,
    /// Plain-text representation of the last rendered ASCII frame.
    pub last_frame_text: String,
    /// Last rendered grid (for ANSI export).
    pub last_frame_grid: Option<Vec<Vec<AsciiCell>>>,
    /// Flash message for status bar (e.g. "Saved: filename.txt").
    pub flash_message: Option<(String, Instant)>,
    /// Currently open overlay panel.
    pub panel: Option<Panel>,
    /// Cursor position within the open panel.
    pub panel_cursor: usize,
    /// Persisted user config (preferences like save_dir).
    pub user_config: crate::config::UserConfig,
    /// Whether the preference panel is in text-editing mode.
    pref_editing: bool,
    /// Text buffer for preference text input.
    pref_input: String,
    /// Directory entries shown below input in preference panel.
    pref_dir_entries: Vec<DirEntry>,
    /// Cursor within the directory listing (-1 = on input field).
    pref_dir_cursor: Option<usize>,
    /// Tab completion: cached matches and cycling index.
    pref_tab_matches: Vec<String>,
    pref_tab_index: usize,
    /// The input text when Tab was first pressed (to re-filter on next Tab).
    pref_tab_base: String,
    /// Current app mode (local viewer or call).
    mode: AppMode,
    /// Text input buffer for Connect panel (room code).
    connect_input: String,
    /// Remote frame received from peer (during call).
    remote_grid: Option<Vec<Vec<AsciiCell>>>,
    /// Channel receiver for remote frames.
    remote_rx: Option<mpsc::Receiver<Vec<Vec<AsciiCell>>>>,
    /// TCP writer for sending frames to peer.
    net_writer: Option<std::net::TcpStream>,
    /// PIP corner position during call.
    pip_corner: PipCorner,
    /// PIP size index into PIP_SCALES.
    pip_scale_idx: usize,
    /// Audio capture stream handle (kept alive during call).
    #[allow(dead_code)]
    audio_capture: Option<cpal::Stream>,
    /// Audio capture receiver (PCM chunks from mic).
    audio_capture_rx: Option<mpsc::Receiver<Vec<i16>>>,
    /// Audio playback stream handle (kept alive during call).
    #[allow(dead_code)]
    audio_playback: Option<cpal::Stream>,
    /// Audio playback sender (PCM chunks to speaker).
    audio_playback_tx: Option<mpsc::Sender<Vec<i16>>>,
    /// Audio receiver from network (decoded audio chunks from peer).
    audio_net_rx: Option<mpsc::Receiver<Vec<i16>>>,
    /// Whether audio is muted.
    audio_muted: bool,
    /// Whether camera is hidden (stop sending video + hide PIP).
    camera_hidden: bool,
    /// Local mic audio level (0.0 – 1.0).
    audio_level_local: f32,
    /// Remote audio level (0.0 – 1.0).
    audio_level_remote: f32,
    /// WebRTC echo canceller.
    echo_canceller: Option<crate::audio::EchoCanceller>,
    /// Local capture sample rate (for resampling to NET_SAMPLE_RATE).
    audio_capture_rate: u32,
    /// Local playback sample rate (for resampling from NET_SAMPLE_RATE).
    audio_playback_rate: u32,
    /// Relay CREATE status receiver (kind, data).
    relay_rx: Option<mpsc::Receiver<(String, String)>>,
    /// Relay CREATE thread handle.
    relay_handle: Option<std::thread::JoinHandle<Option<(std::net::TcpStream, SocketAddr)>>>,
    /// Current relay room code.
    relay_code: Option<String>,
    /// Relay JOIN stream receiver.
    relay_join_rx: Option<mpsc::Receiver<(std::net::TcpStream, SocketAddr)>>,
    /// Relay JOIN error receiver.
    relay_join_err_rx: Option<mpsc::Receiver<String>>,
    /// Disconnect notification from reader thread.
    disconnect_rx: Option<mpsc::Receiver<()>>,
    /// Remote peer status (mic/camera).
    remote_status: Option<PeerStatus>,
    /// Channel for receiving remote status updates.
    remote_status_rx: Option<mpsc::Receiver<PeerStatus>>,
    /// Call start time (for 5-min countdown).
    call_start: Option<Instant>,
    /// Receiver for incoming call notifications from presence thread.
    incoming_rx: Option<std::sync::mpsc::Receiver<crate::net::presence::IncomingCall>>,
    /// Pending incoming call waiting for user to accept/decline.
    pending_call: Option<crate::net::presence::IncomingCall>,
    // ── Account panel ──
    /// Which field is focused (0=key, 1=username).
    account_field: usize,
    /// License key text input.
    account_key_input: String,
    /// Username text input.
    account_username_input: String,
    /// Status message shown in account panel.
    account_status: Option<String>,
    // ── Friends panel ──
    /// Cached friends list.
    friends_list: Vec<String>,
    /// Whether the add-friend input field is focused.
    friends_adding: bool,
    /// Add-friend text input.
    friends_add_input: String,
    /// Status message shown in friends panel.
    friends_status: Option<String>,
    /// Whether the help overlay is visible (independent of panel).
    show_help: bool,
}

/// A directory entry for the preference file picker.
#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    is_dir: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            config: RenderConfig::default(),
            running: true,
            fps: 0.0,
            frame_count: 0,
            last_frame_text: String::new(),
            last_frame_grid: None,
            flash_message: None,
            panel: None,
            panel_cursor: 0,
            user_config: crate::config::UserConfig::default(),
            pref_editing: false,
            pref_input: String::new(),
            pref_dir_entries: Vec::new(),
            pref_dir_cursor: None,
            pref_tab_matches: Vec::new(),
            pref_tab_index: 0,
            pref_tab_base: String::new(),
            mode: AppMode::Local,
            connect_input: String::new(),
            remote_grid: None,
            remote_rx: None,
            net_writer: None,
            pip_corner: PipCorner::TopRight,
            pip_scale_idx: PIP_DEFAULT_SCALE_IDX,
            audio_capture: None,
            audio_capture_rx: None,
            audio_playback: None,
            audio_playback_tx: None,
            audio_net_rx: None,
            audio_muted: false,
            camera_hidden: false,
            audio_level_local: 0.0,
            audio_level_remote: 0.0,
            echo_canceller: None,
            audio_capture_rate: 48000,
            audio_playback_rate: 48000,
            relay_rx: None,
            relay_handle: None,
            relay_code: None,
            relay_join_rx: None,
            relay_join_err_rx: None,
            disconnect_rx: None,
            remote_status: None,
            remote_status_rx: None,
            call_start: None,
            incoming_rx: None,
            pending_call: None,
            account_field: 0,
            account_key_input: String::new(),
            account_username_input: String::new(),
            account_status: None,
            friends_list: Vec::new(),
            friends_adding: false,
            friends_add_input: String::new(),
            friends_status: None,
            show_help: false,
        }
    }

    /// Setup call state with an established TCP connection.
    fn setup_call(&mut self, stream: std::net::TcpStream, peer_addr: SocketAddr) {
        stream.set_nonblocking(false).ok();
        let reader_stream = stream.try_clone().expect("failed to clone stream");
        let writer_stream = stream;

        let (remote_tx, remote_rx) = mpsc::channel::<Vec<Vec<AsciiCell>>>();
        let (audio_net_tx, audio_net_rx) = mpsc::channel::<Vec<i16>>();
        let (status_tx, status_rx) = mpsc::channel::<PeerStatus>();
        let (disconnect_tx, disconnect_rx) = mpsc::channel::<()>();

        // Spawn reader thread — handles both video and audio messages.
        std::thread::spawn(move || {
            use std::io::Read;
            let mut reader = reader_stream;
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            // Cap buffer to 8MB to prevent unbounded growth.
            const MAX_BUF: usize = 8 * 1024 * 1024;
            let mut tmp = [0u8; 8192];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.len() > MAX_BUF {
                            buf.clear();
                            break;
                        }
                        let mut latest_grid = None;
                        loop {
                            match decode_message(&buf) {
                                Some((msg, consumed)) => {
                                    match msg {
                                        Message::Video(frame) => {
                                            latest_grid = Some(frame_to_grid(&frame));
                                        }
                                        Message::Audio(samples) => {
                                            let _ = audio_net_tx.send(samples);
                                        }
                                        Message::Status(status) => {
                                            let _ = status_tx.send(status);
                                        }
                                    }
                                    buf.drain(..consumed);
                                }
                                None => break,
                            }
                        }
                        if let Some(grid) = latest_grid {
                            if remote_tx.send(grid).is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            // Notify main thread that connection is lost.
            let _ = disconnect_tx.send(());
        });

        // Start audio capture and playback.
        let mut capture_rate = 48000u32;
        let (audio_capture, audio_capture_rx) = match crate::audio::start_capture() {
            Ok((stream, rx, rate)) => {
                capture_rate = rate;
                (Some(stream), Some(rx))
            }
            Err(e) => {
                self.flash(format!("mic error: {}", e));
                (None, None)
            }
        };
        let mut playback_rate = 48000u32;
        let (audio_playback, audio_playback_tx) = match crate::audio::start_playback() {
            Ok((stream, tx, rate)) => { playback_rate = rate; (Some(stream), Some(tx)) }
            Err(e) => {
                self.flash(format!("speaker error: {}", e));
                (None, None)
            }
        };

        // Initialize WebRTC echo canceller.
        let echo_canceller = match crate::audio::EchoCanceller::new(capture_rate) {
            Ok(ec) => Some(ec),
            Err(e) => {
                self.flash(format!("AEC init error: {}", e));
                None
            }
        };

        self.mode = AppMode::Call { peer_addr };
        self.remote_rx = Some(remote_rx);
        self.net_writer = Some(writer_stream);
        self.remote_grid = None;
        self.panel = None;
        self.audio_capture = audio_capture;
        self.audio_capture_rx = audio_capture_rx;
        self.audio_capture_rate = capture_rate;
        self.audio_playback = audio_playback;
        self.audio_playback_tx = audio_playback_tx;
        self.audio_playback_rate = playback_rate;
        self.disconnect_rx = Some(disconnect_rx);
        self.remote_status = None;
        self.remote_status_rx = Some(status_rx);
        self.call_start = Some(Instant::now());
        self.audio_net_rx = Some(audio_net_rx);
        self.audio_muted = false;
        self.echo_canceller = echo_canceller;
    }

    /// Create a relay room: connect to relay server, send CREATE, get room code.
    fn start_relay_create(&mut self) {
        self.flash("connecting to relay...".into());
        let (tx, rx) = mpsc::channel();

        let handle = std::thread::spawn(move || -> Option<(std::net::TcpStream, SocketAddr)> {
            use std::io::{BufRead, BufReader, Write};
            let addr = std::net::ToSocketAddrs::to_socket_addrs(&RELAY_ADDR)
                .ok()?.next()?;
            let stream = std::net::TcpStream::connect_timeout(
                &addr,
                Duration::from_secs(5),
            ).ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(120)).into()).ok();
            let mut wstream = stream.try_clone().ok()?;
            write!(wstream, "CREATE\n").ok()?;
            wstream.flush().ok()?;

            let mut reader = BufReader::new(stream.try_clone().ok()?);

            // Read "ROOM XXXXXX"
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            let code = line.trim().strip_prefix("ROOM ")?.to_string();

            let _ = tx.send(("CODE".to_string(), code.clone()));

            let mut line2 = String::new();
            reader.read_line(&mut line2).ok()?;
            if line2.trim() == "PAIRED" {
                let addr = stream.peer_addr().ok()?;
                let _ = tx.send(("PAIRED".to_string(), String::new()));
                Some((stream, addr))
            } else {
                let _ = tx.send(("ERROR".to_string(), "relay connection lost".to_string()));
                None
            }
        });

        self.mode = AppMode::RelayWaiting;
        self.relay_rx = Some(rx);
        self.relay_handle = Some(handle);
    }

    /// Join a relay room with a code.
    fn start_relay_join(&mut self, code: &str) {
        let code = code.trim().to_uppercase();
        self.flash(format!("joining room {}...", code));

        let code_clone = code.clone();
        let (tx, rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel::<String>();

        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let addr = match std::net::ToSocketAddrs::to_socket_addrs(&RELAY_ADDR)
                .ok().and_then(|mut addrs| addrs.next()) {
                Some(a) => a,
                None => { let _ = err_tx.send("invalid relay address".into()); return; }
            };
            let stream = match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(s) => s,
                Err(_) => { let _ = err_tx.send("relay server unreachable".into()); return; }
            };
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
            let mut wstream = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => { let _ = err_tx.send("stream error".into()); return; }
            };
            if write!(wstream, "JOIN {}\n", code_clone).is_err() || wstream.flush().is_err() {
                let _ = err_tx.send("relay send error".into());
                return;
            }

            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => { let _ = err_tx.send("stream error".into()); return; }
            });
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                let _ = err_tx.send("relay read timeout".into());
                return;
            }

            let response = line.trim();
            if response == "OK" {
                if let Ok(addr) = stream.peer_addr() {
                    let _ = tx.send((stream, addr));
                }
            } else if response.starts_with("ERR") {
                let _ = err_tx.send(format!("room not found: {}", code_clone));
            } else {
                let _ = err_tx.send("unexpected relay response".into());
            }
        });

        self.mode = AppMode::RelayJoining;
        self.relay_join_rx = Some(rx);
        self.relay_join_err_rx = Some(err_rx);
    }

    /// Check relay create/join status.
    fn check_relay(&mut self) {
        // Check CREATE flow.
        if let Some(ref rx) = self.relay_rx {
            if let Ok((kind, data)) = rx.try_recv() {
                if kind == "CODE" {
                    let msg = format!(
                        "txxxt me ↓\ncode: {}\ntxxxt.me/{}",
                        data, data
                    );
                    let mut copied = false;
                    if let Ok(mut clip) = arboard::Clipboard::new() {
                        if clip.set_text(&msg).is_ok() {
                            copied = true;
                        }
                    }
                    if copied {
                        self.flash(format!("invite copied! code: {}", data));
                    } else {
                        self.flash(format!("code: {} | txxxt.me/{}", data, data));
                    }
                    self.relay_code = Some(data);
                } else if kind == "PAIRED" {
                    // Peer joined! Now get the stream from the handle.
                    if let Some(handle) = self.relay_handle.take() {
                        if let Ok(Some((stream, addr))) = handle.join() {
                            self.relay_rx = None;
                            self.relay_code = None;
                            self.setup_call(stream, addr);
                            self.flash("relay connected!".into());
                            return;
                        }
                    }
                } else if kind == "ERROR" {
                    self.flash(format!("relay error: {}", data));
                    self.end_call();
                    return;
                }
            }
        }

        // Check JOIN flow.
        if let Some(ref rx) = self.relay_join_rx {
            if let Ok((stream, addr)) = rx.try_recv() {
                self.relay_join_rx = None;
                self.relay_join_err_rx = None;
                self.setup_call(stream, addr);
                self.flash("relay connected!".into());
                return;
            }
        }
        // Check JOIN errors.
        if let Some(ref rx) = self.relay_join_err_rx {
            if let Ok(err) = rx.try_recv() {
                self.flash(err);
                self.end_call();
                return;
            }
        }
    }

    /// Send current mic/camera status to peer.
    fn send_status(&mut self) {
        use std::io::Write;
        let status = PeerStatus {
            mic_muted: self.audio_muted,
            camera_hidden: self.camera_hidden,
        };
        let encoded = encode_status(&status);
        if let Some(ref mut writer) = self.net_writer {
            let _ = writer.write_all(&encoded);
        }
    }

    /// End the current call and return to local mode.
    /// Apply a visual style.
    fn try_apply_style(&mut self, idx: usize) {
        let style = VisualStyle::ALL[idx];
        style.apply(&mut self.config);
    }

    fn end_call(&mut self) {
        self.mode = AppMode::Local;
        self.remote_rx = None;
        self.net_writer = None;
        self.remote_grid = None;
        // Drop audio streams and AEC.
        self.audio_capture = None;
        self.audio_capture_rx = None;
        self.audio_playback = None;
        self.audio_playback_tx = None;
        self.audio_net_rx = None;
        self.echo_canceller = None;
        self.relay_rx = None;
        self.relay_handle = None;
        self.relay_code = None;
        self.relay_join_rx = None;
        self.relay_join_err_rx = None;
        self.disconnect_rx = None;
        self.remote_status = None;
        self.remote_status_rx = None;
        self.call_start = None;
        self.flash("call ended".into());
    }

    /// Handle key when a panel is open. Returns (consumed, optional export action).
    /// Unrecognized keys return (false, None) so they fall through to global handle_key.
    fn handle_panel_key(&mut self, key: KeyEvent) -> (bool, Option<ExportAction>) {
        let Some(panel) = self.panel else { return (false, None) };

        // Allow switching between menu panels directly (except when typing in text fields).
        let is_text_input = matches!(panel, Panel::Connect)
            || (matches!(panel, Panel::Friends) && self.friends_adding)
            || (matches!(panel, Panel::Account) && crate::config::get_account().is_none());

        if !is_text_input {
            match key.code {
                KeyCode::Char('v') | KeyCode::Char('1') if panel == Panel::StylePicker => {
                    self.panel = None;
                    return (true, None);
                }
                KeyCode::Char('v') | KeyCode::Char('1') if panel != Panel::StylePicker => {
                    self.panel = Some(Panel::StylePicker);
                    self.panel_cursor = VisualStyle::from_config(&self.config).index();
                    return (true, None);
                }
                KeyCode::Char('s') | KeyCode::Char('2') if panel == Panel::Settings => {
                    self.panel = None;
                    return (true, None);
                }
                KeyCode::Char('s') | KeyCode::Char('2') if panel != Panel::Settings => {
                    self.panel = Some(Panel::Settings);
                    self.panel_cursor = 0;
                    return (true, None);
                }
                KeyCode::Char('f') if panel == Panel::Friends => {
                    self.panel = None;
                    return (true, None);
                }
                KeyCode::Char('f') if panel != Panel::Friends => {
                    if crate::config::get_account().is_some() {
                        self.open_friends_panel();
                    }
                    return (true, None);
                }
                KeyCode::Char('3') => {
                    if crate::config::get_account().is_some() {
                        if panel == Panel::Friends { self.panel = None; } else { self.open_friends_panel(); }
                    } else {
                        if panel == Panel::Account { self.panel = None; } else { self.open_account_panel(); }
                    }
                    return (true, None);
                }
                KeyCode::Char('a') if panel == Panel::Account => {
                    self.panel = None;
                    return (true, None);
                }
                KeyCode::Char('a') if panel != Panel::Account => {
                    self.open_account_panel();
                    return (true, None);
                }
                KeyCode::Char('4') => {
                    if crate::config::get_account().is_some() {
                        if panel == Panel::Account { self.panel = None; } else { self.open_account_panel(); }
                    }
                    return (true, None);
                }
                _ => {}
            }
        }

        match panel {
            Panel::StylePicker => {
                let count = VisualStyle::ALL.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.panel_cursor = self.panel_cursor.saturating_sub(1);
                        self.try_apply_style(self.panel_cursor);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.panel_cursor + 1 < count {
                            self.panel_cursor += 1;
                        }
                        self.try_apply_style(self.panel_cursor);
                    }
                    KeyCode::Enter | KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('q') => {
                        self.panel = None;
                    }
                    _ => return (false, None),
                }
            }
            Panel::Settings => {
                let count = SettingsItem::ALL.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.panel_cursor = self.panel_cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.panel_cursor + 1 < count {
                            self.panel_cursor += 1;
                        }
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') | KeyCode::Enter => {
                        let item = SettingsItem::ALL[self.panel_cursor];
                        let is_right = matches!(key.code, KeyCode::Right | KeyCode::Char('l'));
                        match item {
                            SettingsItem::Color => {
                                self.config.color = !self.config.color;
                            }
                            SettingsItem::Background => {
                                use crate::render::BgMode;
                                if self.config.bg_mode == BgMode::Person {
                                    self.config.bg_mode = BgMode::Off;
                                } else if crate::segmentation::default_model_path().exists() {
                                    self.config.bg_mode = BgMode::Person;
                                } else {
                                    self.flash("downloading segmentation model...".into());
                                    crate::segmentation::download_model_bg();
                                }
                            }
                            SettingsItem::Contour => {
                                if crate::segmentation::default_model_path().exists() {
                                    self.config.contour = !self.config.contour;
                                } else {
                                    self.flash("downloading segmentation model...".into());
                                    crate::segmentation::download_model_bg();
                                }
                            }
                            SettingsItem::Mirror => {
                                self.config.mirror = !self.config.mirror;
                            }
                            SettingsItem::Brightness => {
                                if is_right {
                                    self.config.brightness_threshold =
                                        self.config.brightness_threshold.saturating_add(5);
                                } else {
                                    self.config.brightness_threshold =
                                        self.config.brightness_threshold.saturating_sub(5);
                                }
                            }
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char('q') => {
                        self.panel = None;
                    }
                    _ => return (false, None),
                }
            }
            Panel::Preference => {
                if self.pref_editing {
                    // Text editing mode: consume ALL keys (typing goes to input buffer).
                    match key.code {
                        KeyCode::Enter => {
                            if let Some(idx) = self.pref_dir_cursor {
                                if idx < self.pref_dir_entries.len() {
                                    let entry = &self.pref_dir_entries[idx];
                                    let base = pref_resolve_parent(&self.pref_input);
                                    let mut new_path = base.join(&entry.name);
                                    if entry.is_dir {
                                        new_path.push("");
                                    }
                                    self.pref_input = pref_display_path(&new_path);
                                    self.pref_dir_cursor = None;
                                    self.pref_refresh_dir_entries();
                                    self.pref_clear_tab();
                                }
                            } else {
                                let val = self.pref_input.trim().to_string();
                                self.user_config.save_dir = if val.is_empty() { None } else { Some(val) };
                                self.pref_editing = false;
                                crate::config::save(&self.user_config);
                                let display = self.user_config.save_dir.as_deref().unwrap_or("~/Downloads");
                                self.flash_message = Some((format!("save folder: {}", display), Instant::now()));
                            }
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            if self.pref_tab_matches.is_empty() || key.code == KeyCode::Tab && self.pref_tab_base != self.pref_input {
                                self.pref_tab_base = self.pref_input.clone();
                                self.pref_tab_matches = pref_tab_complete(&self.pref_input);
                                self.pref_tab_index = 0;
                            } else if !self.pref_tab_matches.is_empty() {
                                self.pref_tab_index = (self.pref_tab_index + 1) % self.pref_tab_matches.len();
                            }
                            if let Some(m) = self.pref_tab_matches.get(self.pref_tab_index) {
                                self.pref_input = m.clone();
                                self.pref_dir_cursor = None;
                                self.pref_refresh_dir_entries();
                            }
                        }
                        KeyCode::Up => {
                            match self.pref_dir_cursor {
                                Some(0) | None => { self.pref_dir_cursor = None; }
                                Some(i) => { self.pref_dir_cursor = Some(i - 1); }
                            }
                        }
                        KeyCode::Down => {
                            if !self.pref_dir_entries.is_empty() {
                                let max = self.pref_dir_entries.len() - 1;
                                self.pref_dir_cursor = Some(match self.pref_dir_cursor {
                                    None => 0,
                                    Some(i) => i.min(max - 1) + 1,
                                });
                            }
                        }
                        KeyCode::Esc => {
                            self.pref_editing = false;
                            self.pref_dir_cursor = None;
                            self.pref_clear_tab();
                        }
                        KeyCode::Backspace => {
                            self.pref_input.pop();
                            self.pref_dir_cursor = None;
                            self.pref_refresh_dir_entries();
                            self.pref_clear_tab();
                        }
                        KeyCode::Char(c) => {
                            self.pref_input.push(c);
                            self.pref_dir_cursor = None;
                            self.pref_refresh_dir_entries();
                            self.pref_clear_tab();
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Enter => {
                            let item = PrefItem::ALL[self.panel_cursor];
                            match item {
                                PrefItem::SaveDir => {
                                    self.pref_editing = true;
                                    self.pref_input = self.user_config.save_dir.clone().unwrap_or_else(|| "~/Downloads".into());
                                    self.pref_dir_cursor = None;
                                    self.pref_refresh_dir_entries();
                                    self.pref_clear_tab();
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            self.panel_cursor = self.panel_cursor.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let count = PrefItem::ALL.len();
                            if self.panel_cursor + 1 < count {
                                self.panel_cursor += 1;
                            }
                        }
                        KeyCode::Esc | KeyCode::Char(',') | KeyCode::Char('q') => {
                            self.panel = None;
                        }
                        _ => return (false, None),
                    }
                }
            }
            Panel::Connect => {
                // Text input for room code.
                match key.code {
                    KeyCode::Enter => {
                        let input = self.connect_input.trim().to_uppercase();
                        if !input.is_empty() {
                            self.panel = None;
                            self.start_relay_join(&input);
                        }
                    }
                    KeyCode::Char('q') if self.connect_input.is_empty() => {
                        self.panel = None;
                    }
                    KeyCode::Esc | KeyCode::Char('c') if self.connect_input.is_empty() => {
                        self.panel = None;
                    }
                    KeyCode::Esc => {
                        self.panel = None;
                    }
                    KeyCode::Backspace => {
                        self.connect_input.pop();
                    }
                    KeyCode::Char(c) => {
                        self.connect_input.push(c);
                    }
                    _ => {}
                }
            }
            Panel::Account => {
                let logged_in = crate::config::get_account().is_some();
                if logged_in {
                    // Logged-in view: only logout or close.
                    match key.code {
                        KeyCode::Char('l') => {
                            // Logout: clear account from config.
                            let mut cfg = crate::config::load();
                            cfg.username = None;
                            cfg.session_token = None;
                            crate::config::save(&cfg);
                            self.user_config = cfg;
                            self.panel = None;
                            self.flash("logged out".into());
                        }
                        KeyCode::Esc | KeyCode::Char('a') => {
                            self.panel = None;
                        }
                        _ => {}
                    }
                } else {
                    // Register/login view.
                    match key.code {
                        KeyCode::Tab => {
                            self.account_field = 1 - self.account_field;
                        }
                        KeyCode::Enter => {
                            let key_str = self.account_key_input.trim().to_string();
                            let username = self.account_username_input.trim().to_string();
                            if key_str.is_empty() {
                                self.account_status = Some("enter license key".into());
                            } else if username.is_empty() {
                                self.account_status = Some("enter username".into());
                            } else {
                                self.account_status = Some("registering...".into());
                                match crate::net::relay::register(&key_str, &username) {
                                    Ok((un, tok)) => {
                                        crate::config::save_license_key(&key_str);
                                        crate::config::save_account(&un, &tok);
                                        self.user_config = crate::config::load();
                                        self.panel = None;
                                        self.flash(format!("registered as @{}", un));
                                        // Start presence listener.
                                        self.incoming_rx = Some(
                                            crate::net::presence::start_presence(&tok),
                                        );
                                    }
                                    Err(_) => {
                                        // Register failed — try login instead (may already be registered).
                                        match crate::net::relay::login(&key_str) {
                                            Ok((un, tok)) => {
                                                crate::config::save_account(&un, &tok);
                                                self.user_config = crate::config::load();
                                                self.panel = None;
                                                self.flash(format!("logged in as @{}", un));
                                                self.incoming_rx = Some(
                                                    crate::net::presence::start_presence(&tok),
                                                );
                                            }
                                            Err(e2) => {
                                                self.account_status = Some(format!("{}", e2));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Esc | KeyCode::Char('\x1b') => {
                            self.panel = None;
                        }
                        KeyCode::Backspace => {
                            if self.account_field == 0 {
                                self.account_key_input.pop();
                            } else {
                                self.account_username_input.pop();
                            }
                        }
                        KeyCode::Char(c) => {
                            if self.account_field == 0 {
                                self.account_key_input.push(c);
                            } else {
                                self.account_username_input.push(c);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Panel::Friends => {
                if self.friends_adding {
                    // Add-friend input mode.
                    match key.code {
                        KeyCode::Enter => {
                            let name = self.friends_add_input.trim().to_string();
                            if !name.is_empty() {
                                if let Some((_, token)) = crate::config::get_account() {
                                    match crate::net::relay::friends_add(&token, &name) {
                                        Ok(()) => {
                                            self.friends_list.push(name.clone());
                                            self.friends_add_input.clear();
                                            self.friends_adding = false;
                                            self.friends_status = Some(format!("added @{}", name));
                                        }
                                        Err(e) => {
                                            self.friends_status = Some(format!("{}", e));
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Esc | KeyCode::Tab => {
                            self.friends_adding = false;
                        }
                        KeyCode::Backspace => {
                            self.friends_add_input.pop();
                        }
                        KeyCode::Char(c) => {
                            self.friends_add_input.push(c);
                        }
                        _ => {}
                    }
                } else {
                    // Friends list navigation mode.
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            self.panel_cursor = self.panel_cursor.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !self.friends_list.is_empty() && self.panel_cursor + 1 < self.friends_list.len() {
                                self.panel_cursor += 1;
                            }
                        }
                        KeyCode::Enter | KeyCode::Char('c') => {
                            // Call selected friend.
                            if let Some(friend) = self.friends_list.get(self.panel_cursor).cloned() {
                                if let Some((_, token)) = crate::config::get_account() {
                                    self.friends_status = Some(format!("calling @{}...", friend));
                                    match crate::net::relay::call_user(&token, &friend) {
                                        Ok(code) => {
                                            self.panel = None;
                                            self.start_relay_join(&code);
                                        }
                                        Err(e) => {
                                            self.friends_status = Some(format!("{}", e));
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('x') => {
                            // Remove selected friend.
                            if let Some(friend) = self.friends_list.get(self.panel_cursor).cloned() {
                                if let Some((_, token)) = crate::config::get_account() {
                                    match crate::net::relay::friends_remove(&token, &friend) {
                                        Ok(()) => {
                                            self.friends_list.remove(self.panel_cursor);
                                            if self.panel_cursor > 0 && self.panel_cursor >= self.friends_list.len() {
                                                self.panel_cursor -= 1;
                                            }
                                            self.friends_status = Some(format!("removed @{}", friend));
                                        }
                                        Err(e) => {
                                            self.friends_status = Some(format!("{}", e));
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Tab => {
                            self.friends_adding = true;
                            self.friends_add_input.clear();
                        }
                        KeyCode::Esc | KeyCode::Char('f') => {
                            self.panel = None;
                        }
                        _ => {}
                    }
                }
            }
        }
        (true, None)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ExportAction> {
        // Handle incoming call popup first.
        if self.pending_call.is_some() {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    // Accept: join the room.
                    if let Some(call) = self.pending_call.take() {
                        self.start_relay_join(&call.code);
                    }
                    return None;
                }
                KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => {
                    // Decline: dismiss popup.
                    self.pending_call = None;
                    return None;
                }
                _ => return None,
            }
        }

        // If a panel is open, route input there first.
        let (consumed, action) = self.handle_panel_key(key);
        if consumed {
            return action;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                // In call/relay mode: end call. In local mode: quit.
                match self.mode {
                    AppMode::Call { .. }
                    | AppMode::RelayWaiting | AppMode::RelayJoining => {
                        self.end_call();
                    }
                    AppMode::Local => {
                        self.running = false;
                    }
                }
            }
            KeyCode::Char('c') => {
                if self.mode == AppMode::Local {
                    self.panel = Some(Panel::Connect);
                    self.connect_input.clear();
                }
            }
            KeyCode::Char('`') => {
                self.show_help = !self.show_help;
            }
            KeyCode::Char('r') => {
                if self.mode == AppMode::Local {
                    self.start_relay_create();
                }
            }
            KeyCode::Char('v') | KeyCode::Char('1') => {
                self.panel = Some(Panel::StylePicker);
                self.panel_cursor = VisualStyle::from_config(&self.config).index();
            }
            KeyCode::Char('s') | KeyCode::Char('2') => {
                self.panel = Some(Panel::Settings);
                self.panel_cursor = 0;
            }
            KeyCode::Char(',') => {
                self.panel = Some(Panel::Preference);
                self.panel_cursor = 0;
                self.pref_editing = false;
            }
            KeyCode::Char('a') => {
                self.open_account_panel();
            }
            KeyCode::Char('4') => {
                if crate::config::get_account().is_some() {
                    // Logged in: 4 = account
                    self.open_account_panel();
                }
                // Logged out: only 3 items, 4 does nothing.
            }
            KeyCode::Char('f') => {
                if crate::config::get_account().is_some() {
                    self.open_friends_panel();
                } else {
                    self.flash("login first: press [a]".into());
                }
            }
            KeyCode::Char('3') => {
                if crate::config::get_account().is_some() {
                    // Logged in: 3 = friends
                    self.open_friends_panel();
                } else {
                    // Logged out: 3 = account (no friends in menu)
                    self.open_account_panel();
                }
            }
            KeyCode::Char('y') => {
                return Some(ExportAction::Save);
            }
            KeyCode::Char('u') => {
                if !crate::config::is_plus() {
                    open_plus_page();
                    self.flash("opening txxxt.me/plus...".into());
                }
            }
            // PIP controls (call mode only).
            KeyCode::Char('+') | KeyCode::Char('=') => {
                if matches!(self.mode, AppMode::Call { .. }) {
                    if self.pip_scale_idx + 1 < PIP_SCALES.len() {
                        self.pip_scale_idx += 1;
                    }
                }
            }
            KeyCode::Char('-') => {
                if matches!(self.mode, AppMode::Call { .. }) {
                    self.pip_scale_idx = self.pip_scale_idx.saturating_sub(1);
                }
            }
            KeyCode::Char('p') => {
                if matches!(self.mode, AppMode::Call { .. }) {
                    self.pip_corner = self.pip_corner.next();
                }
            }
            KeyCode::Char('m') => {
                if matches!(self.mode, AppMode::Call { .. }) {
                    self.audio_muted = !self.audio_muted;
                    let label = if self.audio_muted { "muted" } else { "unmuted" };
                    self.flash(label.into());
                    self.send_status();
                }
            }
            KeyCode::Char('h') => {
                if matches!(self.mode, AppMode::Call { .. }) {
                    self.camera_hidden = !self.camera_hidden;
                    let label = if self.camera_hidden { "camera off" } else { "camera on" };
                    self.flash(label.into());
                    self.send_status();
                }
            }
            _ => {}
        }
        None
    }

    /// Set a flash message to show in the status bar.
    fn flash(&mut self, msg: String) {
        self.flash_message = Some((msg, Instant::now()));
    }

    fn open_account_panel(&mut self) {
        self.panel = Some(Panel::Account);
        self.account_field = 0;
        self.account_status = None;
        if self.account_key_input.is_empty() {
            if let Some(ref k) = self.user_config.license_key {
                self.account_key_input = k.clone();
            }
        }
    }

    fn open_friends_panel(&mut self) {
        self.panel = Some(Panel::Friends);
        self.panel_cursor = 0;
        self.friends_adding = false;
        self.friends_add_input.clear();
        self.friends_status = None;
        if self.friends_list.is_empty() {
            self.refresh_friends();
        }
    }

    fn refresh_friends(&mut self) {
        if let Some((_, token)) = crate::config::get_account() {
            match crate::net::relay::friends_list(&token) {
                Ok(list) => self.friends_list = list,
                Err(e) => {
                    self.friends_status = Some(format!("{}", e));
                    self.friends_list.clear();
                }
            }
        }
    }

    /// Get the current flash message if still within display time.
    fn active_flash(&self) -> Option<&str> {
        self.flash_message.as_ref().and_then(|(msg, t)| {
            if t.elapsed().as_secs() < FLASH_DISPLAY_SECS {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    /// Refresh directory entries based on current pref_input path.
    fn pref_refresh_dir_entries(&mut self) {
        self.pref_dir_entries = pref_list_dir(&self.pref_input);
    }

    /// Clear tab completion state.
    fn pref_clear_tab(&mut self) {
        self.pref_tab_matches.clear();
        self.pref_tab_index = 0;
        self.pref_tab_base.clear();
    }
}

/// Expand ~ to home directory.
fn pref_expand_tilde(path: &str) -> PathBuf {
    if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            return PathBuf::from(path.replacen('~', home.to_str().unwrap_or("~"), 1));
        }
    }
    PathBuf::from(path)
}

/// Convert a PathBuf back to display form with ~ for home.
fn pref_display_path(path: &PathBuf) -> String {
    let s = path.to_str().unwrap_or("").to_string();
    if let Some(home) = dirs::home_dir().and_then(|h| h.to_str().map(String::from)) {
        s.replace(&home, "~")
    } else {
        s
    }
}

/// Get the parent directory to list based on current input.
fn pref_resolve_parent(input: &str) -> PathBuf {
    let expanded = pref_expand_tilde(input);
    if input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR) {
        expanded
    } else {
        expanded.parent().map(|p| p.to_path_buf()).unwrap_or(expanded)
    }
}

/// Get the partial filename typed after the last separator.
fn pref_partial_name(input: &str) -> &str {
    if input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR) {
        ""
    } else {
        input.rsplit('/').next().unwrap_or("")
    }
}

/// List directory entries matching the current input prefix. Directories first, sorted.
fn pref_list_dir(input: &str) -> Vec<DirEntry> {
    let parent = pref_resolve_parent(input);
    let partial = pref_partial_name(input).to_lowercase();

    let Ok(rd) = std::fs::read_dir(&parent) else {
        return Vec::new();
    };

    let mut entries: Vec<DirEntry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            // Skip hidden files unless user is typing a dot.
            if name.starts_with('.') && !partial.starts_with('.') {
                return None;
            }
            // Filter by partial match.
            if !partial.is_empty() && !name.to_lowercase().starts_with(&partial) {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            // Only show directories (this is a folder picker).
            if !is_dir {
                return None;
            }
            Some(DirEntry { name, is_dir })
        })
        .collect();

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Limit to prevent huge lists.
    entries.truncate(10);
    entries
}

/// Generate tab completion matches for the current input.
fn pref_tab_complete(input: &str) -> Vec<String> {
    let parent = pref_resolve_parent(input);
    let partial = pref_partial_name(input).to_lowercase();

    let Ok(rd) = std::fs::read_dir(&parent) else {
        return Vec::new();
    };

    let prefix = if input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR) {
        input.to_string()
    } else {
        // Everything before the last separator + separator.
        let last_sep = input.rfind('/').map(|i| i + 1).unwrap_or(0);
        input[..last_sep].to_string()
    };

    let mut matches: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if name.starts_with('.') && !partial.starts_with('.') {
                return None;
            }
            if !name.to_lowercase().starts_with(&partial) {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            if !is_dir {
                return None;
            }
            Some(format!("{}{}/", prefix, name))
        })
        .collect();

    matches.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    matches
}

/// Detect the local LAN IP address by opening a UDP socket.
/// This doesn't send any data — it just lets the OS pick the right interface.
/// Run the viewer TUI and immediately join a relay room.
pub fn run_viewer_with_code(
    camera: CameraCapture,
    code: &str,
    incoming_rx: Option<std::sync::mpsc::Receiver<crate::net::presence::IncomingCall>>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.user_config = crate::config::load();
    app.user_config.apply_to(&mut app.config);
    app.incoming_rx = incoming_rx;
    // Auto-join relay room.
    app.start_relay_join(code);

    let result = run_main_loop(&mut app, camera, &mut terminal, None);

    crate::config::save(&crate::config::UserConfig::from_render_config(&app.config, &app.user_config));

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}

/// Run the local webcam viewer TUI.
pub fn run_viewer(
    camera: CameraCapture,
    incoming_rx: Option<std::sync::mpsc::Receiver<crate::net::presence::IncomingCall>>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.user_config = crate::config::load();
    app.user_config.apply_to(&mut app.config);
    app.incoming_rx = incoming_rx;

    // Pre-fetch friends list in background.
    let friends_rx = if let Some((_, token)) = crate::config::get_account() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(list) = crate::net::relay::friends_list(&token) {
                let _ = tx.send(list);
            }
        });
        Some(rx)
    } else {
        None
    };

    let result = run_main_loop(&mut app, camera, &mut terminal, friends_rx);

    crate::config::save(&crate::config::UserConfig::from_render_config(&app.config, &app.user_config));
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}

/// The unified main render/input loop shared by viewer and call modes.
fn run_main_loop(
    app: &mut App,
    mut camera: CameraCapture,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    friends_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
) -> Result<()> {
    let mut friends_rx = friends_rx;
    let target_frame_time = Duration::from_millis(66); // ~15 fps
    let mut last_fps_update = Instant::now();
    let mut frames_since_update = 0u32;
    let mut bg_model = BackgroundModel::new(640, 480, 0.05, 25.0);

    // ONNX person segmenter (loaded lazily when Person mode is first activated).
    let mut segmenter: Option<crate::segmentation::Segmenter> = None;
    let mut last_person_mask: Option<Vec<bool>> = None;
    let mut prev_raw_mask: Option<Vec<bool>> = None;

    // Try to initialize segmenter if Person mode is already active.
    if app.config.bg_mode == crate::render::BgMode::Person {
        let path = crate::segmentation::default_model_path();
        if path.exists() {
            segmenter = crate::segmentation::Segmenter::new(&path).ok();
        }
    }

    while app.running {
        let frame_start = Instant::now();

        // Check relay status.
        app.check_relay();

        // Check for pre-fetched friends list.
        if let Some(ref rx) = friends_rx {
            if let Ok(list) = rx.try_recv() {
                app.friends_list = list;
                friends_rx = None; // consumed
            }
        }

        // Check for incoming calls from presence thread.
        if app.pending_call.is_none() {
            let call = app.incoming_rx.as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some(call) = call {
                // Filter out empty probe messages from presence reconnection.
                if !call.caller.is_empty() {
                    app.pending_call = Some(call);
                }
            }
        }

        // Check remote peer status updates.
        if let Some(ref rx) = app.remote_status_rx {
            while let Ok(status) = rx.try_recv() {
                app.remote_status = Some(status);
            }
        }

        // Check for disconnect.
        if let Some(ref rx) = app.disconnect_rx {
            if rx.try_recv().is_ok() {
                app.flash("connection lost".into());
                app.end_call();
            }
        }

        // 5-minute call limit (txxxt+ users: unlimited).
        if !crate::config::is_plus() {
            if let Some(start) = app.call_start {
                let elapsed = start.elapsed().as_secs();
                let limit: u64 = 5 * 60;
                if elapsed >= limit {
                    app.flash("5 min limit — press [u] to upgrade".into());
                    app.end_call();
                } else if limit - elapsed == 60 {
                    app.flash("1 minute remaining".into());
                } else if limit - elapsed == 30 {
                    app.flash("30 seconds remaining".into());
                }
            }
        }

        // Drain latest remote frame from channel (non-blocking).
        if let Some(ref rx) = app.remote_rx {
            while let Ok(grid) = rx.try_recv() {
                app.remote_grid = Some(grid);
            }
        }

        // Capture camera frame.
        let frame_data = camera.frame_rgb();

        // Lazily init/teardown segmenter based on BgMode or contour.
        let needs_segmenter = app.config.bg_mode == crate::render::BgMode::Person || app.config.contour;
        if needs_segmenter && segmenter.is_none() {
            let path = crate::segmentation::default_model_path();
            if path.exists() {
                match crate::segmentation::Segmenter::new(&path) {
                    Ok(s) => { segmenter = Some(s); }
                    Err(_) => {
                        app.flash("model load failed".into());
                        app.config.contour = false;
                    }
                }
            }
        } else if !needs_segmenter && segmenter.is_some() {
            segmenter = None;
            last_person_mask = None;
            prev_raw_mask = None;
        }

        // Update person segmentation mask if segmenter is running.
        if let Ok((rgb, w, h)) = &frame_data {
            if let Some(ref seg) = segmenter {
                seg.send_frame(rgb, *w, *h);
                if let Some(new_mask) = seg.try_recv_mask() {
                    last_person_mask = Some(match (&last_person_mask, &prev_raw_mask) {
                        (Some(stable), Some(prev_raw)) if stable.len() == new_mask.len() && prev_raw.len() == new_mask.len() => {
                            stable.iter().zip(prev_raw.iter()).zip(new_mask.iter())
                                .map(|((&s, &pr), &n)| if pr == n { n } else { s }).collect()
                        }
                        _ => new_mask.clone(),
                    });
                    prev_raw_mask = Some(new_mask);
                }
            }
        }

        // Build fg_mask for background removal (separate from contour).
        let fg_mask_buf: Option<Vec<bool>> = if let Ok((rgb, w, h)) = &frame_data {
            match app.config.bg_mode {
                crate::render::BgMode::Off => None,
                crate::render::BgMode::Motion => {
                    bg_model.reset_if_size_changed(*w, *h);
                    bg_model.update(rgb);
                    Some(bg_model.foreground_mask(rgb))
                }
                crate::render::BgMode::Person => {
                    last_person_mask.clone()
                }
            }
        } else {
            None
        };

        let in_call = matches!(app.mode, AppMode::Call { .. });

        // Render local ASCII grid (always at full terminal size — PIP display rescales later).
        let ascii_grid: Option<Vec<Vec<AsciiCell>>> = if let Ok((rgb, w, h)) = &frame_data {
            let area = terminal.size()?;
            let raw_cols = area.width.saturating_sub(2);
            let view_cols = if app.config.mode == RenderMode::Normal && app.config.charset.is_wide() { raw_cols / 2 } else { raw_cols };
            let view_rows = area.height.saturating_sub(3);
            let fg_mask: Option<&[bool]> = fg_mask_buf.as_deref();
            let mut grid = render_frame(rgb, *w, *h, view_cols, view_rows, &app.config, fg_mask);
            // Contour overlay uses person mask (independent of bg removal).
            if app.config.contour {
                if let Some(ref person_mask) = last_person_mask {
                    crate::render::apply_contour_overlay(
                        &mut grid, rgb, *w, *h,
                        view_cols as u32, view_rows as u32,
                        &app.config, person_mask,
                    );
                }
            }
            Some(grid)
        } else {
            None
        };

        // Send local frame + audio to peer if in call.
        if in_call {
            use std::io::Write;
            let mut send_ok = true;

            // Send video (skip if camera hidden).
            if !app.camera_hidden {
                if let Some(ref grid) = ascii_grid {
                    let encoded = encode_frame(grid);
                    if let Some(ref mut writer) = app.net_writer {
                        if writer.write_all(&encoded).is_err() {
                            send_ok = false;
                        }
                    }
                }
            }

            // Step 1: Forward received audio to playback + feed AEC render.
            // Must happen BEFORE capture processing so AEC has reference signal.
            if let Some(ref rx) = app.audio_net_rx {
                let mut peak: f32 = 0.0;
                while let Ok(samples) = rx.try_recv() {
                    for &s in &samples {
                        let abs = (s as f32 / 32767.0).abs();
                        if abs > peak { peak = abs; }
                    }
                    // Resample from network rate to local playback rate.
                    let resampled = crate::audio::resample(&samples, crate::audio::NET_SAMPLE_RATE, app.audio_playback_rate);
                    // Feed to AEC as render (speaker) reference.
                    if let Some(ref mut ec) = app.echo_canceller {
                        ec.analyze_render(&resampled);
                    }
                    if let Some(ref tx) = app.audio_playback_tx {
                        let _ = tx.send(resampled);
                    }
                }
                app.audio_level_remote = app.audio_level_remote * 0.7 + peak * 0.3;
            }

            // Step 2: Process captured mic audio through AEC + send.
            if send_ok {
                if let Some(ref rx) = app.audio_capture_rx {
                    let mut peak: f32 = 0.0;
                    while let Ok(samples) = rx.try_recv() {
                        // Process through echo canceller.
                        let processed = if let Some(ref mut ec) = app.echo_canceller {
                            ec.process_capture(&samples)
                        } else {
                            samples
                        };

                        // Track level (post-AEC).
                        for &s in &processed {
                            let abs = (s as f32 / 32767.0).abs();
                            if abs > peak { peak = abs; }
                        }
                        // Resample to network rate and send if not muted.
                        if !app.audio_muted && !processed.is_empty() {
                            let resampled = crate::audio::resample(&processed, app.audio_capture_rate, crate::audio::NET_SAMPLE_RATE);
                            let encoded = encode_audio(&resampled);
                            if let Some(ref mut writer) = app.net_writer {
                                if writer.write_all(&encoded).is_err() {
                                    send_ok = false;
                                    break;
                                }
                            }
                        }
                    }
                    // Smooth decay.
                    app.audio_level_local = app.audio_level_local * 0.7 + peak * 0.3;
                }
            }

            if !send_ok && app.net_writer.is_some() {
                app.end_call();
                app.flash("connection lost".into());
            }
        }

        // Keep last frame data for export.
        if let Some(ref grid) = ascii_grid {
            app.last_frame_text = crate::export::grid_to_text(grid);
            app.last_frame_grid = Some(grid.clone());
        }

        // Snapshot state for draw closure.
        let flash = app.active_flash().map(|s| s.to_string());
        let fps = app.fps;
        let color_on = app.config.color;
        let bg_mode = app.config.bg_mode;
        let brightness = app.config.brightness_threshold;
        let ascii_ref = &ascii_grid;
        let remote_ref = &app.remote_grid;
        let open_panel = app.panel;
        let panel_cursor = app.panel_cursor;
        let current_style = VisualStyle::from_config(&app.config);
        let pref_editing = app.pref_editing;
        let pref_input = app.pref_input.clone();
        let pref_save_dir = app.user_config.save_dir.clone();
        let pref_dir_entries = app.pref_dir_entries.clone();
        let pref_dir_cursor = app.pref_dir_cursor;
        let connect_input = app.connect_input.clone();
        let app_mode = app.mode.clone();
        let relay_code = app.relay_code.clone();
        let remote_status = app.remote_status;
        let pip_corner = app.pip_corner;
        let pip_scale = PIP_SCALES[app.pip_scale_idx] as u16;
        let audio_muted = app.audio_muted;
        let camera_hidden = app.camera_hidden;
        let audio_level_local = app.audio_level_local;
        let _audio_level_remote = app.audio_level_remote;
        let pending_call_caller = app.pending_call.as_ref().map(|c| c.caller.clone());

        terminal.draw(|f| {
            let area = f.area();
            let has_audio_bar = matches!(&app_mode, AppMode::Call { .. });
            let chunks = if has_audio_bar {
                Layout::vertical([
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(area)
            } else {
                Layout::vertical([
                    Constraint::Min(1),
                ])
                .split(area)
            };

            let video_area = chunks[0];
            let audio_bar_area = if has_audio_bar { chunks[1] } else { chunks[0] };

            match &app_mode {
                AppMode::Local
                | AppMode::RelayWaiting | AppMode::RelayJoining => {
                    // Single video panel.
                    match ascii_ref {
                        Some(grid) => {
                            let lines = ascii_to_lines(grid);
                            let (menu_left, menu_right) = build_menu_title(open_panel, current_style, fps, app.show_help);
                            let p = Paragraph::new(lines).block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .title(menu_left)
                                    .title(ratatui::widgets::block::Title::from(menu_right).alignment(ratatui::layout::Alignment::Right)),
                            );
                            f.render_widget(p, video_area);
                        }
                        None => {
                            let (menu_left, menu_right) = build_menu_title(open_panel, current_style, fps, app.show_help);
                            let p = Paragraph::new("Camera error — check permissions")
                                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                                    .title(menu_left)
                                    .title(ratatui::widgets::block::Title::from(menu_right).alignment(ratatui::layout::Alignment::Right)));
                            f.render_widget(p, video_area);
                        }
                    }

                    // Overlay panels on full video area.
                    render_panels(f, video_area, open_panel, panel_cursor, current_style,
                        color_on, bg_mode, app.config.mirror, brightness,
                        pref_editing, &pref_input, pref_save_dir.as_deref(),
                        &pref_dir_entries, pref_dir_cursor, &connect_input, app);
                }
                AppMode::Call { peer_addr } => {
                    // FaceTime layout: remote = full screen, local = PIP top-right.

                    // Remote: full video area.
                    let remote_cam_off = remote_status.map(|s| s.camera_hidden).unwrap_or(false);
                    if remote_cam_off {
                        // Peer's camera is off — show ❌ centered.
                        let inner_h = video_area.height.saturating_sub(2) as usize;
                        let mid_row = inner_h / 2;
                        let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
                        for i in 0..inner_h {
                            if i == mid_row {
                                lines.push(Line::from("📷 off").alignment(ratatui::layout::Alignment::Center));
                            } else {
                                lines.push(Line::from(""));
                            }
                        }
                        let p = Paragraph::new(lines).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .title(" remote — camera off "),
                        );
                        f.render_widget(p, video_area);
                    } else {
                        match remote_ref {
                            Some(grid) if !grid.is_empty() => {
                                let inner_cols = video_area.width.saturating_sub(2) as usize;
                                let inner_rows = video_area.height.saturating_sub(2) as usize;
                                let scaled = crate::net::protocol::rescale_grid(grid, inner_cols, inner_rows);
                                let lines = ascii_to_lines(&scaled);
                                let p = Paragraph::new(lines).block(
                                    Block::default()
                                        .borders(Borders::ALL)
                                        .border_type(BorderType::Rounded)
                                        .title(format!(" {} ", peer_addr)),
                                );
                                f.render_widget(p, video_area);
                            }
                            _ => {
                                let p = Paragraph::new("Waiting for peer...")
                                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" remote "));
                                f.render_widget(p, video_area);
                            }
                        }
                    }

                    // Local: PIP overlay.
                    let pip_w = (video_area.width * pip_scale / 100).max(16);
                    let pip_h = (video_area.height * pip_scale / 100).max(6);
                    let (pip_x, pip_y) = match pip_corner {
                        PipCorner::TopRight => (
                            video_area.x + video_area.width - pip_w - 1,
                            video_area.y + 1,
                        ),
                        PipCorner::TopLeft => (
                            video_area.x + 1,
                            video_area.y + 1,
                        ),
                        PipCorner::BottomRight => (
                            video_area.x + video_area.width - pip_w - 1,
                            video_area.y + video_area.height - pip_h - 1,
                        ),
                        PipCorner::BottomLeft => (
                            video_area.x + 1,
                            video_area.y + video_area.height - pip_h - 1,
                        ),
                    };
                    let pip_rect = Rect::new(pip_x, pip_y, pip_w, pip_h);

                    f.render_widget(Clear, pip_rect);
                    if camera_hidden {
                        // Camera off: show ❌ centered in PIP.
                        let inner_h = pip_h.saturating_sub(2) as usize;
                        let mid_row = inner_h / 2;
                        let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
                        for i in 0..inner_h {
                            if i == mid_row {
                                lines.push(Line::from("❌").alignment(ratatui::layout::Alignment::Center));
                            } else {
                                lines.push(Line::from(""));
                            }
                        }
                        let p = Paragraph::new(lines).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .title(" me "),
                        );
                        f.render_widget(p, pip_rect);
                    } else {
                        match ascii_ref {
                            Some(grid) => {
                                let inner_cols = pip_w.saturating_sub(2) as usize;
                                let inner_rows = pip_h.saturating_sub(2) as usize;
                                let scaled = crate::net::protocol::rescale_grid(grid, inner_cols, inner_rows);
                                let lines = ascii_to_lines(&scaled);
                                let p = Paragraph::new(lines).block(
                                    Block::default()
                                        .borders(Borders::ALL)
                                        .border_type(BorderType::Rounded)
                                        .title(" me "),
                                );
                                f.render_widget(p, pip_rect);
                            }
                            None => {
                                let p = Paragraph::new("no cam")
                                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                                        .title(" me "));
                                f.render_widget(p, pip_rect);
                            }
                        }
                    }

                    // Overlay panels on full video area.
                    render_panels(f, video_area, open_panel, panel_cursor, current_style,
                        color_on, bg_mode, app.config.mirror, brightness,
                        pref_editing, &pref_input, pref_save_dir.as_deref(),
                        &pref_dir_entries, pref_dir_cursor, &connect_input, app);
                }
            }

            // Help overlay (independent of panels, top-right).
            if app.show_help {
                render_help_panel(f, video_area);
            }

            // Flash overlay.
            if let Some(ref flash_text) = flash {
                render_flash_overlay(f, video_area, flash_text);
            }

            // Incoming call popup.
            if let Some(ref caller) = pending_call_caller {
                render_incoming_call_popup(f, video_area, caller);
            }

            // Audio level bar (call mode only) — local mic only.
            if has_audio_bar {
                let bar_w = audio_bar_area.width.saturating_sub(4) as usize;
                if audio_muted {
                    // Muted: show red strikethrough bar.
                    let muted_bar = format!(
                        " 🔇 {}",
                        "─".repeat(bar_w),
                    );
                    let bar_line = Paragraph::new(muted_bar).style(
                        Style::default().fg(Color::Red).bg(Color::DarkGray),
                    );
                    f.render_widget(bar_line, audio_bar_area);
                } else {
                    let filled = ((audio_level_local * bar_w as f32) as usize).min(bar_w);
                    let mic_bar = format!(
                        " 🎙 {}{}",
                        "█".repeat(filled),
                        " ".repeat(bar_w.saturating_sub(filled)),
                    );
                    let bar_line = Paragraph::new(mic_bar).style(
                        Style::default().fg(Color::Green).bg(Color::DarkGray),
                    );
                    f.render_widget(bar_line, audio_bar_area);
                }
            }

            // Status info shown as flash for relay/call modes.
            match &app_mode {
                AppMode::RelayWaiting => {
                    let msg = if let Some(ref code) = relay_code {
                        format!("room: {} — waiting for peer", code)
                    } else {
                        "creating room...".to_string()
                    };
                    render_flash_overlay(f, video_area, &msg);
                }
                AppMode::RelayJoining => {
                    render_flash_overlay(f, video_area, "joining room...");
                }
                _ => {}
            }
        })?;

        // FPS counter.
        frames_since_update += 1;
        let elapsed = last_fps_update.elapsed();
        if elapsed >= Duration::from_secs(1) {
            app.fps = frames_since_update as f32 / elapsed.as_secs_f32();
            frames_since_update = 0;
            last_fps_update = Instant::now();
        }
        app.frame_count += 1;

        // Handle input.
        let remaining = target_frame_time.saturating_sub(frame_start.elapsed());
        if event::poll(remaining)? {
            if let Event::Key(key) = event::read()? {
                let action = app.handle_key(key);
                if let Some(ExportAction::Save) = action {
                    // In call mode: composite PIP onto remote grid (what you see = what you get).
                    let export_grid = if matches!(app.mode, AppMode::Call { .. }) {
                        if let Some(ref remote) = app.remote_grid {
                            let term_size = crossterm::terminal::size().unwrap_or((80, 24));
                            let cols = term_size.0.saturating_sub(2) as usize;
                            let rows = term_size.1.saturating_sub(4) as usize; // status + audio bar + borders
                            let scaled_remote = crate::net::protocol::rescale_grid(remote, cols, rows);

                            let pip_scale = PIP_SCALES[app.pip_scale_idx] as usize;
                            let pip_w = (cols * pip_scale / 100).max(16);
                            let pip_h = (rows * pip_scale / 100).max(6);
                            let (pip_x, pip_y) = match app.pip_corner {
                                PipCorner::TopRight => (cols.saturating_sub(pip_w + 1), 0),
                                PipCorner::TopLeft => (0, 0),
                                PipCorner::BottomRight => (cols.saturating_sub(pip_w + 1), rows.saturating_sub(pip_h + 1)),
                                PipCorner::BottomLeft => (0, rows.saturating_sub(pip_h + 1)),
                            };

                            if let Some(ref local) = app.last_frame_grid {
                                Some(crate::export::composite_pip(&scaled_remote, local, pip_x, pip_y, pip_w, pip_h))
                            } else {
                                Some(scaled_remote)
                            }
                        } else {
                            app.last_frame_grid.clone()
                        }
                    } else {
                        app.last_frame_grid.clone()
                    };

                    if let Some(ref grid) = export_grid {
                        match crate::export::save_to_file(grid, app.user_config.save_dir.as_deref()) {
                            Ok(path) => app.flash(format!("saved to {}", path)),
                            Err(e) => app.flash(format!("Error: {}", e)),
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Render overlay panels and connect panel on the given area.
#[allow(clippy::too_many_arguments)]
/// Build a menu bar title line for the video block border.
fn build_menu_title(open_panel: Option<Panel>, current_style: VisualStyle, fps: f32, show_help: bool) -> (Line<'static>, Line<'static>) {
    let logged_in = crate::config::get_account().is_some();
    let username_display = match crate::config::get_account() {
        Some((un, _)) => format!("@{}", un),
        None => String::new(),
    };

    let highlight = Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);
    let dim = Style::default().fg(Color::DarkGray);

    // (num, key_char, rest_of_label, Panel)
    let items: Vec<(u8, char, &str, Panel)> = if logged_in {
        vec![
            (1, 'v', "isual", Panel::StylePicker),
            (2, 's', "ettings", Panel::Settings),
            (3, 'f', "riends", Panel::Friends),
            (4, 'a', "ccount", Panel::Account),
        ]
    } else {
        vec![
            (1, 'v', "isual", Panel::StylePicker),
            (2, 's', "ettings", Panel::Settings),
            (3, 'a', "ccount", Panel::Account),
        ]
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (num, key_char, rest, panel_type) in &items {
        let is_active = match (&open_panel, panel_type) {
            (Some(a), b) => std::mem::discriminant(a) == std::mem::discriminant(b),
            _ => false,
        };
        if is_active {
            spans.push(Span::styled(format!(" [{}]{} ({}) ", key_char, rest, num), highlight));
        } else {
            spans.push(Span::styled(" [".to_string(), normal));
            spans.push(Span::styled(format!("{}", key_char), Style::default().fg(Color::Yellow)));
            spans.push(Span::styled(format!("]{} ({}) ", rest, num), normal));
        }
        spans.push(Span::styled("│", dim));
    }

    // Right side info.
    let style_label = current_style.label();
    let right_info = if !username_display.is_empty() {
        format!(" {} {} {:.0}fps ", username_display, style_label, fps)
    } else {
        format!(" {} {:.0}fps ", style_label, fps)
    };
    spans.push(Span::styled(right_info, dim));

    let left = Line::from(spans);

    let help_style = if show_help { highlight } else { normal };
    let right = Line::from(vec![Span::styled(" [`]help ", help_style)]);

    (left, right)
}

/// Calculate the x offset for a menu item's dropdown panel.
/// Accounts for border (1 char) + menu item positions.
fn menu_item_x(panel: Panel) -> u16 {
    let logged_in = crate::config::get_account().is_some();
    // Each item renders as " [k]rest (N) │"
    // Width = 1(" ") + 1("[") + 1(key) + 1("]") + rest.len() + 1(" ") + 3("(N)") + 1(" ") + 1("│") = rest.len() + 10
    let items: &[(&str, Panel)] = if logged_in {
        &[
            ("isual", Panel::StylePicker),
            ("ettings", Panel::Settings),
            ("riends", Panel::Friends),
            ("ccount", Panel::Account),
            ("onnect", Panel::Connect),
        ]
    } else {
        &[
            ("isual", Panel::StylePicker),
            ("ettings", Panel::Settings),
            ("ccount", Panel::Account),
            ("onnect", Panel::Connect),
        ]
    };
    let mut x: u16 = 1; // border left
    for (rest, p) in items {
        if std::mem::discriminant(p) == std::mem::discriminant(&panel) {
            return x;
        }
        x += rest.len() as u16 + 10; // " [k]rest (N) │"
    }
    x
}

fn render_panels(
    f: &mut ratatui::Frame,
    area: Rect,
    open_panel: Option<Panel>,
    panel_cursor: usize,
    current_style: VisualStyle,
    color_on: bool,
    bg_mode: crate::render::BgMode,
    mirror_on: bool,
    brightness: u8,
    pref_editing: bool,
    pref_input: &str,
    pref_save_dir: Option<&str>,
    pref_dir_entries: &[DirEntry],
    pref_dir_cursor: Option<usize>,
    connect_input: &str,
    app: &App,
) {
    match open_panel {
        Some(Panel::StylePicker) => {
            render_style_picker(f, area, panel_cursor, current_style);
        }
        Some(Panel::Settings) => {
            render_settings_panel(f, area, panel_cursor, color_on, bg_mode, app.config.contour, mirror_on, brightness);
        }
        Some(Panel::Preference) => {
            render_preference_panel(f, area, panel_cursor, pref_editing, pref_input, pref_save_dir, pref_dir_entries, pref_dir_cursor);
        }
        Some(Panel::Connect) => {
            render_connect_panel(f, area, connect_input);
        }
        Some(Panel::Account) => {
            render_account_panel(f, area, app);
        }
        Some(Panel::Friends) => {
            render_friends_panel(f, area, app);
        }
        None => {}
    }
}

/// Render the style picker overlay on top of the video area.
fn render_style_picker(f: &mut ratatui::Frame, view_area: Rect, cursor: usize, current: VisualStyle) {
    let panel_w: u16 = 20;
    let panel_h = VisualStyle::ALL.len() as u16 + 2; // items + border
    let x = view_area.x + menu_item_x(Panel::StylePicker);
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    // Clear the area behind the panel.
    f.render_widget(Clear, panel_rect);

    let items: Vec<Line<'static>> = VisualStyle::ALL
        .iter()
        .enumerate()
        .map(|(i, &vs)| {
            let marker = if vs == current { "● " } else { "  " };
            let label = format!("{}{}", marker, vs.label());
            let style = if i == cursor {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::styled(label, style)
        })
        .collect();

    let picker = Paragraph::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" style ")
    );
    f.render_widget(picker, panel_rect);
}

/// Render the settings panel overlay.
fn render_settings_panel(
    f: &mut ratatui::Frame,
    view_area: Rect,
    cursor: usize,
    color_on: bool,
    bg_mode: crate::render::BgMode,
    contour_on: bool,
    mirror_on: bool,
    brightness: u8,
) {
    let model_available = crate::segmentation::default_model_path().exists();

    // Build row strings first, then derive panel width from content.
    let rows: Vec<(String, bool)> = SettingsItem::ALL
        .iter()
        .map(|&item| {
            let (value, dimmed) = match item {
                SettingsItem::Color => (if color_on { "ON".into() } else { "OFF".into() }, false),
                SettingsItem::Background => {
                    let on = bg_mode == crate::render::BgMode::Person;
                    let dimmed = !on && !model_available;
                    (if on { "ON".into() } else { "OFF".into() }, dimmed)
                }
                SettingsItem::Contour => {
                    let dimmed = !contour_on && !model_available;
                    (if contour_on { "ON".into() } else { "OFF".into() }, dimmed)
                }
                SettingsItem::Mirror => (if mirror_on { "ON".into() } else { "OFF".into() }, false),
                SettingsItem::Brightness => (format!("◀ {} ▶", brightness), false),
            };
            (format!(" {}  {} ", item.label(), value), dimmed)
        })
        .collect();

    // Add Person (pro) row if not already in Person mode.
    // We show it as a separate hint below the cycling value.

    let max_row_len = rows.iter().map(|(r, _)| r.len()).max().unwrap_or(10);
    // +2 for border left/right, min 14 to fit " settings " title
    let panel_w = (max_row_len as u16 + 4).max(16); // +4 for padding inside border
    let panel_h = SettingsItem::ALL.len() as u16 + 2;
    let x = view_area.x + menu_item_x(Panel::Settings);
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let inner_w = panel_w.saturating_sub(2) as usize; // content width inside border
    let items: Vec<Line<'static>> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (row, dimmed))| {
            // Pad to fill inner width
            let padded = format!("{:<width$}", row, width = inner_w);

            let style = if i == cursor {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
            } else if dimmed {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };
            Line::styled(padded, style)
        })
        .collect();

    let panel = Paragraph::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" settings "),
    );
    f.render_widget(panel, panel_rect);
}

/// Render the preference panel overlay.
fn render_preference_panel(
    f: &mut ratatui::Frame,
    view_area: Rect,
    cursor: usize,
    editing: bool,
    input: &str,
    save_dir: Option<&str>,
    dir_entries: &[DirEntry],
    dir_cursor: Option<usize>,
) {
    let default_dir = "~/Downloads";
    let current_dir = save_dir.unwrap_or(default_dir);

    // Build the input row.
    let input_row = if editing && cursor == 0 {
        format!(" {}: {}▏", PrefItem::SaveDir.label(), input)
    } else {
        format!(" {}: {} ", PrefItem::SaveDir.label(), current_dir)
    };

    // Build directory entry rows (only shown when editing).
    let dir_rows: Vec<String> = if editing {
        dir_entries
            .iter()
            .map(|e| {
                let icon = if e.is_dir { "📁 " } else { "  " };
                let suffix = if e.is_dir { "/" } else { "" };
                format!("  {}{}{}", icon, e.name, suffix)
            })
            .collect()
    } else {
        Vec::new()
    };

    // Calculate panel width from all rows.
    let all_row_lens = std::iter::once(input_row.len())
        .chain(dir_rows.iter().map(|r| r.len()));
    let max_row_len = all_row_lens.max().unwrap_or(10);
    let panel_w = (max_row_len as u16 + 2).max(20).min(view_area.width.saturating_sub(4));

    // Panel height: input(1) + separator(1 if dir entries) + dir entries + border(2).
    let has_entries = editing && !dir_entries.is_empty();
    let panel_h = 1 + if has_entries { 1 + dir_entries.len() as u16 } else { 0 } + 2;

    let x = view_area.x + 1;
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let inner_w = panel_w.saturating_sub(2) as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Input line.
    let input_style = if editing {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if cursor == 0 {
        Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::styled(
        format!("{:<width$}", input_row, width = inner_w),
        input_style,
    ));

    // Directory listing with separator.
    if has_entries {
        let sep = "─".repeat(inner_w);
        lines.push(Line::styled(sep, Style::default().fg(Color::DarkGray)));

        for (i, row) in dir_rows.into_iter().enumerate() {
            let style = if dir_cursor == Some(i) {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else if dir_entries[i].is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::styled(
                format!("{:<width$}", row, width = inner_w),
                style,
            ));
        }
    }

    let title = if editing { " preference (editing) " } else { " preference " };
    let panel = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title),
    );
    f.render_widget(panel, panel_rect);
}

/// Render the connect panel (room code input).
fn render_connect_panel(f: &mut ratatui::Frame, view_area: Rect, input: &str) {
    let row = format!(" room code: {}▏", input);
    let panel_w = (row.len() as u16 + 2).max(30).min(view_area.width.saturating_sub(4));
    let panel_h: u16 = 3;
    let x = view_area.x + menu_item_x(Panel::Connect);
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let inner_w = panel_w.saturating_sub(2) as usize;
    let line = Line::styled(
        format!("{:<width$}", row, width = inner_w),
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    );

    let panel = Paragraph::new(vec![line]).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" connect "),
    );
    f.render_widget(panel, panel_rect);
}

fn render_flash_overlay(f: &mut ratatui::Frame, view_area: Rect, message: &str) {
    let text = format!(" {} ", message);
    let panel_w = (text.len() as u16 + 2).min(view_area.width.saturating_sub(2)); // +2 for border
    let panel_h: u16 = 3; // border + 1 line + border

    // Bottom-center of the video area, 1 row above the border.
    let x = view_area.x + (view_area.width.saturating_sub(panel_w)) / 2;
    let y = view_area.y + view_area.height.saturating_sub(panel_h + 1);
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let content = Paragraph::new(Line::styled(
        text,
        Style::default().fg(Color::Green),
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    f.render_widget(content, panel_rect);
}

/// Convert 2D AsciiCell grid to ratatui Lines with optional color.
fn ascii_to_lines(grid: &[Vec<AsciiCell>]) -> Vec<Line<'static>> {
    grid.iter()
        .map(|row| {
            let spans: Vec<Span<'static>> = row
                .iter()
                .map(|cell| {
                    let style = if let Some((r, g, b)) = cell.color {
                        Style::default().fg(Color::Rgb(r, g, b))
                    } else {
                        Style::default()
                    };
                    Span::styled(cell.ch.to_string(), style)
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

use std::net::SocketAddr;
use crate::net::protocol::{encode_frame, encode_audio, encode_status, decode_message, frame_to_grid, Message, PeerStatus};

/// Open txxxt.me/plus in the default browser.
fn open_plus_page() {
    let url = "https://txxxt.me/plus";
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(url).spawn(); }
    #[cfg(target_os = "linux")]
    { let _ = std::process::Command::new("xdg-open").arg(url).spawn(); }
    #[cfg(target_os = "windows")]
    { let _ = std::process::Command::new("cmd").args(["/c", "start", url]).spawn(); }
}

use crate::net::relay::RELAY_ADDR;

/// Render an incoming call popup overlay.
fn render_incoming_call_popup(f: &mut ratatui::Frame, view_area: Rect, caller: &str) {
    let popup_w: u16 = 31;
    let popup_h: u16 = 6;
    let x = view_area.x + (view_area.width.saturating_sub(popup_w)) / 2;
    let y = view_area.y + (view_area.height.saturating_sub(popup_h)) / 2;
    let popup_rect = Rect::new(x, y, popup_w, popup_h);

    f.render_widget(Clear, popup_rect);

    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Incoming call from  "),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            format!("       @{}        ", caller),
            Style::default().fg(Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [a] Accept   [d] Decline  ",
            Style::default().fg(Color::Green),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(content, popup_rect);
}

/// Render the account panel overlay.
fn render_account_panel(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let logged_in = crate::config::get_account();
    let panel_w: u16 = 48;
    let panel_h: u16 = if logged_in.is_some() { 6 } else { 12 };
    let x = area.x + menu_item_x(Panel::Account);
    let y = area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);
    f.render_widget(Clear, panel_rect);

    if let Some((username, _)) = logged_in {
        let lines = vec![
            Line::raw(""),
            Line::styled(format!("  logged in as @{}", username), Style::default().fg(Color::Green)),
            Line::raw(""),
            Line::styled("  [l] logout  [Esc] close", Style::default().fg(Color::DarkGray)),
        ];
        let content = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" account "),
        );
        f.render_widget(content, panel_rect);
    } else {
        let key_style = if app.account_field == 0 {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let user_style = if app.account_field == 1 {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let mut lines = vec![
            Line::raw(""),
            Line::styled("  license key:", Style::default().fg(Color::DarkGray)),
            Line::styled(format!("  > {}", &app.account_key_input), key_style),
            Line::raw(""),
            Line::styled("  username:", Style::default().fg(Color::DarkGray)),
            Line::styled(format!("  > {}", &app.account_username_input), user_style),
            Line::raw(""),
            Line::styled("  [Enter] register  [Tab] switch", Style::default().fg(Color::DarkGray)),
        ];
        if let Some(ref status) = app.account_status {
            lines.push(Line::styled(format!("  {}", status), Style::default().fg(Color::Yellow)));
        }
        let content = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" account "),
        );
        f.render_widget(content, panel_rect);
    }
}

/// Render the friends panel overlay.
fn render_friends_panel(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let list_len = app.friends_list.len().max(1);
    let panel_h: u16 = (list_len as u16 + 7).min(area.height.saturating_sub(4));
    let panel_w: u16 = 32;
    let x = area.x + menu_item_x(Panel::Friends);
    let y = area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);
    f.render_widget(Clear, panel_rect);

    let mut lines: Vec<Line<'static>> = Vec::new();

    if app.friends_list.is_empty() {
        lines.push(Line::styled("  no friends yet", Style::default().fg(Color::DarkGray)));
    } else {
        for (i, name) in app.friends_list.iter().enumerate() {
            let marker = if !app.friends_adding && i == app.panel_cursor { "> " } else { "  " };
            let style = if !app.friends_adding && i == app.panel_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::styled(format!("{}@{}", marker, name), style));
        }
    }

    lines.push(Line::styled("  ────────────────────────", Style::default().fg(Color::DarkGray)));

    // Add field
    if app.friends_adding {
        lines.push(Line::styled(
            format!("  add: {}", &app.friends_add_input),
            Style::default().fg(Color::Black).bg(Color::White),
        ));
    } else {
        lines.push(Line::styled("  [Tab] add friend", Style::default().fg(Color::DarkGray)));
    }

    // Hints
    if !app.friends_adding && !app.friends_list.is_empty() {
        lines.push(Line::styled("  [Enter] call  [x] remove", Style::default().fg(Color::DarkGray)));
    }

    // Status
    if let Some(ref status) = app.friends_status {
        lines.push(Line::styled(format!("  {}", status), Style::default().fg(Color::Yellow)));
    }

    let content = Paragraph::new(lines).block(
        Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" friends "),
    );
    f.render_widget(content, panel_rect);
}

/// Render the help panel overlay (centered).
fn render_help_panel(f: &mut ratatui::Frame, area: Rect) {
    let lines = vec![
        Line::raw(""),
        Line::styled("  [v]  visual style", Style::default().fg(Color::White)),
        Line::styled("  [s]  settings", Style::default().fg(Color::White)),
        Line::styled("  [a]  account", Style::default().fg(Color::White)),
        Line::styled("  [f]  friends", Style::default().fg(Color::White)),
        Line::raw(""),
        Line::styled("  [r]  create room", Style::default().fg(Color::White)),
        Line::styled("  [c]  connect (join room)", Style::default().fg(Color::White)),
        Line::styled("  [y]  copy to clipboard", Style::default().fg(Color::White)),
        Line::raw(""),
        Line::styled("  call mode:", Style::default().fg(Color::DarkGray)),
        Line::styled("  [m]  mute mic", Style::default().fg(Color::White)),
        Line::styled("  [h]  hide camera", Style::default().fg(Color::White)),
        Line::styled("  [p]  pip position", Style::default().fg(Color::White)),
        Line::styled("  [+/-] pip size", Style::default().fg(Color::White)),
        Line::raw(""),
        Line::styled("  [q]  quit", Style::default().fg(Color::White)),
        Line::styled("  [`]  close help", Style::default().fg(Color::DarkGray)),
    ];
    let panel_w: u16 = 30;
    let panel_h = lines.len() as u16 + 2;
    let x = area.x + area.width.saturating_sub(panel_w + 1); // top-right
    let y = area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);
    f.render_widget(Clear, panel_rect);

    let content = Paragraph::new(lines).block(
        Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" help "),
    );
    f.render_widget(content, panel_rect);
}
