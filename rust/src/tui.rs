use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{Color, Frame, Line, Span, Style, Stylize},
    symbols::border,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Terminal,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::cli::DashboardArgs;
use crate::dashboard::humanize_event;
use crate::orchestrator::state::TokenTotals;
use crate::orchestrator::{OrchestratorSnapshot, RetrySnapshot, RunningSnapshot};

const BG: Color = Color::Rgb(26, 26, 46);
const PANEL_BG: Color = Color::Rgb(22, 33, 62);
const PANEL_ALT: Color = Color::Rgb(15, 52, 96);
const ACCENT: Color = Color::Rgb(233, 69, 96);
const TEXT: Color = Color::Rgb(238, 238, 238);
const MUTED: Color = Color::Rgb(168, 175, 196);
const INFO: Color = Color::Rgb(110, 168, 254);

pub async fn run_dashboard(args: DashboardArgs) -> anyhow::Result<()> {
    let refresh_interval = Duration::from_secs(args.refresh.max(1));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut app = DashboardApp::new(args.url);
    let mut terminal = TerminalSession::new()?;
    let (mut events, stop_events, event_task) = spawn_event_reader();
    let mut tick = tokio::time::interval(refresh_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    refresh_snapshot(&client, &mut app, false).await;

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        tokio::select! {
            _ = tick.tick() => {
                refresh_snapshot(&client, &mut app, false).await;
            }
            maybe_event = events.recv() => {
                match maybe_event {
                    Some(event) => {
                        if handle_event(event, &client, &mut app).await? {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    stop_events.store(true, Ordering::Relaxed);
    let _ = event_task.await;
    Ok(())
}

struct DashboardApp {
    base_url: String,
    snapshot: OrchestratorSnapshot,
    last_updated: Option<DateTime<Utc>>,
    error: Option<String>,
    running_scroll: usize,
    retry_scroll: usize,
    focus: TableFocus,
    selected_index: Option<usize>,
    sort_mode: SortMode,
    filter_active: bool,
    filter_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableFocus {
    Running,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Default,
    Identifier,
    State,
    Tokens,
    Turns,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Default => Self::Identifier,
            Self::Identifier => Self::State,
            Self::State => Self::Tokens,
            Self::Tokens => Self::Turns,
            Self::Turns => Self::Default,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Identifier => "id",
            Self::State => "state",
            Self::Tokens => "tokens",
            Self::Turns => "turns",
        }
    }
}

impl DashboardApp {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            snapshot: empty_snapshot(),
            last_updated: None,
            error: None,
            running_scroll: 0,
            retry_scroll: 0,
            focus: TableFocus::Running,
            selected_index: None,
            sort_mode: SortMode::Default,
            filter_active: false,
            filter_text: String::new(),
        }
    }

    fn state_url(&self) -> String {
        format!("{}/api/v1/state", self.base_url.trim_end_matches('/'))
    }

    fn refresh_url(&self) -> String {
        format!("{}/api/v1/refresh", self.base_url.trim_end_matches('/'))
    }

    fn apply_snapshot(&mut self, state: DashboardStateResponse) {
        self.snapshot = state.snapshot;
        self.last_updated = state.generated_at.or_else(|| Some(Utc::now()));
        self.error = None;
    }

    fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    fn scroll_up(&mut self) {
        match self.focus {
            TableFocus::Running => {
                self.running_scroll = self.running_scroll.saturating_sub(1);
            }
            TableFocus::Retry => {
                self.retry_scroll = self.retry_scroll.saturating_sub(1);
            }
        }
    }

    fn scroll_down(&mut self) {
        match self.focus {
            TableFocus::Running => {
                self.running_scroll = self.running_scroll.saturating_add(1);
            }
            TableFocus::Retry => {
                self.retry_scroll = self.retry_scroll.saturating_add(1);
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            TableFocus::Running => TableFocus::Retry,
            TableFocus::Retry => TableFocus::Running,
        };
        self.selected_index = None;
    }

    fn select_by_number(&mut self, n: usize) {
        let count = match self.focus {
            TableFocus::Running => self.visible_running().len(),
            TableFocus::Retry => self.visible_retrying().len(),
        };
        if n > 0 && n <= count {
            self.selected_index = Some(n - 1);
        }
    }

    fn cycle_sort(&mut self) {
        self.sort_mode = self.sort_mode.next();
    }

    fn visible_running(&self) -> Vec<RunningSnapshot> {
        let mut items: Vec<RunningSnapshot> = self
            .snapshot
            .running
            .iter()
            .filter(|r| {
                self.filter_text.is_empty()
                    || r.identifier
                        .to_lowercase()
                        .contains(&self.filter_text.to_lowercase())
            })
            .cloned()
            .collect();

        match self.sort_mode {
            SortMode::Default => {}
            SortMode::Identifier => items.sort_by(|a, b| a.identifier.cmp(&b.identifier)),
            SortMode::State => items.sort_by(|a, b| a.state.cmp(&b.state)),
            SortMode::Tokens => items.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens)),
            SortMode::Turns => items.sort_by(|a, b| b.turn_count.cmp(&a.turn_count)),
        }

        items
    }

    fn visible_retrying(&self) -> Vec<RetrySnapshot> {
        self.snapshot
            .retrying
            .iter()
            .filter(|r| {
                self.filter_text.is_empty()
                    || r.identifier
                        .to_lowercase()
                        .contains(&self.filter_text.to_lowercase())
            })
            .cloned()
            .collect()
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(Self { terminal })
    }

    fn draw<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(render).map(|_| ())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn spawn_event_reader() -> (
    mpsc::UnboundedReceiver<Event>,
    Arc<AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stop);

    let handle = tokio::task::spawn_blocking(move || {
        while !stop_signal.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(200)) {
                Ok(true) => match event::read() {
                    Ok(next) => {
                        if tx.send(next).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => continue,
                Err(_) => break,
            }
        }
    });

    (rx, stop, handle)
}

async fn handle_event(
    event: Event,
    client: &reqwest::Client,
    app: &mut DashboardApp,
) -> anyhow::Result<bool> {
    match event {
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            // In filter mode, capture text input
            if app.filter_active {
                match key.code {
                    KeyCode::Esc => {
                        app.filter_active = false;
                    }
                    KeyCode::Enter => {
                        app.filter_active = false;
                    }
                    KeyCode::Backspace => {
                        app.filter_text.pop();
                    }
                    KeyCode::Char(c) => {
                        app.filter_text.push(c);
                    }
                    _ => {}
                }
                return Ok(false);
            }

            match key.code {
                KeyCode::Char('q') => return Ok(true),
                KeyCode::Char('r') => refresh_snapshot(client, app, true).await,
                KeyCode::Tab => app.toggle_focus(),
                KeyCode::Up => app.scroll_up(),
                KeyCode::Down => app.scroll_down(),
                KeyCode::Char(c @ '1'..='9') => {
                    app.select_by_number(c.to_digit(10).unwrap_or(0) as usize);
                }
                KeyCode::Char('s') => app.cycle_sort(),
                KeyCode::Char('/') => {
                    app.filter_active = true;
                    app.filter_text.clear();
                }
                KeyCode::Esc => {
                    app.selected_index = None;
                    if !app.filter_text.is_empty() {
                        app.filter_text.clear();
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

    Ok(false)
}

async fn refresh_snapshot(client: &reqwest::Client, app: &mut DashboardApp, force: bool) {
    let mut refresh_warning = None;

    if force {
        if let Err(error) = trigger_refresh(client, &app.refresh_url()).await {
            refresh_warning = Some(format!("refresh request failed: {error}"));
        }
    }

    match fetch_snapshot(client, &app.state_url()).await {
        Ok(snapshot) => {
            app.apply_snapshot(snapshot);
            if let Some(warning) = refresh_warning {
                app.set_error(warning);
            }
        }
        Err(error) => match refresh_warning {
            Some(warning) => app.set_error(format!("{warning}; state refresh failed: {error}")),
            None => app.set_error(format!("state refresh failed: {error}")),
        },
    }
}

async fn fetch_snapshot(
    client: &reqwest::Client,
    state_url: &str,
) -> anyhow::Result<DashboardStateResponse> {
    let response = client.get(state_url).send().await?.error_for_status()?;
    let payload: DashboardApiResponse = response.json().await?;
    Ok(payload.into_state())
}

async fn trigger_refresh(client: &reqwest::Client, refresh_url: &str) -> anyhow::Result<()> {
    client.post(refresh_url).send().await?.error_for_status()?;
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &mut DashboardApp) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );

    if area.width < 80 || area.height < 20 {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(BG).fg(TEXT));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("Resize terminal to at least 80x20 to view the dashboard.")
                .style(Style::default().fg(TEXT).bg(BG))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG).fg(TEXT));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Min(6),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(inner);

    render_header(frame, sections[0], app.last_updated);
    render_metrics(frame, sections[1], &app.snapshot);
    render_running_table(frame, sections[2], app);
    render_retry_table(frame, sections[3], app);
    render_footer(frame, sections[4], app);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, last_updated: Option<DateTime<Utc>>) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(28)])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "🦀 Rusty Dashboard",
            Style::default().fg(ACCENT).bold(),
        )]))
        .style(Style::default().bg(BG).fg(TEXT)),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(format!(
            "Last updated: {}",
            format_last_updated(last_updated)
        ))
        .style(Style::default().bg(BG).fg(MUTED))
        .alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_metrics(frame: &mut Frame<'_>, area: Rect, snapshot: &OrchestratorSnapshot) {
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    render_metric_card(
        frame,
        cards[0],
        "Running",
        format_with_commas(snapshot.running_count as u64),
        Color::Green,
    );
    render_metric_card(
        frame,
        cards[1],
        "Retrying",
        format_with_commas(snapshot.retrying_count as u64),
        Color::Yellow,
    );
    render_metric_card(
        frame,
        cards[2],
        "Tokens",
        format_with_commas(snapshot.agent_totals.total_tokens),
        ACCENT,
    );
    render_metric_card(
        frame,
        cards[3],
        "Runtime",
        format_runtime(snapshot.agent_totals.seconds_running),
        INFO,
    );
}

