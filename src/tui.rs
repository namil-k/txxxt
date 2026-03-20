use std::io;
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
    Export,
}

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
            SettingsItem::Background => "background",
            SettingsItem::Mirror => "mirror",
            SettingsItem::Brightness => "bright threshold",
        }
    }
}

/// Export panel items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportItem {
    YankClipboard,
    SaveFile,
}

impl ExportItem {
    const ALL: &'static [ExportItem] = &[
        ExportItem::YankClipboard,
        ExportItem::SaveFile,
    ];

    fn label(self) -> &'static str {
        match self {
            ExportItem::YankClipboard => "yank to clipboard",
            ExportItem::SaveFile => "save to file",
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

/// Action returned from export panel that needs to be executed in main loop.
#[derive(Debug)]
pub(crate) enum ExportAction {
    Yank,
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
        }
    }

    /// Handle key when a panel is open. Returns (consumed, optional export action).
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
                    _ => {}
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
                    _ => {}
                }
            }
            Panel::Export => {
                let count = ExportItem::ALL.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.panel_cursor = self.panel_cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.panel_cursor + 1 < count {
                            self.panel_cursor += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let item = ExportItem::ALL[self.panel_cursor];
                        self.panel = None;
                        let action = match item {
                            ExportItem::YankClipboard => ExportAction::Yank,
                            ExportItem::SaveFile => ExportAction::Save,
                        };
                        return (true, Some(action));
                    }
                    KeyCode::Esc | KeyCode::Char('e') => {
                        self.panel = None;
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
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('v') => {
                self.panel = Some(Panel::StylePicker);
                self.panel_cursor = VisualStyle::from_config(&self.config).index();
            }
            KeyCode::Char('f') => {
                self.panel = Some(Panel::Settings);
                self.panel_cursor = 0;
            }
            KeyCode::Char('e') => {
                self.panel = Some(Panel::Export);
                self.panel_cursor = 0;
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
}

/// Run the local webcam viewer TUI.
pub fn run_viewer(mut camera: CameraCapture) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    // Load persisted user settings.
    let user_config = crate::config::load();
    user_config.apply_to(&mut app.config);
    // Clipboard — fall back gracefully if unavailable (headless env).
    let mut clipboard = arboard::Clipboard::new().ok();

    let target_frame_time = Duration::from_millis(100); // ~10 fps
    let mut last_fps_update = Instant::now();
    let mut frames_since_update = 0u32;
    // Background model for Outline mode foreground detection.
    let mut bg_model = BackgroundModel::new(640, 480, 0.05, 25.0);

    while app.running {
        let frame_start = Instant::now();

        // Capture camera frame
        let frame_data = camera.frame_rgb();

        // Update background model and compute foreground mask outside the draw closure
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

        // Render ascii grid outside the draw closure so we can store it.
        let ascii_grid: Option<Vec<Vec<AsciiCell>>> = if let Ok((rgb, w, h)) = &frame_data {
            let area = terminal.size()?;
            // account for border (2 px) and status bar (1 row)
            let view_cols = area.width.saturating_sub(2);
            let view_rows = area.height.saturating_sub(3);
            let fg_mask: Option<&[bool]> = fg_mask_buf.as_deref();
            Some(render_frame(rgb, *w, *h, view_cols, view_rows, &app.config, fg_mask))
        } else {
            None
        };

        // Keep last frame data up to date for export.
        if let Some(ref grid) = ascii_grid {
            app.last_frame_text = crate::export::grid_to_text(grid);
            app.last_frame_grid = Some(grid.clone());
        }

        // Draw TUI
        let flash = app.active_flash().map(|s| s.to_string());
        let fps = app.fps;
        let color_on = app.config.color;
        let bg_on = app.config.bg_removal;
        let mirror_on = app.config.mirror;
        let brightness = app.config.brightness_threshold;
        let ascii_ref = &ascii_grid;
        let open_panel = app.panel;
        let panel_cursor = app.panel_cursor;
        let current_style = VisualStyle::from_config(&app.config);

        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

            let view_area = chunks[0];
            let status_area = chunks[1];

            // Video area
            match ascii_ref {
                Some(grid) => {
                    let lines = ascii_to_lines(grid);
                    let paragraph = Paragraph::new(lines).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" txxxt "),
                    );
                    f.render_widget(paragraph, view_area);
                }
                None => {
                    let msg = Paragraph::new("Camera error — check permissions")
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" txxxt "));
                    f.render_widget(msg, view_area);
                }
            }

            // Overlay panel
            match open_panel {
                Some(Panel::StylePicker) => {
                    render_style_picker(f, view_area, panel_cursor, current_style);
                }
                Some(Panel::Settings) => {
                    render_settings_panel(f, view_area, panel_cursor, color_on, bg_on, mirror_on, brightness);
                }
                Some(Panel::Export) => {
                    render_export_panel(f, view_area, panel_cursor);
                }
                None => {}
            }

            // Status bar
            let style_label = current_style.label();
            let color_label = if color_on { "COLOR" } else { "MONO" };
            let bg_label = if bg_on { " BG" } else { "" };
            let flash_text = flash.as_deref().unwrap_or("");
            let flash_display = if flash_text.is_empty() { String::new() } else { format!(" {}", flash_text) };
            let status = format!(
                " {} | {}{} | FPS: {:.0} | [v]style [f]settings [e]xport [q]uit{}",
                style_label,
                color_label,
                bg_label,
                fps,
                flash_display,
            );
            let status_line = Paragraph::new(status).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White),
            );
            f.render_widget(status_line, status_area);
        })?;

        // Update FPS counter
        frames_since_update += 1;
        let elapsed = last_fps_update.elapsed();
        if elapsed >= Duration::from_secs(1) {
            app.fps = frames_since_update as f32 / elapsed.as_secs_f32();
            frames_since_update = 0;
            last_fps_update = Instant::now();
        }
        app.frame_count += 1;

        // Handle input (non-blocking)
        let remaining = target_frame_time.saturating_sub(frame_start.elapsed());
        if event::poll(remaining)? {
            if let Event::Key(key) = event::read()? {
                let action = app.handle_key(key);
                // Execute export actions that need resources from main loop.
                if let Some(action) = action {
                    match action {
                        ExportAction::Yank => {
                            if let Some(ref mut cb) = clipboard {
                                if let Some(ref grid) = app.last_frame_grid {
                                    if crate::export::yank_to_clipboard(cb, grid, app.config.color) {
                                        app.flash("Copied!".into());
                                    }
                                }
                            }
                        }
                        ExportAction::Save => {
                            if let Some(ref grid) = app.last_frame_grid {
                                match crate::export::save_to_file(grid, app.config.color) {
                                    Ok(filename) => app.flash(format!("Saved: {}", filename)),
                                    Err(e) => app.flash(format!("Error: {}", e)),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Save user settings before exit.
    crate::config::save(&crate::config::UserConfig::from_render_config(&app.config));

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
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

/// Render the export panel overlay.
fn render_export_panel(f: &mut ratatui::Frame, view_area: Rect, cursor: usize) {
    let rows: Vec<&str> = ExportItem::ALL.iter().map(|item| item.label()).collect();
    let max_row_len = rows.iter().map(|r| r.len()).max().unwrap_or(10);
    let panel_w = (max_row_len as u16 + 6).max(12); // padding + border
    let panel_h = ExportItem::ALL.len() as u16 + 2;
    let x = view_area.x + 1;
    let y = view_area.y + 1;
    let panel_rect = Rect::new(x, y, panel_w, panel_h);

    f.render_widget(Clear, panel_rect);

    let inner_w = panel_w.saturating_sub(2) as usize;
    let items: Vec<Line<'static>> = rows
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let padded = format!(" {:<width$}", label, width = inner_w - 1);
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
            .title(" export ")
            .style(Style::default().bg(Color::DarkGray)),
    );
    f.render_widget(panel, panel_rect);
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
