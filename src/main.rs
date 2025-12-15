mod scanner;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph},
};
use scanner::{AppInfo, scan_applications_with_progress};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
    ratatui::restore();
    result
}

enum AppState {
    Loading,
    Ready,
    PopupNoSelection,
    PopupPasswordInput,
    Trimming,
}

/// The main application which holds the state and logic of the application.
pub struct App {
    /// Is the application running?
    running: bool,
    /// List of scanned applications
    apps: Vec<AppInfo>,
    /// Currently selected index in the list
    selected_index: usize,
    /// Current state of the app
    state: AppState,
    /// List state for scrolling
    list_state: ListState,
    /// Loading spinner frame
    loading_frame: usize,
    /// Current scan progress
    scan_progress: usize,
    /// Total items to scan
    scan_total: usize,
    /// Current trim progress
    trim_progress: usize,
    /// Total items to trim
    trim_total: usize,
    /// Current app being trimmed
    trim_current: String,
    /// Shared state for trimming progress
    trim_progress_state: Option<Arc<Mutex<(usize, usize, String)>>>,
    /// Shared state for trim result
    trim_result_state: Option<Arc<Mutex<Option<Vec<AppInfo>>>>>,
    /// Password input buffer
    password_input: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Construct a new instance of [`App`].
    pub fn new() -> Self {
        Self {
            running: false,
            apps: Vec::new(),
            selected_index: 0,
            state: AppState::Loading,
            list_state: ListState::default(),
            loading_frame: 0,
            scan_progress: 0,
            scan_total: 0,
            trim_progress: 0,
            trim_total: 0,
            trim_current: String::new(),
            trim_progress_state: None,
            trim_result_state: None,
            password_input: String::new(),
        }
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        self.running = true;
        let progress = Arc::new(Mutex::new((0usize, 0usize)));
        let apps_result = Arc::new(Mutex::new(None));

        let progress_clone = Arc::clone(&progress);
        let apps_clone = Arc::clone(&apps_result);
        thread::spawn(move || {
            let apps = scan_applications_with_progress(|current, total, _name| {
                if let Ok(mut p) = progress_clone.lock() {
                    *p = (current, total);
                }
            });
            if let Ok(mut result) = apps_clone.lock() {
                *result = Some(apps);
            }
        });

        while self.running {
            if matches!(self.state, AppState::Loading) {
                if let Ok(p) = progress.lock() {
                    self.scan_progress = p.0;
                    self.scan_total = p.1;
                }
                if let Ok(mut result) = apps_result.lock()
                    && let Some(apps) = result.take()
                {
                    self.apps = apps;
                    self.state = AppState::Ready;
                }
            }
            if matches!(self.state, AppState::Trimming) {
                if let Some(ref progress_state) = self.trim_progress_state
                    && let Ok(p) = progress_state.lock()
                {
                    self.trim_progress = p.0;
                    self.trim_total = p.1;
                    self.trim_current = p.2.clone();
                }

                // Check if trimming is complete
                let trimming_done = if let Some(ref result_state) = self.trim_result_state {
                    if let Ok(mut result) = result_state.lock() {
                        if let Some(apps) = result.take() {
                            self.apps = apps;
                            self.selected_index = 0;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if trimming_done {
                    self.state = AppState::Ready;
                    self.trim_progress_state = None;
                    self.trim_result_state = None;
                }
            }
            terminal.draw(|frame| self.render(frame))?;
            if matches!(self.state, AppState::Loading | AppState::Trimming) {
                if poll(Duration::from_millis(50))? {
                    self.handle_crossterm_events()?;
                }
            } else {
                self.handle_crossterm_events()?;
            }
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        match self.state {
            AppState::Loading => {
                let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let spinner = spinner_frames[self.loading_frame % spinner_frames.len()];
                self.loading_frame += 1;

                let vertical_chunks = Layout::vertical([
                    Constraint::Percentage(40),
                    Constraint::Length(8),
                    Constraint::Percentage(40),
                ])
                .split(area);

                let horizontal_chunks = Layout::horizontal([
                    Constraint::Percentage(25),
                    Constraint::Percentage(50),
                    Constraint::Percentage(25),
                ])
                .split(vertical_chunks[1]);

                let content = Layout::vertical([Constraint::Length(3), Constraint::Length(3)])
                    .split(horizontal_chunks[1]);

                let progress_ratio = if self.scan_total > 0 {
                    self.scan_progress as f64 / self.scan_total as f64
                } else {
                    0.0
                };

                let title = if self.scan_total > 0 {
                    format!(
                        "{} Scanning {}/{}",
                        spinner, self.scan_progress, self.scan_total
                    )
                } else {
                    format!("{} Initializing scan...", spinner)
                };

                frame.render_widget(
                    Paragraph::new(title)
                        .style(Style::default().fg(Color::Cyan))
                        .centered(),
                    content[0],
                );

                let gauge = Gauge::default()
                    .block(Block::default().borders(Borders::ALL).title("BinTrim"))
                    .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
                    .ratio(progress_ratio);

                frame.render_widget(gauge, content[1]);
            }
            AppState::Ready => {
                // Split the screen: header + main list + summary at bottom
                let chunks = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(8),
                ])
                .split(area);

                self.render_header(frame, chunks[0]);
                self.render_app_list(frame, chunks[1]);
                self.render_summary(frame, chunks[2]);
            }
            AppState::PopupNoSelection => {
                let chunks = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(8),
                ])
                .split(area);

                self.render_header(frame, chunks[0]);
                self.render_app_list(frame, chunks[1]);
                self.render_summary(frame, chunks[2]);

                // Render popup on top
                self.render_no_selection_popup(frame, area);
            }
            AppState::PopupPasswordInput => {
                // Render the main UI in the background
                let chunks = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(8),
                ])
                .split(area);

                self.render_header(frame, chunks[0]);
                self.render_app_list(frame, chunks[1]);
                self.render_summary(frame, chunks[2]);
                self.render_password_popup(frame, area);
            }
            AppState::Trimming => {
                let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let spinner = spinner_frames[self.loading_frame % spinner_frames.len()];
                self.loading_frame += 1;

                let vertical_chunks = Layout::vertical([
                    Constraint::Percentage(40),
                    Constraint::Length(8),
                    Constraint::Percentage(40),
                ])
                .split(area);
                let horizontal_chunks = Layout::horizontal([
                    Constraint::Percentage(25),
                    Constraint::Percentage(50),
                    Constraint::Percentage(25),
                ])
                .split(vertical_chunks[1]);

                let content = Layout::vertical([Constraint::Length(3), Constraint::Length(3)])
                    .split(horizontal_chunks[1]);

                let progress_ratio = if self.trim_total > 0 {
                    self.trim_progress as f64 / self.trim_total as f64
                } else {
                    0.0
                };

                let title = if self.trim_total > 0 {
                    format!(
                        "{} Trimming: {} ({}/{})",
                        spinner, self.trim_current, self.trim_progress, self.trim_total
                    )
                } else {
                    format!("{} Preparing to trim...", spinner)
                };

                frame.render_widget(
                    Paragraph::new(title)
                        .style(Style::default().fg(Color::Yellow))
                        .centered(),
                    content[0],
                );

                let gauge = Gauge::default()
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("BinTrim - Trimming Applications"),
                    )
                    .gauge_style(Style::default().fg(Color::Yellow).bg(Color::Black))
                    .ratio(progress_ratio);

