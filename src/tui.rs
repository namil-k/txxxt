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

/// Application state.
pub struct App {
    pub config: RenderConfig,
    pub running: bool,
    pub fps: f32,
    frame_count: u64,
}

impl App {
    pub fn new() -> Self {
        Self {
            config: RenderConfig::default(),
            running: true,
            fps: 0.0,
            frame_count: 0,
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
        // (bg_model requires &mut self which can't be used inside Fn closure).
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

        // Draw TUI
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

            let view_area = chunks[0];
            let status_area = chunks[1];

            // Render ASCII frame if capture succeeded
            if let Ok((rgb, w, h)) = &frame_data {
                let fg_mask: Option<&[bool]> = fg_mask_buf.as_deref();

                let ascii = render_frame(
                    rgb,
                    *w,
                    *h,
                    view_area.width.saturating_sub(2), // account for border
                    view_area.height.saturating_sub(2),
                    &app.config,
                    fg_mask,
                );
                let lines = ascii_to_lines(&ascii);
                let paragraph = Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" txxxt "),
                );
                f.render_widget(paragraph, view_area);
            } else {
                let msg = Paragraph::new("Camera error — check permissions")
                    .block(Block::default().borders(Borders::ALL).title(" txxxt "));
                f.render_widget(msg, view_area);
            }

            // Status bar
            let mode_label = match app.config.mode {
                RenderMode::Normal => "NORMAL",
                RenderMode::Outline => "OUTLINE",
            };
            let color_label = if app.config.color { "COLOR" } else { "MONO" };
            let status = format!(
                " {} | {} | {} | FPS: {:.0} | [o]utline [c]olor [v]charset [q]uit",
                mode_label,
                color_label,
                app.config.charset.label(),
                app.fps
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
                app.handle_key(key);
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
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