fn render_metric_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    value: String,
    value_color: Color,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(PANEL_ALT))
        .style(Style::default().bg(PANEL_BG).fg(TEXT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(title, Style::default().fg(MUTED))),
            Line::from(Span::styled(value, Style::default().fg(value_color).bold())),
        ])
        .style(Style::default().bg(PANEL_BG).fg(TEXT))
        .alignment(Alignment::Center),
        inner,
    );
}

fn render_running_table(frame: &mut Frame<'_>, area: Rect, app: &mut DashboardApp) {
    let sort_label = if app.sort_mode != SortMode::Default {
        format!(" Running Sessions [sort: {}] ", app.sort_mode.label())
    } else {
        " Running Sessions ".to_string()
    };
    let block = section_block(&sort_label, matches!(app.focus, TableFocus::Running));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = app.visible_running();
    if visible.is_empty() {
        let msg = if app.filter_text.is_empty() {
            "No running sessions."
        } else {
            "No matching sessions."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .style(Style::default().bg(PANEL_BG).fg(MUTED))
                .alignment(Alignment::Left),
            inner,
        );
        return;
    }

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let max_scroll = visible.len().saturating_sub(visible_rows);
    app.running_scroll = app.running_scroll.min(max_scroll);

    let selected = if matches!(app.focus, TableFocus::Running) {
        app.selected_index
    } else {
        None
    };

    let rows = visible
        .iter()
        .skip(app.running_scroll)
        .take(visible_rows)
        .enumerate()
        .map(|(index, session)| running_row(index, session, selected));

    let header = Row::new(vec!["Issue", "State", "Turns", "Update", "Tokens"])
        .style(Style::default().fg(ACCENT).bg(PANEL_BG).bold());
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .column_spacing(1)
    .style(Style::default().bg(PANEL_BG).fg(TEXT));

    frame.render_widget(table, inner);
}

