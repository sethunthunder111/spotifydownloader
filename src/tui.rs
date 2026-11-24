//! Full-screen terminal interface, shown when spotify-dl is started without a subcommand.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthChar;

use crate::config::Config;
use crate::jobs::{self, Job, Settings, Sink, run_job};

const LOG_LIMIT: usize = 1000;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Song,
    Album,
    Playlist,
    Search,
    File,
    Login,
    Settings,
    Quit,
}

const MENU: &[(&str, Action)] = &[
    ("Download song", Action::Song),
    ("Download album", Action::Album),
    ("Download playlist", Action::Playlist),
    ("Search & download", Action::Search),
    ("Download from file", Action::File),
    ("Log in to Spotify", Action::Login),
    ("Settings", Action::Settings),
    ("Quit", Action::Quit),
];

enum Mode {
    Menu,
    Input(Action),
}

struct App {
    settings: Settings,
    config: Config,
    menu: ListState,
    mode: Mode,
    input: String,
    logs: Vec<String>,
    status: String,
    /// What the running job is doing right now.
    progress: String,
    started: Option<Instant>,
    sink: Sink,
    rx: mpsc::UnboundedReceiver<jobs::Event>,
    job: Option<JoinHandle<()>>,
    quit: bool,
}

/// Runs the full-screen UI until the user quits.
pub async fn run(settings: Settings, config: Config) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App {
        settings,
        config,
        menu: ListState::default().with_selected(Some(0)),
        mode: Mode::Menu,
        input: String::new(),
        logs: vec!["Welcome. Pick an action with the arrow keys and press Enter.".into()],
        status: String::new(),
        progress: String::new(),
        started: None,
        sink: Sink::Channel(tx),
        rx,
        job: None,
        quit: false,
    };

    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal).await;
    ratatui::restore();

    if let Some(job) = app.job.take() {
        job.abort();
    }
    result
}

