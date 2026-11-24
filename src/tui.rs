//! Full-screen terminal interface, shown when spotify-dl is started without a subcommand.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthChar;

use crate::art::Cover;
use crate::config::{Config, MAX_PARALLEL, MIN_PARALLEL, Source, describe};
use crate::format;
use crate::jobs::{self, Export, IsrcRequest, Job, Settings, Sink, TrackStatus, run_job};
use crate::paths::{Completion, expand_home};
use crate::youtube::SourceFormat;

const LOG_LIMIT: usize = 1000;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
/// Rows of the "Now downloading" panel, border included, when there is room for the cover art.
const PANEL_ROWS_WITH_ART: u16 = 14;
/// Rows of the panel when the screen is too short for art.
const PANEL_ROWS_SMALL: u16 = 8;
/// The log column must be at least this tall for the big panel, and this tall for the small one.
const BIG_PANEL_MIN_COLUMN: u16 = 22;
const SMALL_PANEL_MIN_COLUMN: u16 = 14;
/// Smallest width of the text next to the cover art.
const PANEL_TEXT_MIN_WIDTH: u16 = 34;
const DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Song,
    Album,
    Playlist,
    Search,
    File,
    Isrc,
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
    ("Find ISRC code", Action::Isrc),
    ("Log in to Spotify", Action::Login),
    ("Settings", Action::Settings),
    ("Quit", Action::Quit),
];

/// Something you can change on the settings screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Folder,
    ClientId,
    ClientSecret,
    Parallel,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Folder => "Download folder",
            Field::ClientId => "Spotify client ID",
            Field::ClientSecret => "Spotify client secret",
            Field::Parallel => "Parallel downloads",
        }
    }
}

const SETTINGS: &[(&str, Option<Field>)] = &[
    ("Download folder", Some(Field::Folder)),
    ("Spotify client ID", Some(Field::ClientId)),
    ("Spotify client secret", Some(Field::ClientSecret)),
    ("Parallel downloads", Some(Field::Parallel)),
    ("Back", None),
];

enum Mode {
    Menu,
    /// Typing the link, search or path for a download.
    Input(Action),
    Settings,
    /// Typing a new value for one setting.
    Edit(Field),
}

struct App {
    settings: Settings,
    config: Config,
    menu: ListState,
    settings_menu: ListState,
    mode: Mode,
    input: String,
    /// Folder suggestions while editing the download folder.
    completion: Option<Completion>,
    logs: Vec<String>,
    status: String,
    /// What the running job is doing right now.
    progress: String,
    started: Option<Instant>,
    /// Tracks being downloaded right now, in the order they started.
    active: Vec<TrackStatus>,
    /// Finished, failed and total tracks of the running job.
    overall: (usize, usize, usize),
    /// Cover art of the running job.
    cover: Option<Arc<Cover>>,
    sink: Sink,
    rx: mpsc::UnboundedReceiver<jobs::Event>,
    job: Option<JoinHandle<()>>,
    quit: bool,
}