fn render_retry_table(frame: &mut Frame<'_>, area: Rect, app: &mut DashboardApp) {
    let block = section_block(" Retry Queue ", matches!(app.focus, TableFocus::Retry));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = app.visible_retrying();
    if visible.is_empty() {
        let msg = if app.filter_text.is_empty() {
            "No retrying sessions."
        } else {
            "No matching sessions."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .style(Style::default().bg(PANEL_BG).fg(MUTED))
                .alignment(Alignment::Left),
            inner,
        );
        return;
    }

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let max_scroll = visible.len().saturating_sub(visible_rows);
    app.retry_scroll = app.retry_scroll.min(max_scroll);

    let rows = visible
        .iter()
        .skip(app.retry_scroll)
        .take(visible_rows)
        .enumerate()
        .map(|(index, session)| retry_row(index, session));

    let header = Row::new(vec!["Issue", "Attempt", "Due at", "Error"])
        .style(Style::default().fg(ACCENT).bg(PANEL_BG).bold());
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .column_spacing(1)
    .style(Style::default().bg(PANEL_BG).fg(TEXT));

    frame.render_widget(table, inner);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &DashboardApp) {
    if app.filter_active {
        let spans = vec![
            Span::styled("Filter: ", Style::default().fg(ACCENT).bold()),
            Span::styled(&app.filter_text, Style::default().fg(TEXT)),
            Span::styled("█", Style::default().fg(ACCENT)),
            Span::styled("  (Enter to apply, Esc to cancel)", Style::default().fg(MUTED)),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::default().bg(BG).fg(MUTED))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let mut spans = vec![
        Span::styled("q", Style::default().fg(ACCENT).bold()),
        Span::styled(": quit  ", Style::default().fg(MUTED)),
        Span::styled("r", Style::default().fg(ACCENT).bold()),
        Span::styled(": refresh  ", Style::default().fg(MUTED)),
        Span::styled("tab", Style::default().fg(ACCENT).bold()),
        Span::styled(": switch  ", Style::default().fg(MUTED)),
        Span::styled("↑↓", Style::default().fg(ACCENT).bold()),
        Span::styled(": scroll  ", Style::default().fg(MUTED)),
        Span::styled("1-9", Style::default().fg(ACCENT).bold()),
        Span::styled(": select  ", Style::default().fg(MUTED)),
        Span::styled("s", Style::default().fg(ACCENT).bold()),
        Span::styled(": sort  ", Style::default().fg(MUTED)),
        Span::styled("/", Style::default().fg(ACCENT).bold()),
        Span::styled(": filter", Style::default().fg(MUTED)),
    ];

    if let Some(error) = &app.error {
        spans.push(Span::styled("  •  ", Style::default().fg(MUTED)));
        spans.push(Span::styled(
            truncate_with_ellipsis(error, 60),
            Style::default().fg(Color::Red),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(BG).fg(MUTED))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn section_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    let border_color = if focused { ACCENT } else { PANEL_ALT };
    let title_style = if focused {
        Style::default().fg(ACCENT).bold()
    } else {
        Style::default().fg(TEXT).bold()
    };

    Block::default()
        .title(title)
        .title_style(title_style)
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(PANEL_BG).fg(TEXT))
}

fn running_row(index: usize, running: &RunningSnapshot, selected: Option<usize>) -> Row<'static> {
    let is_selected = selected == Some(index);
    let row_bg = if is_selected {
        PANEL_ALT
    } else if index.is_multiple_of(2) {
        PANEL_BG
    } else {
        BG
    };
    let update = humanize_update(
        running.last_event.as_deref(),
        running.last_message.as_deref(),
    );

    let row = Row::new(vec![
        Cell::from(truncate_with_ellipsis(&running.identifier, 12)),
        Cell::from(running.state.clone()).style(Style::default().fg(state_color(&running.state))),
        Cell::from(running.turn_count.to_string()),
        Cell::from(truncate_with_ellipsis(&update, 18)),
        Cell::from(format_with_commas(running.total_tokens)),
    ])
    .style(Style::default().bg(row_bg).fg(TEXT));

    if is_selected {
        row.style(Style::default().bg(PANEL_ALT).fg(TEXT).bold())
    } else {
        row
    }
}

fn retry_row(index: usize, retry: &RetrySnapshot) -> Row<'static> {
    let row_bg = if index.is_multiple_of(2) { PANEL_BG } else { BG };

    Row::new(vec![
        Cell::from(truncate_with_ellipsis(&retry.identifier, 12)),
        Cell::from(retry.attempt.to_string()),
        Cell::from(format_due_at(&retry.due_at)),
        Cell::from(truncate_with_ellipsis(
            retry.error.as_deref().unwrap_or("-"),
            28,
        )),
    ])
    .style(Style::default().bg(row_bg).fg(TEXT))
}