impl App {
    async fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            while let Ok(event) = self.rx.try_recv() {
                match event {
                    jobs::Event::Log(line) => self.push_log(&line),
                    jobs::Event::Progress(text) => self.progress = text,
                }
            }
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                self.on_key(key);
            }
        }
        Ok(())
    }

    fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Adds a message to the log. Multi-line messages become several lines, and control
    /// characters are removed: written raw, they would corrupt the screen.
    fn push_log(&mut self, message: &str) {
        for line in message.lines() {
            let clean: String = line
                .replace('\t', "    ")
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            self.logs.push(clean);
        }
        if self.logs.len() > LOG_LIMIT {
            self.logs.drain(..self.logs.len() - LOG_LIMIT);
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Menu => match key.code {
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                KeyCode::Enter => self.activate_selected(),
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                _ => {}
            },
            Mode::Input(action) => match key.code {
                KeyCode::Enter => {
                    let text = std::mem::take(&mut self.input).trim().to_string();
                    self.mode = Mode::Menu;
                    self.submit(action, text);
                }
                KeyCode::Esc => {
                    self.input.clear();
                    self.mode = Mode::Menu;
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            },
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = MENU.len() as isize;
        let current = self.menu.selected().unwrap_or(0) as isize;
        self.menu
            .select(Some((current + delta).rem_euclid(len) as usize));
    }

    fn activate_selected(&mut self) {
        let action = MENU[self.menu.selected().unwrap_or(0)].1;
        self.status.clear();
        match action {
            Action::Quit => self.quit = true,
            Action::Login => self.start(Job::Login),
            _ if self.busy() && action != Action::Settings => {
                self.status = "A download is already running. Wait for it to finish.".into();
            }
            _ => self.mode = Mode::Input(action),
        }
    }

    fn submit(&mut self, action: Action, text: String) {
        if text.is_empty() && action != Action::Settings {
            self.status = "Nothing entered.".into();
            return;
        }
        let job = match action {
            Action::Song => Job::Song(text),
            Action::Album => Job::Album(text),
            Action::Playlist => Job::Playlist(text),
            Action::Search => Job::Search(text),
            Action::File => Job::File(PathBuf::from(text.trim_matches('"'))),
            Action::Settings => {
                if text.is_empty() {
                    self.status = "Download folder unchanged.".into();
                } else {
                    match self.config.set_download_dir(PathBuf::from(&text)) {
                        Ok(()) => {
                            self.settings.download_dir = self.config.download_dir();
                            let message = format!(
                                "Download folder set to {}",
                                self.settings.download_dir.display()
                            );
                            self.push_log(&message);
                            self.status = "Settings saved.".into();
                        }
                        Err(e) => self.status = format!("Could not save settings: {e:#}"),
                    }
                }
                return;
            }
            Action::Login | Action::Quit => return,
        };
        self.start(job);
    }

    fn start(&mut self, job: Job) {
        if self.busy() {
            self.status = "A download is already running. Wait for it to finish.".into();
            return;
        }
        self.status.clear();
        self.progress.clear();
        self.started = Some(Instant::now());
        let settings = self.settings.clone();
        let sink = self.sink.clone();
        self.job = Some(tokio::spawn(async move {
            match run_job(job, &settings, &sink).await {
                Ok(()) => sink.log("Done."),
                Err(e) => sink.log(format!("error: {e:#}")),
            }
        }));
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let [menu_area, log_area] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).areas(body);

        let header_text = format!(" Downloads: {}", self.settings.download_dir.display());
        frame.render_widget(
            Paragraph::new(header_text).block(Block::bordered().title(" spotify-dl ")),
            header,
        );

        let items: Vec<ListItem> = MENU
            .iter()
            .map(|(label, _)| ListItem::new(*label))
            .collect();
        let menu = List::new(items)
            .block(Block::bordered().title(" Menu "))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        frame.render_stateful_widget(menu, menu_area, &mut self.menu);

        // Wrap long lines (links, file paths) and show the newest rows that fit.
        let width = log_area.width.saturating_sub(2) as usize;
        let height = log_area.height.saturating_sub(2) as usize;
        let mut rows: Vec<Line> = Vec::with_capacity(height);
        'fill: for entry in self.logs.iter().rev() {
            let style = log_style(entry);
            for piece in wrap(entry, width).into_iter().rev() {
                if rows.len() == height {
                    break 'fill;
                }
                rows.push(Line::styled(piece, style));
            }
        }
        rows.reverse();
        frame.render_widget(
            Paragraph::new(rows).block(Block::bordered().title(" Log ")),
            log_area,
        );

        let (footer_text, footer_style) = match self.mode {
            Mode::Input(action) => (format!("{}: {}_", prompt(action), self.input), Style::new()),
            Mode::Menu if self.busy() => {
                let elapsed = self.started.map(|s| s.elapsed()).unwrap_or_default();
                let spin = SPINNER[(elapsed.as_millis() / 150) as usize % SPINNER.len()];
                let doing = if self.progress.is_empty() {
                    "Working..."
                } else {
                    self.progress.as_str()
                };
                (
                    format!("{spin} {doing}  [{}s, q quits]", elapsed.as_secs()),
                    Style::new().fg(Color::Cyan),
                )
            }
            Mode::Menu if !self.status.is_empty() => {
                (self.status.clone(), Style::new().fg(Color::Yellow))
            }
            Mode::Menu => (
                "Up/Down select  Enter run  q quit".to_string(),
                Style::new().add_modifier(Modifier::DIM),
            ),
        };
        frame.render_widget(
            Paragraph::new(Line::styled(footer_text, footer_style)).block(Block::bordered()),
            footer,
        );
    }
}

/// Colours log lines by what they report.
fn log_style(line: &str) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with("error") || trimmed.starts_with("[fail]") {
        Style::new().fg(Color::Red)
    } else if trimmed.starts_with("warning") || trimmed.starts_with("[skip]") {
        Style::new().fg(Color::Yellow)
    } else if trimmed.starts_with("saved:") || trimmed.starts_with("Done.") {
        Style::new().fg(Color::Green)
    } else if trimmed.starts_with("found:") {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    }
}

/// Splits `text` into rows no wider than `width` terminal cells.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut rows = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().expect("rows is never empty").push(c);
        used += w;
    }
    rows
}

fn prompt(action: Action) -> &'static str {
    match action {
        Action::Song => "Track URL, URI or ID",
        Action::Album => "Album URL, URI or ID",
        Action::Playlist => "Playlist URL, URI or ID",
        Action::Search => "Search (Artist - Title)",
        Action::File => "Path to list file",
        Action::Settings => "New download folder (Enter keeps current)",
        Action::Login | Action::Quit => "",
    }
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wraps_by_width() {
        assert_eq!(wrap("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap("", 4), vec![""]);
        assert_eq!(wrap("abc", 0), vec!["abc"]);
    }
}