/// Runs the full-screen UI until the user quits.
pub async fn run(settings: Settings, config: Config) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut logs = vec!["Welcome. Pick an action with the arrow keys and press Enter.".to_string()];
    if settings.id_source == Source::Missing || settings.secret_source == Source::Missing {
        logs.push("Spotify client ID and secret are not set. Open Settings to enter them.".into());
    }
    let ask = settings.client_id.trim().is_empty() || settings.client_secret.trim().is_empty();
    let mut app = App {
        settings,
        config,
        menu: ListState::default().with_selected(Some(0)),
        settings_menu: ListState::default().with_selected(Some(0)),
        mode: Mode::Menu,
        input: String::new(),
        completion: None,
        logs,
        status: String::new(),
        progress: String::new(),
        started: None,
        active: Vec::new(),
        overall: (0, 0, 0),
        cover: None,
        sink: Sink::Channel(tx),
        rx,
        job: None,
        quit: false,
    };
    if ask {
        app.ask_for_credentials();
    }

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
                    jobs::Event::Track(status) => {
                        match self.active.iter_mut().find(|t| t.id == status.id) {
                            Some(existing) => *existing = status,
                            None => self.active.push(status),
                        }
                    }
                    jobs::Event::TrackDone(id) => self.active.retain(|t| t.id != id),
                    jobs::Event::Overall {
                        done,
                        failed,
                        total,
                    } => self.overall = (done, failed, total),
                    jobs::Event::Cover(cover) => self.cover = Some(cover),
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

    // ---------- Keys ----------

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
                KeyCode::Up => move_selection(&mut self.menu, MENU.len(), -1),
                KeyCode::Down => move_selection(&mut self.menu, MENU.len(), 1),
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
            Mode::Settings => match key.code {
                KeyCode::Up => move_selection(&mut self.settings_menu, SETTINGS.len(), -1),
                KeyCode::Down => move_selection(&mut self.settings_menu, SETTINGS.len(), 1),
                KeyCode::Enter => match SETTINGS[self.settings_menu.selected().unwrap_or(0)].1 {
                    Some(field) => self.begin_edit(field),
                    None => self.leave_settings(),
                },
                KeyCode::Esc | KeyCode::Char('q') => self.leave_settings(),
                _ => {}
            },
            Mode::Edit(field) => self.on_edit_key(field, key.code),
        }
    }

    fn on_edit_key(&mut self, field: Field, code: KeyCode) {
        match code {
            KeyCode::Enter => self.save_field(field),
            KeyCode::Esc => {
                self.status.clear();
                self.end_edit();
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_completion(field);
            }
            // The parallel count is a number; ignore anything else.
            KeyCode::Char(c) if field == Field::Parallel && !c.is_ascii_digit() => {}
            KeyCode::Char(c) => {
                self.input.push(c);
                self.refresh_completion(field);
            }
            KeyCode::Tab if field == Field::Folder => {
                if let Some(c) = self.completion.as_mut()
                    && let Some((text, keep)) = c.next()
                {
                    self.input = text;
                    if !keep {
                        self.completion = Some(Completion::new(&self.input));
                    }
                }
            }
            KeyCode::Down | KeyCode::Up | KeyCode::BackTab if field == Field::Folder => {
                let forward = code == KeyCode::Down;
                if let Some(c) = self.completion.as_mut()
                    && let Some(text) = c.step(forward)
                {
                    self.input = text;
                }
            }
            _ => {}
        }
    }

    fn refresh_completion(&mut self, field: Field) {
        if field == Field::Folder {
            self.completion = Some(Completion::new(&self.input));
        }
    }

    // ---------- Menu actions ----------

    fn activate_selected(&mut self) {
        let action = MENU[self.menu.selected().unwrap_or(0)].1;
        self.status.clear();
        match action {
            Action::Quit => self.quit = true,
            Action::Login => self.start(Job::Login),
            Action::Settings => {
                self.settings_menu.select(Some(0));
                self.mode = Mode::Settings;
            }
            _ if self.busy() => {
                self.status = "A download is already running. Wait for it to finish.".into();
            }
            _ => self.mode = Mode::Input(action),
        }
    }

    fn submit(&mut self, action: Action, text: String) {
        if text.is_empty() {
            self.status = "Nothing entered.".into();
            return;
        }
        let job = match action {
            Action::Song => Job::Song(text),
            Action::Album => Job::Album(text),
            Action::Playlist => Job::Playlist(text),
            Action::Search => Job::Search(text),
            Action::File => Job::File(PathBuf::from(text.trim_matches('"'))),
            // Playlists are long, so they are also saved as CSV and JSON files.
            Action::Isrc => Job::Isrc(IsrcRequest {
                input: text,
                csv: Export::PlaylistOnly,
                json: Export::PlaylistOnly,
            }),
            Action::Login | Action::Settings | Action::Quit => return,
        };
        self.start(job);
    }

    fn start(&mut self, job: Job) {
        if self.busy() {
            self.status = "A download is already running. Wait for it to finish.".into();
            return;
        }
        if self.needs_credentials() {
            self.ask_for_credentials();
            return;
        }
        self.status.clear();
        self.progress.clear();
        self.active.clear();
        self.overall = (0, 0, 0);
        self.cover = None;
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

    // ---------- Settings ----------

    fn needs_credentials(&self) -> bool {
        self.settings.client_id.trim().is_empty() || self.settings.client_secret.trim().is_empty()
    }

    /// Opens the entry box for whichever Spotify credential is still missing.
    fn ask_for_credentials(&mut self) {
        let field = if self.settings.client_id.trim().is_empty() {
            Field::ClientId
        } else {
            Field::ClientSecret
        };
        self.begin_edit(field);
        self.status = "Enter your Spotify client ID and secret first (free at developer.spotify.com/dashboard).".into();
    }

    fn leave_settings(&mut self) {
        self.status.clear();
        self.mode = Mode::Menu;
    }

    fn begin_edit(&mut self, field: Field) {
        self.status.clear();
        self.input = match field {
            Field::Folder => self.settings.download_dir.display().to_string(),
            Field::ClientId => self.config.saved_client_id().to_string(),
            // Never show the secret, saved or not.
            Field::ClientSecret => String::new(),
            Field::Parallel => self.settings.parallel.to_string(),
        };
        self.completion = (field == Field::Folder).then(|| Completion::new(&self.input));
        self.mode = Mode::Edit(field);
    }

    fn end_edit(&mut self) {
        self.input.clear();
        self.completion = None;
        self.mode = Mode::Settings;
    }

    fn save_field(&mut self, field: Field) {
        let text = std::mem::take(&mut self.input);
        let text = text.trim().trim_matches('"').to_string();
        self.end_edit();
        if text.is_empty() {
            self.status = format!("{} unchanged.", field.label());
            return;
        }
        let saved = match field {
            Field::Folder => self.config.set_download_dir(expand_home(&text)),
            Field::ClientId => self.config.set_client_id(&text),
            Field::ClientSecret => self.config.set_client_secret(&text),
            Field::Parallel => match text.parse::<usize>() {
                Ok(n) if (MIN_PARALLEL..=MAX_PARALLEL).contains(&n) => self.config.set_parallel(n),
                _ => {
                    self.status = format!("Enter a number from {MIN_PARALLEL} to {MAX_PARALLEL}.");
                    return;
                }
            },
        };
        if let Err(e) = saved {
            self.status = format!("Could not save: {e:#}");
            return;
        }

        // Later jobs read the new values from `settings`.
        (self.settings.client_id, self.settings.id_source) = self.config.client_id();
        (self.settings.client_secret, self.settings.secret_source) = self.config.client_secret();
        self.settings.download_dir = self.config.download_dir();
        self.settings.parallel = self.config.parallel();

        if matches!(field, Field::ClientId | Field::ClientSecret) && self.needs_credentials() {
            self.ask_for_credentials();
            return;
        }

        let overridden = match field {
            Field::Folder | Field::Parallel => false,
            Field::ClientId => self.settings.id_source == Source::Env,
            Field::ClientSecret => self.settings.secret_source == Source::Env,
        };
        self.status = if overridden {
            format!(
                "{} saved, but a .env file or environment variable still overrides it.",
                field.label()
            )
        } else {
            format!("{} saved.", field.label())
        };
        if field == Field::Folder {
            let message = format!(
                "Download folder set to {}",
                self.settings.download_dir.display()
            );
            self.push_log(&message);
        }
    }

    // ---------- Drawing ----------

    fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let [left, right] =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).areas(body);

        let header_text = format!(" Downloads: {}", self.settings.download_dir.display());
        frame.render_widget(
            Paragraph::new(header_text).block(Block::bordered().title(" spotify-dl ")),
            header,
        );

        let in_settings = matches!(self.mode, Mode::Settings | Mode::Edit(_));
        let (labels, title): (Vec<&str>, &str) = if in_settings {
            (SETTINGS.iter().map(|(l, _)| *l).collect(), " Settings ")
        } else {
            (MENU.iter().map(|(l, _)| *l).collect(), " Menu ")
        };
        let list = List::new(labels.into_iter().map(ListItem::new))
            .block(Block::bordered().title(title))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        let state = if in_settings {
            &mut self.settings_menu
        } else {
            &mut self.menu
        };
        frame.render_stateful_widget(list, left, state);

        match self.mode {
            Mode::Settings => self.draw_values(frame, right),
            Mode::Edit(Field::Folder) => self.draw_folders(frame, right),
            Mode::Edit(Field::Parallel) => self.draw_parallel_help(frame, right),
            Mode::Edit(field) => self.draw_help(frame, right, field),
            Mode::Menu | Mode::Input(_) => {
                let panel_rows = self.panel_rows(right.height);
                if panel_rows == 0 {
                    self.draw_log(frame, right);
                } else {
                    let [top, bottom] =
                        Layout::vertical([Constraint::Length(panel_rows), Constraint::Min(3)])
                            .areas(right);
                    self.draw_panel(frame, top);
                    self.draw_log(frame, bottom);
                }
            }
        }

        let (footer_text, footer_style) = self.footer();
        frame.render_widget(
            Paragraph::new(Line::styled(footer_text, footer_style)).block(Block::bordered()),
            footer,
        );
    }

    fn footer(&self) -> (String, Style) {
        let hint = |text: &str| (text.to_string(), Style::new().add_modifier(Modifier::DIM));
        match self.mode {
            Mode::Input(action) => (format!("{}: {}_", prompt(action), self.input), Style::new()),
            Mode::Edit(field) => {
                let shown = if field == Field::ClientSecret {
                    "*".repeat(self.input.chars().count())
                } else {
                    self.input.clone()
                };
                (format!("{}: {shown}_", field.label()), Style::new())
            }
            Mode::Settings if !self.status.is_empty() => {
                (self.status.clone(), Style::new().fg(Color::Yellow))
            }
            Mode::Settings => hint("Up/Down select  Enter edit  Esc back"),
            Mode::Menu if self.busy() => {
                let elapsed = self.started.map(|s| s.elapsed()).unwrap_or_default();
                let spin = SPINNER[(elapsed.as_millis() / 150) as usize % SPINNER.len()];
                let (done, failed, total) = self.overall;
                let counts = format!("{} of {total} tracks done", done + failed);
                let doing = if !self.progress.is_empty() {
                    self.progress.as_str()
                } else if total > 0 {
                    counts.as_str()
                } else {
                    "Working..."
                };
                (
                    format!("{spin} {doing}  [{}s, q quits]", elapsed.as_secs()),
                    Style::new().fg(Color::Cyan),
                )
            }
            Mode::Menu if !self.status.is_empty() => {
                (self.status.clone(), Style::new().fg(Color::Yellow))
            }
            Mode::Menu => hint("Up/Down select  Enter run  q quit"),
        }
    }

    /// How tall the "Now downloading" panel is. Zero hides it: no job is running, there is
    /// nothing to show yet, or the terminal is too short.
    fn panel_rows(&self, column_height: u16) -> u16 {
        let has_content = !self.active.is_empty() || self.overall.2 > 0 || self.cover.is_some();
        if !self.busy() || !has_content {
            0
        } else if column_height >= BIG_PANEL_MIN_COLUMN {
            PANEL_ROWS_WITH_ART
        } else if column_height >= SMALL_PANEL_MIN_COLUMN {
            PANEL_ROWS_SMALL
        } else {
            0
        }
    }

    /// Cover art on the left, overall progress and one entry per active track on the right.
    fn draw_panel(&self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered().title(" Now downloading ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Art is about twice as wide as it is tall. The small panel has no art.
        let art_rows = inner.height.min(12);
        let art_cols = art_rows * 2;
        let show_art = area.height >= PANEL_ROWS_WITH_ART
            && self.cover.is_some()
            && inner.width >= art_cols + 1 + PANEL_TEXT_MIN_WIDTH;
        let text_area = if show_art {
            let [art, text] =
                Layout::horizontal([Constraint::Length(art_cols + 1), Constraint::Min(1)])
                    .areas(inner);
            if let Some(cover) = &self.cover {
                draw_art(frame, art, cover, art_cols, art_rows);
            }
            text
        } else {
            inner
        };
        frame.render_widget(Paragraph::new(self.panel_lines(text_area)), text_area);
    }

    fn panel_lines(&self, area: Rect) -> Vec<Line<'static>> {
        let width = area.width as usize;
        let dim = Style::new().add_modifier(Modifier::DIM);
        let (done, failed, total) = self.overall;

        // Overall bar from finished tracks plus the fraction of each active one.
        let mut fraction_of_active = 0.0;
        let mut speed = 0.0;
        for t in &self.active {
            if let Some(tr) = &t.transfer {
                speed += tr.speed.unwrap_or(0.0);
                if let Some(total) = tr.total {
                    fraction_of_active += tr.downloaded as f64 / total.max(1) as f64;
                }
            }
        }
        let overall = if total == 0 {
            0.0
        } else {
            ((done + failed) as f64 + fraction_of_active) / total as f64
        };
        let tail = format!(
            " {:>3.0}%  {}/{} tracks  {}",
            overall * 100.0,
            done + failed,
            total,
            format::rate(speed)
        );
        let bar_width = width.saturating_sub(tail.chars().count() + 2).clamp(4, 30);
        let mut lines = vec![Line::from(fit(
            &format!("{}{tail}", format::bar(overall, bar_width)),
            width,
        ))];

        // Two lines per track. If they do not all fit, the last line says how many are hidden.
        let spare = (area.height as usize).saturating_sub(1);
        let overflow = self.active.len() > spare / 2;
        let room = if overflow {
            spare.saturating_sub(1) / 2
        } else {
            self.active.len()
        };
        for t in self.active.iter().take(room) {
            lines.push(Line::from(fit(
                &format!("{}  [{}]", t.label, t.stage.label()),
                width,
            )));
            lines.push(Line::styled(fit(&track_detail(t, width), width), dim));
        }
        if overflow {
            lines.push(Line::styled(
                fit(
                    &format!("+{} more downloading", self.active.len() - room),
                    width,
                ),
                dim,
            ));
        }
        if self.active.is_empty() && !self.progress.is_empty() {
            lines.push(Line::styled(fit(&self.progress, width), dim));
        }
        lines
    }

    fn draw_log(&self, frame: &mut Frame, area: Rect) {
        // Wrap long lines (links, file paths) and show the newest rows that fit.
        let width = area.width.saturating_sub(2) as usize;
        let height = area.height.saturating_sub(2) as usize;
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
            area,
        );
    }

    /// The current value of every setting.
    fn draw_values(&self, frame: &mut Frame, area: Rect) {
        let s = &self.settings;
        let lines = vec![
            Line::from(format!(" Download folder: {}", s.download_dir.display())),
            Line::from(format!(
                " Client ID:       {}",
                describe(&s.client_id, s.id_source, false)
            )),
            Line::from(format!(
                " Client secret:   {}",
                describe(&s.client_secret, s.secret_source, true)
            )),
            Line::from(format!(" Parallel:        {} tracks at a time", s.parallel)),
            Line::from(""),
            Line::styled(
                format!(" Saved in {}", self.config.config_file().display()),
                Style::new().add_modifier(Modifier::DIM),
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(" Current values ")),
            area,
        );
    }

    /// Folder suggestions for what has been typed so far.
    fn draw_folders(&self, frame: &mut Frame, area: Rect) {
        let Some(completion) = &self.completion else {
            return;
        };
        let block =
            Block::bordered().title(" Folders: Tab completes, Up/Down choose, Enter saves ");
        if !completion.readable() {
            frame.render_widget(
                Paragraph::new(" Cannot open that folder. Enter saves the path and creates it.")
                    .block(block),
                area,
            );
        } else if completion.matches().is_empty() {
            frame.render_widget(
                Paragraph::new(" No folders match. Enter saves the path and creates it.")
                    .block(block),
                area,
            );
        } else {
            let list = List::new(
                completion
                    .matches()
                    .iter()
                    .map(|m| ListItem::new(m.as_str())),
            )
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(completion.selected());
            frame.render_stateful_widget(list, area, &mut state);
        }
    }

    fn draw_parallel_help(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(format!(
                " How many tracks to download at the same time ({MIN_PARALLEL} to {MAX_PARALLEL})."
            )),
            Line::from(" Albums, playlists and file lists use it. Three is a good start."),
            Line::from(" More is faster, but YouTube may slow down or refuse many requests."),
            Line::from(""),
            Line::styled(
                " Enter saves. Esc cancels.",
                Style::new().add_modifier(Modifier::DIM),
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(" Parallel downloads ")),
            area,
        );
    }

    /// Where to find a credential, and where it ends up.
    fn draw_help(&self, frame: &mut Frame, area: Rect, field: Field) {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let mut lines = vec![
            Line::from(" Find it on your app's page in the Spotify developer dashboard:"),
            Line::from(format!(" {DASHBOARD_URL}")),
        ];
        if field == Field::ClientSecret {
            lines.push(Line::from(
                " On that page, open Settings and choose View client secret.",
            ));
            lines.push(Line::from(""));
            lines.push(Line::styled(
                " The secret is saved as plain text in your user config folder.",
                dim,
            ));
        }
        let (current, source) = match field {
            Field::ClientId => (self.settings.client_id.as_str(), self.settings.id_source),
            _ => (
                self.settings.client_secret.as_str(),
                self.settings.secret_source,
            ),
        };
        if source == Source::Env && !current.is_empty() {
            lines.push(Line::styled(
                " A .env file or environment variable is set and overrides the saved value.",
                Style::new().fg(Color::Yellow),
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(" Enter saves. Esc cancels.", dim));
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(format!(" {} ", field.label()))),
            area,
        );
    }
}