fn humanize_update(last_event: Option<&str>, last_message: Option<&str>) -> String {
    if let Some(event) = last_event.filter(|event| !event.trim().is_empty()) {
        let display = humanize_event(event);
        if display == event {
            return last_message
                .filter(|message| !message.trim().is_empty())
                .unwrap_or(display)
                .to_string();
        }
        return display.to_string();
    }

    last_message
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("-")
        .to_string()
}

fn format_due_at(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| truncate_with_ellipsis(value, 10))
}

fn format_last_updated(value: Option<DateTime<Utc>>) -> String {
    value
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn format_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut reversed = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().rev().enumerate() {
        if index != 0 && index % 3 == 0 {
            reversed.push(',');
        }
        reversed.push(ch);
    }

    reversed.chars().rev().collect()
}

fn format_runtime(seconds_running: f64) -> String {
    let total_seconds = seconds_running.max(0.0).floor() as u64;

    if total_seconds < 60 {
        format!("{total_seconds}s")
    } else if total_seconds < 3_600 {
        format!("{}m {}s", total_seconds / 60, total_seconds % 60)
    } else {
        format!(
            "{}h {}m",
            total_seconds / 3_600,
            (total_seconds % 3_600) / 60
        )
    }
}

fn state_color(state: &str) -> Color {
    if state.eq_ignore_ascii_case("inprogress") {
        Color::Green
    } else if state.eq_ignore_ascii_case("todo") {
        Color::Yellow
    } else if state.eq_ignore_ascii_case("failed") {
        Color::Red
    } else {
        TEXT
    }
}

fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }

    if max_chars <= 1 {
        return "…".to_string();
    }

    let mut truncated: String = value.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

fn empty_snapshot() -> OrchestratorSnapshot {
    OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        running: Vec::new(),
        retrying: Vec::new(),
        agent_totals: TokenTotals::default(),
    }
}

#[derive(Debug, Deserialize)]
struct DashboardApiResponse {
    generated_at: String,
    counts: DashboardCounts,
    running: Vec<ApiRunningSnapshot>,
    retrying: Vec<ApiRetrySnapshot>,
    codex_totals: ApiTokenTotals,
}

impl DashboardApiResponse {
    fn into_state(self) -> DashboardStateResponse {
        DashboardStateResponse {
            generated_at: DateTime::parse_from_rfc3339(&self.generated_at)
                .ok()
                .map(|timestamp| timestamp.with_timezone(&Utc)),
            snapshot: OrchestratorSnapshot {
                running_count: self.counts.running,
                retrying_count: self.counts.retrying,
                running: self.running.into_iter().map(Into::into).collect(),
                retrying: self.retrying.into_iter().map(Into::into).collect(),
                agent_totals: self.codex_totals.into(),
            },
        }
    }
}

struct DashboardStateResponse {
    generated_at: Option<DateTime<Utc>>,
    snapshot: OrchestratorSnapshot,
}

#[derive(Debug, Deserialize)]
struct DashboardCounts {
    running: usize,
    retrying: usize,
}

#[derive(Debug, Deserialize)]
struct ApiRunningSnapshot {
    issue_id: String,
    identifier: String,
    state: String,
    session_id: Option<String>,
    turn_count: u32,
    last_event: Option<String>,
    last_message: Option<String>,
    started_at: String,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    issue_url: Option<String>,
}

