use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::background::BackgroundModel;
use crate::camera::CameraCapture;
use crate::render::{render_frame, AsciiCell, RenderConfig, RenderMode};

const COPIED_DISPLAY_SECS: u64 = 1;

/// Application state.
pub struct App {
    pub config: RenderConfig,
    pub running: bool,
    pub fps: f32,
    frame_count: u64,
    /// Plain-text representation of the last rendered ASCII frame.
    pub last_frame_text: String,
    /// When the user yanked — used to show "Copied!" for 1 s.
    pub copied_at: Option<Instant>,
}

impl App {
    pub fn new() -> Self {
        Self {
            config: RenderConfig::default(),
            running: true,
            fps: 0.0,
            frame_count: 0,
            last_frame_text: String::new(),
            copied_at: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('o') => {
                self.config.mode = match self.config.mode {
                    RenderMode::Normal => RenderMode::Outline,
                    RenderMode::Outline => RenderMode::Normal,
                };
            }
            KeyCode::Char('c') => {
                self.config.color = !self.config.color;
            }
            KeyCode::Char('v') => {
                self.config.charset = self.config.charset.next();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.config.brightness_threshold =
                    self.config.brightness_threshold.saturating_add(5);
            }
            KeyCode::Char('-') => {
                self.config.brightness_threshold =
                    self.config.brightness_threshold.saturating_sub(5);
            }
            _ => {}
        }
    }

    /// Copy `last_frame_text` to the system clipboard.
    /// Sets `copied_at` on success; silently ignores clipboard errors.
    pub fn yank_frame(&mut self, clipboard: &mut arboard::Clipboard) {
        if !self.last_frame_text.is_empty() {
            if clipboard.set_text(self.last_frame_text.clone()).is_ok() {
                self.copied_at = Some(Instant::now());
            }
        }
    }

    /// True if "Copied!" should still be shown in the status bar.
    fn showing_copied(&self) -> bool {
        self.copied_at
            .map(|t| t.elapsed().as_secs() < COPIED_DISPLAY_SECS)
            .unwrap_or(false)
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
            if app.config.mode == RenderMode::Outline {
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

        // Keep last_frame_text up to date for yank.
        if let Some(ref grid) = ascii_grid {
            app.last_frame_text = grid_to_text(grid);
        }

        // Draw TUI
        let showing_copied = app.showing_copied();
        let fps = app.fps;
        let mode = app.config.mode;
        let color_on = app.config.color;
        let charset_label = app.config.charset.label();
        let ascii_ref = &ascii_grid;

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
                            .title(" txxxt "),
                    );
                    f.render_widget(paragraph, view_area);
                }
                None => {
                    let msg = Paragraph::new("Camera error — check permissions")
                        .block(Block::default().borders(Borders::ALL).title(" txxxt "));
                    f.render_widget(msg, view_area);
                }
            }

            // Status bar
            let mode_label = match mode {
                RenderMode::Normal => "NORMAL",
                RenderMode::Outline => "OUTLINE",
            };
            let color_label = if color_on { "COLOR" } else { "MONO" };
            let copy_notice = if showing_copied { " Copied!" } else { "" };
            let status = format!(
                " {} | {} | {} | FPS: {:.0} | [o]utline [c]olor [v]charset [y]ank [q]uit{}",
                mode_label,
                color_label,
                charset_label,
                fps,
                copy_notice,
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
                if key.code == KeyCode::Char('y') {
                    if let Some(ref mut cb) = clipboard {
                        app.yank_frame(cb);
                    }
                } else {
                    app.handle_key(key);
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

/// Convert 2D AsciiCell grid to a plain-text string (rows separated by newlines).
fn grid_to_text(grid: &[Vec<AsciiCell>]) -> String {
    let mut out = String::new();
    for (i, row) in grid.iter().enumerate() {
        for cell in row {
            out.push(cell.ch);
        }
        if i + 1 < grid.len() {
            out.push('\n');
        }
    }
    out
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
