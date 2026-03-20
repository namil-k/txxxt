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
}

/// App mode: local viewer or in-call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Local,
    Call {
        peer_addr: SocketAddr,
    },
    /// Waiting for incoming connection on a port.
    Listening {
        port: u16,
    },
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
    Mirror,
    Brightness,
}

impl SettingsItem {
    const ALL: &'static [SettingsItem] = &[
        SettingsItem::Color,
        SettingsItem::Background,
        SettingsItem::Mirror,
        SettingsItem::Brightness,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsItem::Color => "color",
            SettingsItem::Background => "bg removal",
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
        VisualStyle::Outline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VisualStyle::Charset(cs) => cs.label(),
            VisualStyle::Outline => "outline",
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
        if config.mode == RenderMode::Outline {
            VisualStyle::Outline
        } else {
            VisualStyle::Charset(config.charset)
        }
    }

    /// Index of this style in ALL.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }
}

const FLASH_DISPLAY_SECS: u64 = 2;

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
    /// Text input buffer for Connect panel (IP:port).
    connect_input: String,
    /// Remote frame received from peer (during call).
    remote_grid: Option<Vec<Vec<AsciiCell>>>,
    /// Channel receiver for remote frames.
    remote_rx: Option<mpsc::Receiver<Vec<Vec<AsciiCell>>>>,
    /// TCP writer for sending frames to peer.
    net_writer: Option<std::net::TcpStream>,
    /// Handle to listener thread (for cancellation).
    listener_handle: Option<std::thread::JoinHandle<Option<(std::net::TcpStream, SocketAddr)>>>,
    /// Channel for listener result.
    listener_rx: Option<mpsc::Receiver<(std::net::TcpStream, SocketAddr)>>,
    /// PIP corner position during call.
    pip_corner: PipCorner,
    /// PIP size index into PIP_SCALES.
    pip_scale_idx: usize,
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
            listener_handle: None,
            listener_rx: None,
            pip_corner: PipCorner::TopRight,
            pip_scale_idx: PIP_DEFAULT_SCALE_IDX,
        }
    }

    /// Start a call by connecting to the given address.
    fn start_call(&mut self, addr: &str) {
        match std::net::TcpStream::connect(addr) {
            Ok(stream) => {
                let peer_addr = stream.peer_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
                self.setup_call(stream, peer_addr);
                self.flash(format!("connected to {}", peer_addr));
            }
            Err(e) => {
                self.flash(format!("connection failed: {}", e));
            }
        }
    }

    /// Start listening for incoming connections.
    fn start_listen(&mut self, port: u16) {
        let (tx, rx) = mpsc::channel();

        let handle = std::thread::spawn(move || {
            let listener = match std::net::TcpListener::bind(("0.0.0.0", port)) {
                Ok(l) => l,
                Err(_) => return None,
            };
            listener.set_nonblocking(false).ok();
            match listener.accept() {
                Ok((stream, addr)) => {
                    let _ = tx.send((stream, addr));
                    Some((std::net::TcpStream::connect("0.0.0.0:0").ok()?, addr))
                }
                Err(_) => None,
            }
        });

        self.mode = AppMode::Listening { port };
        self.listener_rx = Some(rx);
        self.listener_handle = Some(handle);

        // Build address string and copy to clipboard.
        let ip = get_local_ip().unwrap_or_else(|| "127.0.0.1".into());
        let addr = format!("{}:{}", ip, port);
        let mut copied = false;
        if let Ok(mut clip) = arboard::Clipboard::new() {
            if clip.set_text(&addr).is_ok() {
                copied = true;
            }
        }
        if copied {
            self.flash(format!("{} copied!", addr));
        } else {
            self.flash(format!("listening — share: {}", addr));
        }
    }

    /// Check if a listener has accepted a connection.
    fn check_listener(&mut self) {
        if let Some(ref rx) = self.listener_rx {
            if let Ok((stream, peer_addr)) = rx.try_recv() {
                self.listener_rx = None;
                self.listener_handle = None;
                self.setup_call(stream, peer_addr);
                self.flash(format!("connected: {}", peer_addr));
            }
        }
    }

    /// Setup call state with an established TCP connection.
    fn setup_call(&mut self, stream: std::net::TcpStream, peer_addr: SocketAddr) {
        stream.set_nonblocking(false).ok();
        let reader_stream = stream.try_clone().expect("failed to clone stream");
        let writer_stream = stream;

        let (remote_tx, remote_rx) = mpsc::channel::<Vec<Vec<AsciiCell>>>();

        // Spawn reader thread.
        std::thread::spawn(move || {
            use std::io::Read;
            let mut reader = reader_stream;
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut tmp = [0u8; 8192];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        let mut latest_grid = None;
                        loop {
                            match decode_frame(&buf) {
                                Some((frame, consumed)) => {
                                    latest_grid = Some(frame_to_grid(&frame));
                                    buf.drain(..consumed);
                                }
                                None => break,
                            }
                        }
                        if let Some(grid) = latest_grid {
                            if remote_tx.send(grid).is_err() {
                                break; // main loop dropped receiver
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        self.mode = AppMode::Call { peer_addr };
        self.remote_rx = Some(remote_rx);
        self.net_writer = Some(writer_stream);
        self.remote_grid = None;
        self.panel = None;
    }

    /// End the current call and return to local mode.
    fn end_call(&mut self) {
        self.mode = AppMode::Local;
        self.remote_rx = None;
        self.net_writer = None;
        self.remote_grid = None;
        self.listener_rx = None;
        self.listener_handle = None;
        self.flash("call ended".into());
    }

    /// Handle key when a panel is open. Returns (consumed, optional export action).
    /// Unrecognized keys return (false, None) so they fall through to global handle_key.
    fn handle_panel_key(&mut self, key: KeyEvent) -> (bool, Option<ExportAction>) {
        let Some(panel) = self.panel else { return (false, None) };

        match panel {
            Panel::StylePicker => {
                let count = VisualStyle::ALL.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.panel_cursor = self.panel_cursor.saturating_sub(1);
                        VisualStyle::ALL[self.panel_cursor].apply(&mut self.config);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.panel_cursor + 1 < count {
                            self.panel_cursor += 1;
                        }
                        VisualStyle::ALL[self.panel_cursor].apply(&mut self.config);
                    }
                    KeyCode::Enter | KeyCode::Esc | KeyCode::Char('v') => {
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
                                self.config.bg_removal = !self.config.bg_removal;
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
                    KeyCode::Esc | KeyCode::Char('f') => {
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
                        KeyCode::Esc | KeyCode::Char(',') => {
                            self.panel = None;
                        }
                        _ => return (false, None),
                    }
                }
            }
            Panel::Connect => {
                // Text input for IP:port address.
                match key.code {
                    KeyCode::Enter => {
                        let addr = self.connect_input.trim().to_string();
                        if !addr.is_empty() {
                            self.panel = None;
                            self.start_call(&addr);
                        }
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
        }
        (true, None)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ExportAction> {
        // If a panel is open, route input there first.
        let (consumed, action) = self.handle_panel_key(key);
        if consumed {
            return action;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                // In call/listening mode: end call. In local mode: quit.
                match self.mode {
                    AppMode::Call { .. } | AppMode::Listening { .. } => {
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
            KeyCode::Char('l') => {
                if self.mode == AppMode::Local {
                    self.start_listen(7878);
                }
            }
            KeyCode::Char('v') => {
                self.panel = Some(Panel::StylePicker);
                self.panel_cursor = VisualStyle::from_config(&self.config).index();
            }
            KeyCode::Char('f') => {
                self.panel = Some(Panel::Settings);
                self.panel_cursor = 0;
            }
            KeyCode::Char(',') => {
                self.panel = Some(Panel::Preference);
                self.panel_cursor = 0;
                self.pref_editing = false;
            }
            KeyCode::Char('y') => {
                return Some(ExportAction::Save);
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
            _ => {}
        }
        None
    }

    /// Set a flash message to show in the status bar.
    fn flash(&mut self, msg: String) {
        self.flash_message = Some((msg, Instant::now()));
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
fn get_local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

/// Run the local webcam viewer TUI.
pub fn run_viewer(camera: CameraCapture) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.user_config = crate::config::load();
    app.user_config.apply_to(&mut app.config);

    let result = run_main_loop(&mut app, camera, &mut terminal);

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
) -> Result<()> {
    let target_frame_time = Duration::from_millis(66); // ~15 fps
    let mut last_fps_update = Instant::now();
    let mut frames_since_update = 0u32;
    let mut bg_model = BackgroundModel::new(640, 480, 0.05, 25.0);

    while app.running {
        let frame_start = Instant::now();

        // Check if a listener accepted a connection.
        app.check_listener();

        // Drain latest remote frame from channel (non-blocking).
        if let Some(ref rx) = app.remote_rx {
            while let Ok(grid) = rx.try_recv() {
                app.remote_grid = Some(grid);
            }
        }

        // Capture camera frame.
        let frame_data = camera.frame_rgb();

        let fg_mask_buf: Option<Vec<bool>> = if let Ok((rgb, w, h)) = &frame_data {
            bg_model.reset_if_size_changed(*w, *h);
            bg_model.update(rgb);
            if app.config.bg_removal {
                Some(bg_model.foreground_mask(rgb))
            } else {
                None
            }
        } else {
            None
        };

        let in_call = matches!(app.mode, AppMode::Call { .. });

        // Render local ASCII grid (always at full terminal size — PIP display rescales later).
        let ascii_grid: Option<Vec<Vec<AsciiCell>>> = if let Ok((rgb, w, h)) = &frame_data {
            let area = terminal.size()?;
            let view_cols = area.width.saturating_sub(2);
            let view_rows = area.height.saturating_sub(3);
            let fg_mask: Option<&[bool]> = fg_mask_buf.as_deref();
            Some(render_frame(rgb, *w, *h, view_cols, view_rows, &app.config, fg_mask))
        } else {
            None
        };

        // Send local frame to peer if in call.
        if in_call {
            if let Some(ref grid) = ascii_grid {
                use std::io::Write;
                let encoded = encode_frame(grid);
                let send_ok = if let Some(ref mut writer) = app.net_writer {
                    writer.write_all(&encoded).is_ok()
                } else {
                    false
                };
                if !send_ok && app.net_writer.is_some() {
                    app.end_call();
                    app.flash("connection lost".into());
                }
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
        let bg_on = app.config.bg_removal;
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
        let pip_corner = app.pip_corner;
        let pip_scale = PIP_SCALES[app.pip_scale_idx] as u16;

        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

            let video_area = chunks[0];
            let status_area = chunks[1];

            match &app_mode {
                AppMode::Local | AppMode::Listening { .. } => {
                    // Single video panel.
                    match ascii_ref {
                        Some(grid) => {
                            let lines = ascii_to_lines(grid);
                            let title = match &app_mode {
                                AppMode::Listening { port } => format!(" txxxt — listening on :{} ", port),
                                _ => " txxxt ".to_string(),
                            };
                            let p = Paragraph::new(lines).block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .title(title),
                            );
                            f.render_widget(p, video_area);
                        }
                        None => {
                            let p = Paragraph::new("Camera error — check permissions")
                                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" txxxt "));
                            f.render_widget(p, video_area);
                        }
                    }

                    // Overlay panels on full video area.
                    render_panels(f, video_area, open_panel, panel_cursor, current_style,
                        color_on, bg_on, app.config.mirror, brightness,
                        pref_editing, &pref_input, pref_save_dir.as_deref(),
                        &pref_dir_entries, pref_dir_cursor, &connect_input);
                }
                AppMode::Call { peer_addr } => {
                    // FaceTime layout: remote = full screen, local = PIP top-right.

                    // Remote: full video area.
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

                    // Local: PIP overlay — size and position configurable.
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

                    // Overlay panels on full video area.
                    render_panels(f, video_area, open_panel, panel_cursor, current_style,
                        color_on, bg_on, app.config.mirror, brightness,
                        pref_editing, &pref_input, pref_save_dir.as_deref(),
                        &pref_dir_entries, pref_dir_cursor, &connect_input);
                }
            }

            // Flash overlay.
            if let Some(ref flash_text) = flash {
                render_flash_overlay(f, video_area, flash_text);
            }

            // Status bar.
            let style_label = current_style.label();
            let color_label = if color_on { "COLOR" } else { "MONO" };
            let bg_label = if bg_on { " BG" } else { "" };
            let mode_info = match &app_mode {
                AppMode::Local => "[c]all [l]isten".to_string(),
                AppMode::Listening { port } => format!("listening :{} | [q]cancel", port),
                AppMode::Call { peer_addr } => format!(
                    "{} | [p]ip:{} [+/-]{}% | [q]hangup",
                    peer_addr, pip_corner.label(), pip_scale,
                ),
            };
            let status = format!(
                " {} | {}{} | FPS: {:.0} | {} | [v]style [f]settings [y]save",
                style_label, color_label, bg_label, fps, mode_info,
            );
            let status_line = Paragraph::new(status).style(
                Style::default().fg(Color::Black).bg(Color::White),
            );
            f.render_widget(status_line, status_area);
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
                    if let Some(ref grid) = app.last_frame_grid {
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
fn render_panels(
    f: &mut ratatui::Frame,
    area: Rect,
    open_panel: Option<Panel>,
    panel_cursor: usize,
    current_style: VisualStyle,
    color_on: bool,
    bg_on: bool,
    mirror_on: bool,
    brightness: u8,
    pref_editing: bool,
    pref_input: &str,
    pref_save_dir: Option<&str>,
    pref_dir_entries: &[DirEntry],
    pref_dir_cursor: Option<usize>,
    connect_input: &str,
) {
    match open_panel {
        Some(Panel::StylePicker) => {
            render_style_picker(f, area, panel_cursor, current_style);
        }
        Some(Panel::Settings) => {
            render_settings_panel(f, area, panel_cursor, color_on, bg_on, mirror_on, brightness);
        }
        Some(Panel::Preference) => {
            render_preference_panel(f, area, panel_cursor, pref_editing, pref_input, pref_save_dir, pref_dir_entries, pref_dir_cursor);
        }
        Some(Panel::Connect) => {
            render_connect_panel(f, area, connect_input);
        }
        None => {}
    }
}

/// Render the style picker overlay on top of the video area.
fn render_style_picker(f: &mut ratatui::Frame, view_area: Rect, cursor: usize, current: VisualStyle) {
    let panel_w: u16 = 20;
    let panel_h = VisualStyle::ALL.len() as u16 + 2; // items + border
    // Position: top-left corner of view area.
    let x = view_area.x + 1;
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
            .style(Style::default().bg(Color::DarkGray)),
    );
    f.render_widget(picker, panel_rect);
}

/// Render the settings panel overlay.
fn render_settings_panel(
    f: &mut ratatui::Frame,
    view_area: Rect,
    cursor: usize,
    color_on: bool,
    bg_on: bool,
    mirror_on: bool,
    brightness: u8,
) {
    // Build row strings first, then derive panel width from content.
    let rows: Vec<String> = SettingsItem::ALL
        .iter()
        .map(|&item| {
            let value: String = match item {
                SettingsItem::Color => if color_on { "ON".into() } else { "OFF".into() },
                SettingsItem::Background => if bg_on { "ON".into() } else { "OFF".into() },
                SettingsItem::Mirror => if mirror_on { "ON".into() } else { "OFF".into() },
                SettingsItem::Brightness => format!("◀ {} ▶", brightness),
            };
            format!(" {}  {} ", item.label(), value)
        })
        .collect();

    let max_row_len = rows.iter().map(|r| r.len()).max().unwrap_or(10);
    // +2 for border left/right, min 14 to fit " settings " title
    let panel_w = (max_row_len as u16 + 2).max(14);
    let panel_h = SettingsItem::ALL.len() as u16 + 2;
    let x = view_area.x + 1;
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let inner_w = panel_w.saturating_sub(2) as usize; // content width inside border
    let items: Vec<Line<'static>> = rows
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            // Pad to fill inner width
            let padded = format!("{:<width$}", row, width = inner_w);
            let style = if i == cursor {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
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
            .title(" settings ")
            .style(Style::default().bg(Color::DarkGray)),
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
            .title(title)
            .style(Style::default().bg(Color::DarkGray)),
    );
    f.render_widget(panel, panel_rect);
}

/// Render a flash message as a bordered overlay at the bottom of the video area.
/// Render the connect panel (IP:port input).
fn render_connect_panel(f: &mut ratatui::Frame, view_area: Rect, input: &str) {
    let row = format!(" address: {}▏", input);
    let panel_w = (row.len() as u16 + 2).max(30).min(view_area.width.saturating_sub(4));
    let panel_h: u16 = 3;
    let x = view_area.x + 1;
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
            .title(" connect ")
            .style(Style::default().bg(Color::DarkGray)),
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
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(Color::DarkGray)),
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
use crate::net::protocol::{encode_frame, decode_frame, frame_to_grid};

/// CLI entry point: connect to peer, then run viewer in call mode.
pub fn run_call(
    camera: CameraCapture,
    stream: std::net::TcpStream,
    peer_addr: SocketAddr,
) -> Result<()> {
    // We reuse run_viewer but pre-configure the call state.
    // Setup terminal first, then create app with call state.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.user_config = crate::config::load();
    app.user_config.apply_to(&mut app.config);
    app.setup_call(stream, peer_addr);

    // Run the same unified loop via an internal helper.
    let result = run_main_loop(&mut app, camera, &mut terminal);

    crate::config::save(&crate::config::UserConfig::from_render_config(&app.config, &app.user_config));
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}