impl From<ApiRunningSnapshot> for RunningSnapshot {
    fn from(value: ApiRunningSnapshot) -> Self {
        Self {
            issue_id: value.issue_id,
            identifier: value.identifier,
            state: value.state,
            session_id: value.session_id,
            turn_count: value.turn_count,
            last_event: value.last_event,
            last_message: value.last_message,
            started_at: value.started_at,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
            issue_url: value.issue_url,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiRetrySnapshot {
    issue_id: String,
    identifier: String,
    attempt: u32,
    due_at: String,
    error: Option<String>,
    issue_url: Option<String>,
}

impl From<ApiRetrySnapshot> for RetrySnapshot {
    fn from(value: ApiRetrySnapshot) -> Self {
        Self {
            issue_id: value.issue_id,
            identifier: value.identifier,
            attempt: value.attempt,
            due_at: value.due_at,
            error: value.error,
            issue_url: value.issue_url,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiTokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    seconds_running: f64,
}

impl From<ApiTokenTotals> for TokenTotals {
    fn from(value: ApiTokenTotals) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
            seconds_running: value.seconds_running,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_runtime, format_with_commas, state_color, DashboardApiResponse};
    use ratatui::style::Color;

    #[test]
    fn format_with_commas_inserts_grouping() {
        assert_eq!(format_with_commas(0), "0");
        assert_eq!(format_with_commas(1_200), "1,200");
        assert_eq!(format_with_commas(12_345_678), "12,345,678");
    }

    #[test]
    fn format_runtime_uses_expected_units() {
        assert_eq!(format_runtime(42.0), "42s");
        assert_eq!(format_runtime(923.5), "15m 23s");
        assert_eq!(format_runtime(7_380.0), "2h 3m");
    }

    #[test]
    fn state_color_maps_known_states() {
        assert_eq!(state_color("InProgress"), Color::Green);
        assert_eq!(state_color("Todo"), Color::Yellow);
        assert_eq!(state_color("Failed"), Color::Red);
    }

    #[test]
    fn dashboard_api_response_deserializes_into_snapshot() {
        let payload = r#"{
            "generated_at": "2026-03-14T10:01:46Z",
            "counts": { "running": 2, "retrying": 0 },
            "running": [
                {
                    "issue_id": "28",
                    "identifier": "rusty-28",
                    "state": "InProgress",
                    "session_id": "abc-123",
                    "turn_count": 3,
                    "last_event": "notification",
                    "last_message": "session update",
                    "started_at": "2026-03-14T10:01:24Z",
                    "input_tokens": 500,
                    "output_tokens": 200,
                    "total_tokens": 700,
                    "issue_url": "https://example.test/issues/rusty-28"
                }
            ],
            "retrying": [],
            "codex_totals": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "total_tokens": 1500,
                "seconds_running": 923.5
            },
            "rate_limits": null
        }"#;

        let response: DashboardApiResponse = serde_json::from_str(payload).expect("valid payload");
        let state = response.into_state();

        assert_eq!(state.snapshot.running_count, 2);
        assert_eq!(state.snapshot.retrying_count, 0);
        assert_eq!(state.snapshot.running[0].identifier, "rusty-28");
        assert_eq!(state.snapshot.agent_totals.total_tokens, 1_500);
        assert!(state.generated_at.is_some());
    }

    #[test]
    fn dashboard_api_response_preserves_issue_url() {
        let payload = r#"{
            "generated_at": "2026-03-14T10:01:46Z",
            "counts": { "running": 1, "retrying": 0 },
            "running": [
                {
                    "issue_id": "42",
                    "identifier": "rusty-42",
                    "state": "InProgress",
                    "session_id": null,
                    "turn_count": 1,
                    "last_event": null,
                    "last_message": null,
                    "started_at": "2026-03-14T10:01:24Z",
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "total_tokens": 0,
                    "issue_url": "https://github.com/example/repo/issues/42"
                }
            ],
            "retrying": [],
            "codex_totals": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0, "seconds_running": 0.0 },
            "rate_limits": null
        }"#;

        let response: DashboardApiResponse = serde_json::from_str(payload).expect("valid payload");
        let state = response.into_state();
        assert_eq!(
            state.snapshot.running[0].issue_url.as_deref(),
            Some("https://github.com/example/repo/issues/42")
        );
    }

    #[test]
    fn sort_mode_cycles_through_all_variants() {
        use super::SortMode;

        let mut mode = SortMode::Default;
        mode = mode.next();
        assert_eq!(mode, SortMode::Identifier);
        mode = mode.next();
        assert_eq!(mode, SortMode::State);
        mode = mode.next();
        assert_eq!(mode, SortMode::Tokens);
        mode = mode.next();
        assert_eq!(mode, SortMode::Turns);
        mode = mode.next();
        assert_eq!(mode, SortMode::Default);
    }

    #[test]
    fn sort_mode_labels_are_non_empty() {
        use super::SortMode;
        for mode in [
            SortMode::Default,
            SortMode::Identifier,
            SortMode::State,
            SortMode::Tokens,
            SortMode::Turns,
        ] {
            assert!(!mode.label().is_empty());
        }
    }
}