fn move_selection(state: &mut ListState, len: usize, delta: isize) {
    let current = state.selected().unwrap_or(0) as isize;
    state.select(Some((current + delta).rem_euclid(len as isize) as usize));
}

/// Colours log lines by what they report. Track lines start with `[i/N]`, so the markers can
/// be anywhere in the line.
fn log_style(line: &str) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with("error") || trimmed.contains("[fail]") {
        Style::new().fg(Color::Red)
    } else if trimmed.contains("warning:")
        || trimmed.contains("[skip]")
        || trimmed.contains("skipped,")
    {
        Style::new().fg(Color::Yellow)
    } else if trimmed.contains("saved:")
        || trimmed.starts_with("ISRC:")
        || trimmed.starts_with("Done.")
        || trimmed.starts_with("Finished")
    {
        Style::new().fg(Color::Green)
    } else if trimmed.contains("] match:")
        || trimmed.contains("] source:")
        || trimmed.contains("] mp3:")
    {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    }
}

/// Draws the cover as coloured ASCII art in the top-left of `area`.
fn draw_art(frame: &mut Frame, area: Rect, cover: &Cover, cols: u16, rows: u16) {
    let lines: Vec<Line> = cover
        .ascii(cols, rows)
        .into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|cell| {
                        let [r, g, b] = cell.rgb;
                        Span::styled(cell.ch.to_string(), Style::new().fg(Color::Rgb(r, g, b)))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The second line of a track entry: bar, percent, speed, bytes, ETA and stream details.
fn track_detail(t: &TrackStatus, width: usize) -> String {
    let Some(tr) = &t.transfer else {
        return String::new();
    };
    let fraction = tr
        .total
        .map(|total| tr.downloaded as f64 / total.max(1) as f64);
    let mut text = format::bar(fraction.unwrap_or(0.0), 12);
    if let Some(f) = fraction {
        text.push_str(&format!(" {:>3.0}%", f * 100.0));
    }
    if let Some(speed) = tr.speed {
        text.push_str(&format!("  {}", format::rate(speed)));
    }
    text.push_str(&format!("  {}", format::bytes(tr.downloaded)));
    if let Some(total) = tr.total {
        text.push_str(&format!(" / {}", format::bytes(total)));
    }
    if let Some(eta) = tr.eta_secs {
        text.push_str(&format!("  ETA {}", format::duration(eta)));
    }
    if let Some(f) = &t.format {
        let details = format_summary(f);
        // Stream details come last, so they are the first thing cut on a narrow panel.
        if !details.is_empty() && text.chars().count() + details.chars().count() + 2 <= width {
            text.push_str(&format!("  {details}"));
        }
    }
    text
}

/// `opus 133 kbps 48 kHz stereo`.
fn format_summary(f: &SourceFormat) -> String {
    let mut parts = Vec::new();
    if !f.codec.is_empty() {
        parts.push(f.codec.clone());
    }
    if let Some(kbps) = f.bitrate_kbps {
        parts.push(format!("{kbps:.0} kbps"));
    }
    if let Some(hz) = f.sample_rate {
        parts.push(format::khz(hz));
    }
    if let Some(n) = f.channels {
        parts.push(format::channels(n));
    }
    parts.join(" ")
}

/// Cuts `text` to `width` terminal cells.
fn fit(text: &str, width: usize) -> String {
    wrap(text, width).swap_remove(0)
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
        Action::Isrc => "Song or playlist link, or a song name",
        Action::Login | Action::Settings | Action::Quit => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{fit, wrap};

    #[test]
    fn wraps_by_width() {
        assert_eq!(wrap("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap("", 4), vec![""]);
        assert_eq!(wrap("abc", 0), vec!["abc"]);
    }

    #[test]
    fn fit_cuts_to_width() {
        assert_eq!(fit("abcdef", 4), "abcd");
        assert_eq!(fit("ab", 4), "ab");
        assert_eq!(fit("", 4), "");
    }
}