                frame.render_widget(gauge, content[1]);
            }
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header_line = Line::from(vec![
            Span::styled(format!("{:<4}", ""), Style::default()),
            Span::styled(
                format!("{:<30}", "Name"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<20}", "Architectures"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Pruneable Size",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let header = Paragraph::new(header_line)
            .block(Block::default().borders(Borders::ALL).title(
            "Applications (Space: toggle | a: select all | Enter: trim | ↑/↓: navigate | q: quit)",
        ));

        frame.render_widget(header, area);
    }

    fn render_app_list(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .apps
            .iter()
            .enumerate()
            .map(|(i, app)| {
                let checkbox = if app.has_x86_64() {
                    if app.selected { "[x]" } else { "[ ]" }
                } else {
                    "[-]"
                };

                let arch_display = app.architectures_display();

                // Show only x86_64 size (pruneable size)
                let size_display = if app.has_x86_64() {
                    format!("{:.2} MB", app.x86_64_size_mb())
                } else {
                    "0 MB".to_string()
                };

                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", checkbox),
                        if app.has_x86_64() {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(
                        format!("{:<30}", app.name),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("{:<20}", arch_display),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(size_display, Style::default().fg(Color::Yellow)),
                ]);

                let style = if i == self.selected_index {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                ListItem::new(line).style(style)
            })
            .collect();

        let list = List::new(items).block(Block::default().borders(Borders::ALL));

        self.list_state.select(Some(self.selected_index));
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_summary(&self, frame: &mut Frame, area: Rect) {
        let total_apps_with_x86 = self.apps.iter().filter(|app| app.has_x86_64()).count();

        let total_x86_size: f64 = self
            .apps
            .iter()
            .filter(|app| app.has_x86_64())
            .map(|app| app.x86_64_size_mb())
            .sum();

        let selected_apps = self
            .apps
            .iter()
            .filter(|app| app.selected && app.has_x86_64())
            .count();

        let estimated_prune_size: f64 = self
            .apps
            .iter()
            .filter(|app| app.selected && app.has_x86_64())
            .map(|app| app.x86_64_size_mb())
            .sum();

        let prune_size_display = if estimated_prune_size > 0.0 {
            format!("{:.2} MB", estimated_prune_size)
        } else {
            "-".to_string()
        };

        let summary_text = vec![
            Line::from(vec![
                Span::styled(
                    "Applications with x86_64: ",
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{}", total_apps_with_x86),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Total x86_64 size: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{:.2} MB", total_x86_size),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Selected applications: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{}", selected_apps),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Estimated prune size: ", Style::default().fg(Color::White)),
                Span::styled(
                    prune_size_display,
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        let summary = Paragraph::new(summary_text)
            .block(Block::default().borders(Borders::ALL).title("Summary"));

        frame.render_widget(summary, area);
    }

    fn render_no_selection_popup(&self, frame: &mut Frame, area: Rect) {
        let popup_area = Self::centered_rect(50, 30, area);

        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "No applications selected",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Please select at least one application to trim."),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter or Esc to continue",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let popup = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Warning"))
            .centered();

        frame.render_widget(Clear, popup_area);
        frame.render_widget(popup, popup_area);
    }

    fn render_password_popup(&self, frame: &mut Frame, area: Rect) {
        let selected_count = self
            .apps
            .iter()
            .filter(|app| app.selected && app.has_x86_64())
            .count();

        let popup_area = Self::centered_rect(60, 40, area);
        let password_display = "*".repeat(self.password_input.len());

        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("{} Application(s) will be trimmed", selected_count),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("This will remove the x86_64 architecture"),
            Line::from("from the selected applications."),
            Line::from(""),
            Line::from(Span::styled(
                "Enter sudo password:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                password_display,
                Style::default().fg(Color::White),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter to confirm, Esc to cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let popup = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Sudo Authentication"),
            )
            .centered();

        frame.render_widget(Clear, popup_area);
        frame.render_widget(popup, popup_area);
    }

    fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::vertical([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

        Layout::horizontal([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
    }

    fn handle_crossterm_events(&mut self) -> color_eyre::Result<()> {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key_event(key),
            Event::Mouse(_) => {}
            Event::Resize(_, _) => {}
            _ => {}
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    fn on_key_event(&mut self, key: KeyEvent) {
        match self.state {
            AppState::Ready => match (key.modifiers, key.code) {
                (_, KeyCode::Esc | KeyCode::Char('q'))
                | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => self.quit(),
                (_, KeyCode::Down | KeyCode::Char('j')) => self.move_down(),
                (_, KeyCode::Up | KeyCode::Char('k')) => self.move_up(),
                (_, KeyCode::Char(' ')) => self.toggle_selected(),
                (_, KeyCode::Char('a')) => self.toggle_select_all(),
                (_, KeyCode::Enter) => self.start_trim(),
                _ => {}
            },
            AppState::PopupNoSelection => match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.state = AppState::Ready;
                }
                _ => {}
            },
            AppState::PopupPasswordInput => match key.code {
                KeyCode::Char(c) => {
                    self.password_input.push(c);
                }
                KeyCode::Backspace => {
                    self.password_input.pop();
                }
                KeyCode::Enter => {
                    if !self.password_input.is_empty() {
                        self.execute_trim();
                    }
                }
                KeyCode::Esc => {
                    self.password_input.clear();
                    self.state = AppState::Ready;
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn move_down(&mut self) {
        if !self.apps.is_empty() && self.selected_index < self.apps.len() - 1 {
            self.selected_index += 1;
        }
    }

    fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    fn toggle_selected(&mut self) {
        if let Some(app) = self.apps.get_mut(self.selected_index)
            && app.has_x86_64()
        {
            app.selected = !app.selected;
        }
    }

    fn toggle_select_all(&mut self) {
        let all_selected = self
            .apps
            .iter()
            .filter(|app| app.has_x86_64())
            .all(|app| app.selected);
        let new_state = !all_selected;

        for app in &mut self.apps {
            if app.has_x86_64() {
                app.selected = new_state;
            }
        }
    }

    fn start_trim(&mut self) {
        let selected_count = self
            .apps
            .iter()
            .filter(|app| app.selected && app.has_x86_64())
            .count();

        if selected_count == 0 {
            self.state = AppState::PopupNoSelection;
        } else {
            self.password_input.clear();
            self.state = AppState::PopupPasswordInput;
        }
    }

    fn execute_trim(&mut self) {
        let apps_to_trim: Vec<_> = self
            .apps
            .iter()
            .filter(|app| app.selected && app.has_x86_64())
            .cloned()
            .collect();

        let password = self.password_input.clone();
        self.password_input.clear();
        self.state = AppState::Trimming;

        let progress = Arc::new(Mutex::new((0usize, apps_to_trim.len(), String::new())));
        let apps_result = Arc::new(Mutex::new(None));

        // Save references to Arc
        self.trim_progress_state = Some(Arc::clone(&progress));
        self.trim_result_state = Some(Arc::clone(&apps_result));

        let progress_clone = Arc::clone(&progress);
        let apps_clone = Arc::clone(&apps_result);
        thread::spawn(move || {
            // Trim each selected app
            for (index, app) in apps_to_trim.iter().enumerate() {
                if let Ok(mut p) = progress_clone.lock() {
                    *p = (index + 1, apps_to_trim.len(), app.name.clone());
                }

                // Remove x86_64 architecture (requires sudo)
                let binary_path_str = app.binary_path.to_string_lossy();
                let temp_path = format!("{}.tmp", binary_path_str);

                // Attempt to remove x86, write to tmp
                let lipo_cmd = Command::new("sudo")
                    .arg("-S") // Read password from stdin
                    .arg("lipo")
                    .arg(&*binary_path_str)
                    .arg("-remove")
                    .arg("x86_64")
                    .arg("-output")
                    .arg(&temp_path)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                if let Ok(mut child) = lipo_cmd {
                    // Write password to stdin and flush
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = writeln!(stdin, "{}", password);
                        let _ = stdin.flush();
                        drop(stdin);
                    }

                    if let Ok(status) = child.wait()
                        && status.success()
                    {
                        // Replace original with temp file using sudo (password should be cached)
                        let mv_cmd = Command::new("sudo")
                            .arg("-n") // Non-interactive, use cached credentials
                            .arg("mv")
                            .arg(&temp_path)
                            .arg(&*binary_path_str)
                            .output();

                        let _ = mv_cmd;
                    }
                }
            }

            // Rescan
            let new_apps = scan_applications_with_progress(|_, _, _| {});

            if let Ok(mut result) = apps_clone.lock() {
                *result = Some(new_apps);
            }
        });
    }

    fn quit(&mut self) {
        self.running = false;
    }
}
