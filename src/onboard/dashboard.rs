//! `agentzero dashboard` — multi-tab TUI dashboard (Costs, Memory, Cron, Metrics, and live Logs).

use crate::config::Config;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::{collections::HashSet, io, sync::mpsc, time::Duration};

const MAX_LOG_ENTRIES: usize = 500;
const TICK_MS: u64 = 100;

// ── Tab ───────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    MissionControl,
    Memory,
    Cron,
    Costs,
    Metrics,
}

// ── Log types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct LogEntry {
    event_type: String,
    timestamp: String,
    detail: String,
}

// ── Cron types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CronJob {
    id: String,
    name: Option<String>,
    command: String,
    expression: String,
    job_type: String,
    next_run: String,
    last_run: Option<String>,
    last_status: Option<String>,
    last_output: Option<String>,
    prompt: Option<String>,
    enabled: bool,
    delivery: CronJobDelivery,
}

#[derive(Debug, Clone)]
struct CronJobDelivery {
    mode: String,
    channel: Option<String>,
    to: Option<String>,
}

#[derive(PartialEq, Eq)]
enum CronMode {
    List,
    Detail,
    ConfirmDelete,
}

struct CronState {
    jobs: Vec<CronJob>,
    cursor: usize,
    scroll_offset: usize,
    mode: CronMode,
    loading: bool,
    status: Option<String>,
}

impl CronState {
    fn new() -> Self {
        Self {
            jobs: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            mode: CronMode::List,
            loading: true,
            status: None,
        }
    }

    fn selected(&self) -> Option<&CronJob> {
        self.jobs.get(self.cursor)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    fn move_down(&mut self, visible: usize) {
        if self.cursor + 1 < self.jobs.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible {
                self.scroll_offset = self.cursor + 1 - visible;
            }
        }
    }
}

enum CronAction {
    Fetch,
    Delete { id: String },
}

// ── Cost types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ModelRow {
    model: String,
    cost_usd: f64,
    total_tokens: u64,
    request_count: usize,
    channel: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct CostSummary {
    hourly_cost_usd: f64,
    daily_cost_usd: f64,
    monthly_cost_usd: f64,
    total_tokens: u64,
    request_count: usize,
    by_model: Vec<ModelRow>,
}

struct CostsState {
    summary: CostSummary,
    version: Option<String>,
    uptime_seconds: Option<u64>,
    cursor: usize,
    scroll_offset: usize,
    loading: bool,
    status: Option<String>,
}

impl CostsState {
    fn new() -> Self {
        Self {
            summary: CostSummary::default(),
            version: None,
            uptime_seconds: None,
            cursor: 0,
            scroll_offset: 0,
            loading: true,
            status: None,
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    fn move_down(&mut self, visible: usize) {
        if self.cursor + 1 < self.summary.by_model.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible {
                self.scroll_offset = self.cursor + 1 - visible;
            }
        }
    }
}

enum CostsAction {
    Fetch,
}

// ── Metrics types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MetricSample {
    labels: Vec<(String, String)>,
    value: String,
}

#[derive(Debug, Clone)]
struct MetricFamily {
    name: String,
    help: String,
    kind: String,
    samples: Vec<MetricSample>,
}

struct MetricsState {
    families: Vec<MetricFamily>,
    scroll: usize,
    loading: bool,
    status: Option<String>,
}

impl MetricsState {
    fn new() -> Self {
        Self {
            families: Vec::new(),
            scroll: 0,
            loading: true,
            status: None,
        }
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self, max: usize) {
        if self.scroll + 1 < max {
            self.scroll += 1;
        }
    }
}

enum MetricsAction {
    Fetch,
}

// ── Memory types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MemoryEntry {
    id: String,
    key: String,
    content: String,
    category: String,
    timestamp: String,
}

// ── Messages (async → TUI) ────────────────────────────────────────────────────

enum Msg {
    // event stream
    EventStreamEvent(LogEntry),
    EventStreamConnected,
    EventStreamDisconnected(String),
    // memory
    MemoryLoaded(Vec<MemoryEntry>),
    MemoryError(String),
    MemoryDeleted(String),
    // cron
    CronLoaded(Vec<CronJob>),
    CronError(String),
    CronDeleted(String),
    // costs
    CostsLoaded(CostsSnapshot),
    CostsError(String),
    // metrics
    MetricsLoaded(Vec<MetricFamily>),
    MetricsError(String),
}

#[derive(Debug, Clone)]
struct CostsSnapshot {
    summary: CostSummary,
    version: Option<String>,
    uptime_seconds: Option<u64>,
}

// ── Actions (TUI → async) ─────────────────────────────────────────────────────

enum Action {
    FetchMemory { query: Option<String> },
    DeleteMemory { key: String },
}

// ── Dashboard state ───────────────────────────────────────────────────────────

struct EventsState {
    entries: Vec<LogEntry>,
    filtered: Vec<usize>,
    paused: bool,
    connected: bool,
    status_msg: String,
    auto_scroll: bool,
    scroll: usize,
    type_filters: HashSet<String>,
    all_types: Vec<String>,
    filter_mode: bool,
    filter_cursor: usize,
    detail_open: bool,
}

impl EventsState {
    fn new(url: String) -> Self {
        Self {
            entries: Vec::new(),
            filtered: Vec::new(),
            paused: false,
            connected: false,
            status_msg: format!("Connecting to {url}…"),
            auto_scroll: true,
            scroll: 0,
            type_filters: HashSet::new(),
            all_types: Vec::new(),
            filter_mode: false,
            filter_cursor: 0,
            detail_open: false,
        }
    }

    fn push(&mut self, entry: LogEntry) {
        if self.paused {
            return;
        }
        if !self.all_types.contains(&entry.event_type) {
            self.all_types.push(entry.event_type.clone());
            self.all_types.sort();
        }
        self.entries.push(entry);
        if self.entries.len() > MAX_LOG_ENTRIES {
            self.entries.drain(..self.entries.len() - MAX_LOG_ENTRIES);
            self.rebuild_filtered();
        } else {
            let idx = self.entries.len() - 1;
            if self.passes_filter(idx) {
                self.filtered.push(idx);
            }
        }
        if self.auto_scroll && !self.filtered.is_empty() {
            self.scroll = self.filtered.len() - 1;
        }
    }

    fn passes_filter(&self, idx: usize) -> bool {
        self.type_filters.is_empty() || self.type_filters.contains(&self.entries[idx].event_type)
    }

    fn rebuild_filtered(&mut self) {
        self.filtered = (0..self.entries.len())
            .filter(|&i| self.passes_filter(i))
            .collect();
    }

    fn toggle_filter(&mut self, type_name: &str) {
        if self.type_filters.contains(type_name) {
            self.type_filters.remove(type_name);
        } else {
            self.type_filters.insert(type_name.to_string());
        }
        self.rebuild_filtered();
        if self.auto_scroll && !self.filtered.is_empty() {
            self.scroll = self.filtered.len() - 1;
        }
    }

    fn scroll_up(&mut self) {
        self.auto_scroll = false;
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        let max = self.filtered.len().saturating_sub(1);
        self.scroll = (self.scroll + 1).min(max);
        if self.scroll >= max {
            self.auto_scroll = true;
        }
    }

    fn jump_to_bottom(&mut self) {
        self.scroll = self.filtered.len().saturating_sub(1);
        self.auto_scroll = true;
    }
}

#[derive(PartialEq, Eq)]
enum MemoryMode {
    List,
    Search, // user is typing in search bar
    Detail, // full-content overlay
    ConfirmDelete,
}

struct MemoryState {
    entries: Vec<MemoryEntry>,
    cursor: usize,
    scroll_offset: usize,
    mode: MemoryMode,
    search_input: String,
    loading: bool,
    status: Option<String>, // error / info banner
}

impl MemoryState {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            mode: MemoryMode::List,
            search_input: String::new(),
            loading: true,
            status: None,
        }
    }

    fn selected(&self) -> Option<&MemoryEntry> {
        self.entries.get(self.cursor)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    fn move_down(&mut self, visible: usize) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible {
                self.scroll_offset = self.cursor + 1 - visible;
            }
        }
    }
}

struct DashboardApp {
    tab: Tab,
    events: EventsState,
    memory: MemoryState,
    cron: CronState,
    costs: CostsState,
    metrics: MetricsState,
}

impl DashboardApp {
    fn new(sse_url: String) -> Self {
        Self {
            tab: Tab::Costs,
            events: EventsState::new(sse_url),
            memory: MemoryState::new(),
            cron: CronState::new(),
            costs: CostsState::new(),
            metrics: MetricsState::new(),
        }
    }
}

// ── Colors ────────────────────────────────────────────────────────────────────

fn event_type_color(t: &str) -> Color {
    match t.to_lowercase().as_str() {
        "error" => Color::Red,
        "warn" | "warning" => Color::Yellow,
        "tool_call" | "tool_result" | "tool_call_start" | "tool_call_result" => Color::Magenta,
        "message" | "chat" => Color::Blue,
        "health" | "status" | "connected" | "heartbeat_tick" => Color::Green,
        "llm_response" => Color::Cyan,
        "llm_request" => Color::LightBlue,
        "agent_start" | "agent_end" => Color::LightYellow,
        "turn_complete" => Color::LightGreen,
        "channel_message" => Color::LightMagenta,
        "webhook_auth_failure" => Color::LightRed,
        _ => Color::DarkGray,
    }
}

fn category_color(cat: &str) -> Color {
    match cat.to_lowercase().as_str() {
        "core" => Color::Blue,
        "daily" => Color::Yellow,
        "conversation" => Color::Cyan,
        _ => Color::DarkGray,
    }
}

fn format_iso_timestamp_local(ts: &str) -> String {
    // Try parsing as RFC3339 first
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
    }
    // Try parsing as UTC (no timezone)
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ") {
        let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc);
        return utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
    }
    // Fallback: just replace T
    ts.replace('T', " ")
}

// ── Draw ──────────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &DashboardApp) {
    let area = f.area();
    // Top: tab bar (1) + content (rest)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    draw_tab_bar(f, outer[0], app);

    match app.tab {
        Tab::MissionControl => draw_events_tab(f, outer[1], &app.events),
        Tab::Memory => draw_memory(f, outer[1], &app.memory),
        Tab::Cron => draw_cron(f, outer[1], &app.cron),
        Tab::Costs => draw_costs(f, outer[1], &app.costs),
        Tab::Metrics => draw_metrics(f, outer[1], &app.metrics),
    }
}

fn draw_tab_bar(f: &mut Frame, area: Rect, app: &DashboardApp) {
    let tab_style = |selected: bool| {
        if selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" [1] Costs ", tab_style(app.tab == Tab::Costs)),
            Span::raw(" "),
            Span::styled(" [2] Memory ", tab_style(app.tab == Tab::Memory)),
            Span::raw(" "),
            Span::styled(" [3] Cron ", tab_style(app.tab == Tab::Cron)),
            Span::raw(" "),
            Span::styled(
                " [4] Mission Control ",
                tab_style(app.tab == Tab::MissionControl),
            ),
            Span::raw(" "),
            Span::styled(" [5] Metrics ", tab_style(app.tab == Tab::Metrics)),
            Span::styled("   [q]uit", Style::default().fg(Color::DarkGray)),
        ])),
        area,
    );
}

// ── Logs tab ──────────────────────────────────────────────────────────────────

fn draw_events_tab(f: &mut Frame, area: Rect, s: &EventsState) {
    let filter_height: u16 = if s.filter_mode { 3 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(filter_height),
        ])
        .split(area);

    // status bar
    let conn = if s.connected {
        Span::styled("● Connected", Style::default().fg(Color::Green))
    } else {
        Span::styled(
            format!("○ {}", s.status_msg),
            Style::default().fg(Color::Red),
        )
    };
    let paused = if s.paused {
        Span::styled(
            " ⏸ PAUSED",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("")
    };
    let counts = if s.type_filters.is_empty() {
        format!("  {} events", s.filtered.len())
    } else {
        format!("  {}/{}", s.filtered.len(), s.entries.len())
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            conn,
            paused,
            Span::raw(counts),
            Span::styled(
                "  [p]ause  [f]ilter  [G]bottom",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        chunks[0],
    );

    // log list
    let visible = chunks[1].height.saturating_sub(2) as usize;
    let start = if s.filtered.len() <= visible || s.scroll < visible {
        0
    } else {
        (s.scroll + 1).saturating_sub(visible)
    };
    let end = (start + visible).min(s.filtered.len());

    let items: Vec<ListItem> = s.filtered[start..end]
        .iter()
        .enumerate()
        .map(|(i, &idx)| {
            let e = &s.entries[idx];
            let color = event_type_color(&e.event_type);
            let selected = start + i == s.scroll;
            if selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", e.timestamp),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::REVERSED),
                    ),
                    Span::styled(
                        format!("{:<14}", &e.event_type[..e.event_type.len().min(14)]),
                        Style::default()
                            .fg(color)
                            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                    ),
                    Span::styled(
                        format!(" {}", e.detail),
                        Style::default().add_modifier(Modifier::REVERSED),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", e.timestamp),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("{:<14}", &e.event_type[..e.event_type.len().min(14)]),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" {}", e.detail)),
                ]))
            }
        })
        .collect();

    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Events  [Enter] detail "),
        ),
        chunks[1],
    );

    // detail overlay
    if s.detail_open {
        if let Some(&entry_idx) = s.filtered.get(s.scroll) {
            draw_log_detail_overlay(f, area, &s.entries[entry_idx]);
        }
    }

    // filter bar
    if s.filter_mode && chunks[2].height > 0 {
        let fc = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(2)])
            .split(chunks[2]);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Filter: ", Style::default().fg(Color::DarkGray)),
                Span::styled("Space", Style::default().fg(Color::White)),
                Span::styled(" toggle  ", Style::default().fg(Color::DarkGray)),
                Span::styled("←→", Style::default().fg(Color::White)),
                Span::styled(" move  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::White)),
                Span::styled(" close", Style::default().fg(Color::DarkGray)),
            ])),
            fc[0],
        );

        let type_spans: Vec<Span> = s
            .all_types
            .iter()
            .enumerate()
            .flat_map(|(i, t)| {
                let active = s.type_filters.contains(t);
                let selected = i == s.filter_cursor;
                let cb = if active { "[x] " } else { "[ ] " };
                let style = if selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else if active {
                    Style::default().fg(event_type_color(t))
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                [Span::styled(format!("{cb}{t}"), style), Span::raw("  ")]
            })
            .collect();

        f.render_widget(Paragraph::new(Line::from(type_spans)), fc[1]);
    }
}

fn draw_log_detail_overlay(f: &mut Frame, area: Rect, entry: &LogEntry) {
    let pw = (area.width * 4 / 5)
        .max(40)
        .min(area.width.saturating_sub(4));
    let ph = (area.height * 7 / 10)
        .max(10)
        .min(area.height.saturating_sub(4));
    let px = area.x + (area.width.saturating_sub(pw)) / 2;
    let py = area.y + (area.height.saturating_sub(ph)) / 2;
    let popup = Rect::new(px, py, pw, ph);

    f.render_widget(Clear, popup);

    let color = event_type_color(&entry.event_type);
    let title = format!(" {} — {} ", entry.event_type, entry.timestamp);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(color));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    // Pretty-print JSON if possible, otherwise raw
    let body = serde_json::from_str::<serde_json::Value>(&entry.detail)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| entry.detail.clone());

    f.render_widget(
        Paragraph::new(body.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            "[Esc/q] close",
            Style::default().fg(Color::DarkGray),
        )),
        chunks[1],
    );
}

// ── Memory tab ────────────────────────────────────────────────────────────────

fn draw_memory(f: &mut Frame, area: Rect, s: &MemoryState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    // hints bar
    let hint = if s.mode == MemoryMode::Search {
        Line::from(vec![
            Span::styled(" Search: ", Style::default().fg(Color::Yellow)),
            Span::styled(&s.search_input, Style::default().fg(Color::White)),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::styled(
                "  Enter=go  Esc=cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                format!(" {} entries", s.entries.len()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "  [/]search  [r]efresh  [Enter]detail  [d]elete",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    f.render_widget(Paragraph::new(hint), chunks[0]);

    // status / error banner
    if let Some(ref msg) = s.status {
        let color = if msg.starts_with("Error") || msg.starts_with("Delete") {
            Color::Red
        } else {
            Color::Green
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {msg}"), Style::default().fg(color))),
            chunks[1],
        );
    } else if s.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                " Loading…",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[1],
        );
    }

    // list
    let list_area = chunks[2];
    let visible = list_area.height.saturating_sub(2) as usize;
    let start = s.scroll_offset;
    let end = (start + visible).min(s.entries.len());

    let items: Vec<ListItem> = s.entries[start..end]
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let abs = start + i;
            let selected = abs == s.cursor;
            let base = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let cat_color = category_color(&e.category);
            let preview: String = e
                .content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect();
            let ts_formatted = format_iso_timestamp_local(&e.timestamp);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<28}", &e.key[..e.key.len().min(28)]),
                    base.fg(Color::White),
                ),
                Span::styled(
                    format!(" {:<12}", &e.category[..e.category.len().min(12)]),
                    base.fg(cat_color),
                ),
                Span::styled(format!(" {:<60}", preview), base.fg(Color::DarkGray)),
                Span::styled(format!(" {}", ts_formatted), base.fg(Color::DarkGray)),
            ]))
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Memory ")),
        list_area,
    );

    // detail overlay
    if s.mode == MemoryMode::Detail || s.mode == MemoryMode::ConfirmDelete {
        if let Some(entry) = s.selected() {
            draw_memory_overlay(f, area, entry, s.mode == MemoryMode::ConfirmDelete);
        }
    }
}

fn draw_memory_overlay(f: &mut Frame, area: Rect, entry: &MemoryEntry, confirm_delete: bool) {
    // Centered popup — 80% width, 70% height
    let pw = (area.width * 4 / 5)
        .max(40)
        .min(area.width.saturating_sub(4));
    let ph = (area.height * 7 / 10)
        .max(10)
        .min(area.height.saturating_sub(4));
    let px = area.x + (area.width.saturating_sub(pw)) / 2;
    let py = area.y + (area.height.saturating_sub(ph)) / 2;
    let popup = Rect::new(px, py, pw, ph);

    f.render_widget(Clear, popup);

    let title = format!(" {} [{}] ", entry.key, entry.category);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Blue));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(entry.content.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            format!("Created: {}", format_iso_timestamp_local(&entry.timestamp)),
            Style::default().fg(Color::DarkGray),
        )),
        chunks[1],
    );

    if confirm_delete {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Delete this entry? ", Style::default().fg(Color::Red)),
                Span::styled(
                    "[y]",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled("es  ", Style::default().fg(Color::Red)),
                Span::styled(
                    "[n]",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("o / Esc", Style::default().fg(Color::White)),
            ])),
            chunks[2],
        );
    } else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "[d]elete  [Esc/q] close",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[2],
        );
    }
}

// ── Cron tab ──────────────────────────────────────────────────────────────────

fn draw_cron(f: &mut Frame, area: Rect, s: &CronState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    // hints bar
    let hint = Line::from(vec![
        Span::styled(
            format!(" {} jobs", s.jobs.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "  [r]efresh  [Enter]detail  [d]elete",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(hint), chunks[0]);

    // status / loading banner
    if let Some(ref msg) = s.status {
        let color = if msg.starts_with("Error") {
            Color::Red
        } else {
            Color::Green
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {msg}"), Style::default().fg(color))),
            chunks[1],
        );
    } else if s.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                " Loading…",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[1],
        );
    }

    // list
    let list_area = chunks[2];
    let visible = list_area.height.saturating_sub(2) as usize;
    let start = s.scroll_offset;
    let end = (start + visible).min(s.jobs.len());

    let items: Vec<ListItem> = s.jobs[start..end]
        .iter()
        .enumerate()
        .map(|(i, job)| {
            let abs = start + i;
            let selected = abs == s.cursor;
            let base = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let enabled_indicator = if job.enabled { "●" } else { "○" };
            let enabled_color = if job.enabled {
                Color::Green
            } else {
                Color::DarkGray
            };
            let label = job.name.as_deref().unwrap_or(&job.command);
            let label_preview: String = label.chars().take(30).collect();
            let status_str = job.last_status.as_deref().unwrap_or("-");
            let status_color = match status_str {
                "ok" | "success" => Color::Green,
                "error" | "failed" => Color::Red,
                _ => Color::DarkGray,
            };
            let next_formatted = format_iso_timestamp_local(&job.next_run);
            let next_short: String = next_formatted.chars().take(16).collect();
            ListItem::new(Line::from(vec![
                Span::styled(format!("{enabled_indicator} "), base.fg(enabled_color)),
                Span::styled(format!("{:<30}", label_preview), base.fg(Color::White)),
                Span::styled(
                    format!(" {:<20}", &job.expression[..job.expression.len().min(20)]),
                    base.fg(Color::Cyan),
                ),
                Span::styled(format!(" next:{next_short}"), base.fg(Color::DarkGray)),
                Span::styled(format!(" [{status_str}]"), base.fg(status_color)),
            ]))
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Cron Jobs ")),
        list_area,
    );

    // overlay
    if s.mode == CronMode::Detail || s.mode == CronMode::ConfirmDelete {
        if let Some(job) = s.selected() {
            draw_cron_overlay(f, area, job, s.mode == CronMode::ConfirmDelete);
        }
    }
}

fn draw_cron_overlay(f: &mut Frame, area: Rect, job: &CronJob, confirm_delete: bool) {
    let pw = (area.width * 4 / 5)
        .max(40)
        .min(area.width.saturating_sub(4));
    let ph = (area.height * 7 / 10)
        .max(14)
        .min(area.height.saturating_sub(4));
    let px = area.x + (area.width.saturating_sub(pw)) / 2;
    let py = area.y + (area.height.saturating_sub(ph)) / 2;
    let popup = Rect::new(px, py, pw, ph);

    f.render_widget(Clear, popup);

    let label = job.name.as_deref().unwrap_or("(unnamed)");
    let title = format!(" {label} ");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    // build detail text
    let enabled_str = if job.enabled { "enabled" } else { "disabled" };
    let next_run_formatted = format_iso_timestamp_local(&job.next_run);
    let last_run_formatted = job
        .last_run
        .as_ref()
        .map(|lr| format_iso_timestamp_local(lr));
    let last_status_str = job.last_status.as_deref().unwrap_or("-");
    let last_output_str = job.last_output.as_deref().unwrap_or("");

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("ID:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(&job.id, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Schedule: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&job.expression, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Type:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(&job.job_type, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Status:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                enabled_str,
                Style::default().fg(if job.enabled {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("Command:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(&job.command, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Next run: ", Style::default().fg(Color::DarkGray)),
            Span::styled(next_run_formatted, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Last run: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                last_run_formatted.as_deref().unwrap_or("never"),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Last status: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                last_status_str,
                Style::default().fg(match last_status_str {
                    "ok" | "success" => Color::Green,
                    "error" | "failed" => Color::Red,
                    _ => Color::DarkGray,
                }),
            ),
        ]),
    ];
    // Delivery configuration
    if job.delivery.mode != "none" && !job.delivery.mode.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Delivery: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&job.delivery.mode, Style::default().fg(Color::Yellow)),
        ]));
        if let Some(ref channel) = job.delivery.channel {
            lines.push(Line::from(vec![
                Span::styled("  Channel: ", Style::default().fg(Color::DarkGray)),
                Span::styled(channel, Style::default().fg(Color::Cyan)),
            ]));
        }
        if let Some(ref to) = job.delivery.to {
            lines.push(Line::from(vec![
                Span::styled("  Target:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(to, Style::default().fg(Color::White)),
            ]));
        }
    }

    if let Some(ref prompt) = job.prompt {
        if !prompt.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "Prompt:",
                Style::default().fg(Color::DarkGray),
            )]));
            for l in prompt.lines().take(6) {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    }
    if !last_output_str.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Last output:",
            Style::default().fg(Color::DarkGray),
        )]));
        for l in last_output_str.lines().take(5) {
            lines.push(Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[0]);

    if confirm_delete {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Delete this job? ", Style::default().fg(Color::Red)),
                Span::styled(
                    "[y]",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled("es  ", Style::default().fg(Color::Red)),
                Span::styled(
                    "[n]",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("o / Esc", Style::default().fg(Color::White)),
            ])),
            chunks[1],
        );
    } else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "[d]elete  [Esc/q] close",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[1],
        );
    }
}

// ── Costs tab ─────────────────────────────────────────────────────────────────

fn draw_costs(f: &mut Frame, area: Rect, s: &CostsState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(0),
        ])
        .split(area);

    // hints bar
    let hint = Line::from(vec![
        Span::styled(
            format!(" {} models", s.summary.by_model.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("  [r]efresh", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(hint), chunks[0]);

    // status / loading banner
    if let Some(ref msg) = s.status {
        let color = if msg.starts_with("Error") {
            Color::Red
        } else {
            Color::Green
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {msg}"), Style::default().fg(color))),
            chunks[1],
        );
    } else if s.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                " Loading…",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[1],
        );
    }

    // status panel: version + uptime
    let version_str = s.version.as_deref().unwrap_or("—");
    let uptime_str = s
        .uptime_seconds
        .map(format_uptime)
        .unwrap_or_else(|| "—".to_string());
    let status_line = Line::from(vec![
        Span::styled("Version: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            version_str,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("    Uptime:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            uptime_str,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(status_line).block(Block::default().borders(Borders::ALL).title(" Status ")),
        chunks[2],
    );

    // summary panel
    let summary_lines = vec![
        Line::from(vec![
            Span::styled("Hourly:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:>8.4}", s.summary.hourly_cost_usd),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("    Daily:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:>8.4}", s.summary.daily_cost_usd),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("    Monthly: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:>8.2}", s.summary.monthly_cost_usd),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Tokens:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", s.summary.total_tokens),
                Style::default().fg(Color::White),
            ),
            Span::styled("    Requests: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", s.summary.request_count),
                Style::default().fg(Color::White),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(summary_lines)
            .block(Block::default().borders(Borders::ALL).title(" Summary ")),
        chunks[3],
    );

    // per-model list
    let list_area = chunks[4];
    let visible = list_area.height.saturating_sub(2) as usize;
    let start = s.scroll_offset;
    let end = (start + visible).min(s.summary.by_model.len());

    let header = ListItem::new(Line::from(vec![Span::styled(
        format!(
            " {:<32} {:<10} {:>12} {:>10} {:>10}",
            "model", "channel", "cost", "tokens", "requests"
        ),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )]));

    let mut items: Vec<ListItem> = vec![header];
    items.extend(
        s.summary.by_model[start..end]
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let abs = start + i;
                let selected = abs == s.cursor;
                let base = if selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                let model_preview: String = row.model.chars().take(32).collect();
                let channel_str = row.channel.as_deref().unwrap_or("-");
                let channel_preview: String = channel_str.chars().take(10).collect();
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {:<32}", model_preview), base.fg(Color::White)),
                    Span::styled(format!(" {:<10}", channel_preview), base.fg(Color::Cyan)),
                    Span::styled(format!(" ${:>11.4}", row.cost_usd), base.fg(Color::Yellow)),
                    Span::styled(format!(" {:>10}", row.total_tokens), base.fg(Color::White)),
                    Span::styled(format!(" {:>10}", row.request_count), base.fg(Color::White)),
                ]))
            }),
    );

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" By Model ")),
        list_area,
    );
}

// ── Metrics tab ───────────────────────────────────────────────────────────────

fn draw_metrics(f: &mut Frame, area: Rect, s: &MetricsState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    // hints bar
    let hint = Line::from(vec![
        Span::styled(
            format!(" {} metrics", s.families.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "  [r]efresh  [j/k] scroll",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(hint), chunks[0]);

    // status / loading banner
    if let Some(ref msg) = s.status {
        let color = if msg.starts_with("Error") {
            Color::Red
        } else {
            Color::Green
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {msg}"), Style::default().fg(color))),
            chunks[1],
        );
    } else if s.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                " Loading…",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[1],
        );
    }

    // build all lines
    let mut lines: Vec<Line> = Vec::new();
    for fam in &s.families {
        // family header: name (type) — help
        lines.push(Line::from(vec![
            Span::styled(
                fam.name.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({})", fam.kind),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        if !fam.help.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", fam.help),
                Style::default().fg(Color::DarkGray),
            )));
        }
        for sample in &fam.samples {
            let labels = if sample.labels.is_empty() {
                String::new()
            } else {
                let parts: Vec<String> = sample
                    .labels
                    .iter()
                    .map(|(k, v)| format!("{k}=\"{v}\""))
                    .collect();
                format!("{{{}}}", parts.join(","))
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {labels:<48} "),
                    Style::default().fg(Color::White),
                ),
                Span::styled(sample.value.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let list_area = chunks[2];
    let visible = list_area.height.saturating_sub(2) as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(visible);
    let scroll = s.scroll.min(max_scroll);
    let end = (scroll + visible).min(total);

    let visible_lines: Vec<Line> = lines[scroll..end].to_vec();

    f.render_widget(
        Paragraph::new(visible_lines)
            .block(Block::default().borders(Borders::ALL).title(" Metrics "))
            .wrap(Wrap { trim: false }),
        list_area,
    );
}

// ── Event stream parsing ──────────────────────────────────────────────────────

fn parse_event_stream_message(raw: &str) -> Option<LogEntry> {
    let mut event_type = "message".to_string();
    let mut data_lines: Vec<&str> = Vec::new();

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim());
        }
    }
    if data_lines.is_empty() {
        return None;
    }

    let data_str = data_lines.join("\n");
    let (final_type, timestamp, detail) =
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
            let t = v
                .get("type")
                .and_then(|x| x.as_str())
                .unwrap_or(&event_type)
                .to_string();
            let ts = v
                .get("timestamp")
                .and_then(|x| x.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
            let detail = v
                .get("message")
                .or_else(|| v.get("content"))
                .or_else(|| v.get("data"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    let m: serde_json::Map<String, serde_json::Value> = v
                        .as_object()
                        .map(|o| {
                            o.iter()
                                .filter(|(k, _)| k.as_str() != "type" && k.as_str() != "timestamp")
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    serde_json::to_string(&m).unwrap_or_default()
                });
            (t, ts, detail)
        } else {
            (
                event_type,
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                data_str,
            )
        };

    Some(LogEntry {
        event_type: final_type,
        timestamp,
        detail,
    })
}

// ── Async tasks ───────────────────────────────────────────────────────────────

async fn run_event_stream_task(url: String, token: Option<String>, tx: mpsc::SyncSender<Msg>) {
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    loop {
        let mut req = client.get(&url).header("Accept", "text/event-stream");
        if let Some(ref t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }

        let response = match req.send().await {
            Ok(r) if r.status().is_success() => {
                let _ = tx.send(Msg::EventStreamConnected);
                r
            }
            Ok(r) => {
                let msg = if r.status() == reqwest::StatusCode::UNAUTHORIZED {
                    "HTTP 401 — token rejected. Delete cli-token in config dir and re-run to re-pair.".to_string()
                } else {
                    format!("HTTP {} — is the gateway running?", r.status())
                };
                let _ = tx.send(Msg::EventStreamDisconnected(msg));
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            Err(e) if e.is_connect() => {
                let _ = tx.send(Msg::EventStreamDisconnected(
                    "Connection refused — start the gateway first".to_string(),
                ));
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            Err(e) => {
                let _ = tx.send(Msg::EventStreamDisconnected(format!("{e}")));
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(pos) = buffer.find("\n\n") {
                        let raw = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();
                        if let Some(entry) = parse_event_stream_message(&raw) {
                            if tx.send(Msg::EventStreamEvent(entry)).is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }

        let _ = tx.send(Msg::EventStreamDisconnected(
            "Stream ended — reconnecting…".to_string(),
        ));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_memory_task(
    base_url: String,
    token: Option<String>,
    tx: mpsc::SyncSender<Msg>,
    actions: mpsc::Receiver<Action>,
) {
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let auth_header = token.map(|t| format!("Bearer {t}"));

    // Initial load
    fetch_memory(&client, &base_url, auth_header.as_ref(), None, &tx).await;

    // Action loop — run on a blocking thread since mpsc::Receiver is sync
    loop {
        let action = match actions.recv() {
            Ok(a) => a,
            Err(_) => break,
        };
        match action {
            Action::FetchMemory { query } => {
                fetch_memory(
                    &client,
                    &base_url,
                    auth_header.as_ref(),
                    query.as_deref(),
                    &tx,
                )
                .await;
            }
            Action::DeleteMemory { key } => {
                let url = format!("{base_url}/api/memory/{}", urlencoding(&key));
                let mut req = client.delete(&url);
                if let Some(h) = &auth_header {
                    req = req.header("Authorization", h);
                }
                match req.send().await {
                    Ok(r) if r.status().is_success() => {
                        let _ = tx.send(Msg::MemoryDeleted(key));
                    }
                    Ok(r) => {
                        let _ = tx.send(Msg::MemoryError(format!(
                            "Delete failed: HTTP {}",
                            r.status()
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::MemoryError(format!("Delete failed: {e}")));
                    }
                }
            }
        }
    }
}

async fn fetch_memory(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: Option<&String>,
    query: Option<&str>,
    tx: &mpsc::SyncSender<Msg>,
) {
    let url = if let Some(q) = query {
        format!("{base_url}/api/memory?query={}", urlencoding(q))
    } else {
        format!("{base_url}/api/memory")
    };

    let mut req = client.get(&url);
    if let Some(h) = auth_header {
        req = req.header("Authorization", h);
    }

    match req.send().await {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) => {
                let entries = parse_memory_entries(&body);
                let _ = tx.send(Msg::MemoryLoaded(entries));
            }
            Err(e) => {
                let _ = tx.send(Msg::MemoryError(format!("Parse error: {e}")));
            }
        },
        Ok(r) => {
            let _ = tx.send(Msg::MemoryError(format!("HTTP {}", r.status())));
        }
        Err(e) => {
            let _ = tx.send(Msg::MemoryError(format!("Error: {e}")));
        }
    }
}

fn parse_memory_entries(body: &serde_json::Value) -> Vec<MemoryEntry> {
    body.get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| MemoryEntry {
                    id: v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    key: v
                        .get("key")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    content: v
                        .get("content")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    category: v
                        .get("category")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    timestamp: v
                        .get("timestamp")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn run_cron_task(
    base_url: String,
    token: Option<String>,
    tx: mpsc::SyncSender<Msg>,
    actions: mpsc::Receiver<CronAction>,
) {
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let auth_header = token.map(|t| format!("Bearer {t}"));

    fetch_cron(&client, &base_url, &auth_header, &tx).await;

    loop {
        let action = match actions.recv() {
            Ok(a) => a,
            Err(_) => break,
        };
        match action {
            CronAction::Fetch => {
                fetch_cron(&client, &base_url, &auth_header, &tx).await;
            }
            CronAction::Delete { id } => {
                let url = format!("{base_url}/api/cron/{id}");
                let mut req = client.delete(&url);
                if let Some(h) = &auth_header {
                    req = req.header("Authorization", h);
                }
                match req.send().await {
                    Ok(r) if r.status().is_success() => {
                        let _ = tx.send(Msg::CronDeleted(id));
                    }
                    Ok(r) => {
                        let _ = tx.send(Msg::CronError(format!(
                            "Delete failed: HTTP {}",
                            r.status()
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::CronError(format!("Delete failed: {e}")));
                    }
                }
            }
        }
    }
}

async fn fetch_cron(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: &Option<String>,
    tx: &mpsc::SyncSender<Msg>,
) {
    let url = format!("{base_url}/api/cron");
    let mut req = client.get(&url);
    if let Some(h) = auth_header {
        req = req.header("Authorization", h);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) => {
                let jobs = parse_cron_jobs(&body);
                let _ = tx.send(Msg::CronLoaded(jobs));
            }
            Err(e) => {
                let _ = tx.send(Msg::CronError(format!("Parse error: {e}")));
            }
        },
        Ok(r) => {
            let _ = tx.send(Msg::CronError(format!("HTTP {}", r.status())));
        }
        Err(e) => {
            let _ = tx.send(Msg::CronError(format!("Error: {e}")));
        }
    }
}

fn parse_cron_jobs(body: &serde_json::Value) -> Vec<CronJob> {
    body.get("jobs")
        .and_then(|j| j.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| CronJob {
                    id: v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    command: v
                        .get("command")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    expression: v
                        .get("expression")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    job_type: v
                        .get("job_type")
                        .and_then(|x| x.as_str())
                        .unwrap_or("shell")
                        .to_string(),
                    next_run: v
                        .get("next_run")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    last_run: v
                        .get("last_run")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    last_status: v
                        .get("last_status")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    last_output: v
                        .get("last_output")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    prompt: v
                        .get("prompt")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
                    delivery: {
                        let delivery_obj = v.get("delivery").and_then(|x| x.as_object());
                        CronJobDelivery {
                            mode: delivery_obj
                                .and_then(|d| d.get("mode"))
                                .and_then(|x| x.as_str())
                                .unwrap_or("none")
                                .to_string(),
                            channel: delivery_obj
                                .and_then(|d| d.get("channel"))
                                .and_then(|x| x.as_str())
                                .map(|s| s.to_string()),
                            to: delivery_obj
                                .and_then(|d| d.get("to"))
                                .and_then(|x| x.as_str())
                                .map(|s| s.to_string()),
                        }
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn run_costs_task(
    base_url: String,
    token: Option<String>,
    tx: mpsc::SyncSender<Msg>,
    actions: mpsc::Receiver<CostsAction>,
) {
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let auth_header = token.map(|t| format!("Bearer {t}"));

    fetch_costs(&client, &base_url, &auth_header, &tx).await;

    loop {
        let action = match actions.recv() {
            Ok(a) => a,
            Err(_) => break,
        };
        match action {
            CostsAction::Fetch => {
                fetch_costs(&client, &base_url, &auth_header, &tx).await;
            }
        }
    }
}

async fn fetch_costs(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: &Option<String>,
    tx: &mpsc::SyncSender<Msg>,
) {
    let cost_req = {
        let mut r = client.get(format!("{base_url}/api/cost"));
        if let Some(h) = auth_header {
            r = r.header("Authorization", h);
        }
        r.send()
    };
    let status_req = {
        let mut r = client.get(format!("{base_url}/api/status"));
        if let Some(h) = auth_header {
            r = r.header("Authorization", h);
        }
        r.send()
    };

    let (cost_res, status_res) = tokio::join!(cost_req, status_req);

    let summary = match cost_res {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) => parse_cost_summary(&body),
            Err(e) => {
                let _ = tx.send(Msg::CostsError(format!("Parse error: {e}")));
                return;
            }
        },
        Ok(r) => {
            let _ = tx.send(Msg::CostsError(format!("HTTP {}", r.status())));
            return;
        }
        Err(e) => {
            let _ = tx.send(Msg::CostsError(format!("Error: {e}")));
            return;
        }
    };

    let (version, uptime_seconds) = match status_res {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) => (
                body.get("version")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                body.get("uptime_seconds").and_then(|x| x.as_u64()),
            ),
            Err(_) => (None, None),
        },
        _ => (None, None),
    };

    let _ = tx.send(Msg::CostsLoaded(CostsSnapshot {
        summary,
        version,
        uptime_seconds,
    }));
}

async fn run_metrics_task(
    base_url: String,
    token: Option<String>,
    tx: mpsc::SyncSender<Msg>,
    actions: mpsc::Receiver<MetricsAction>,
) {
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let auth_header = token.map(|t| format!("Bearer {t}"));

    fetch_metrics(&client, &base_url, &auth_header, &tx).await;

    loop {
        let action = match actions.recv() {
            Ok(a) => a,
            Err(_) => break,
        };
        match action {
            MetricsAction::Fetch => {
                fetch_metrics(&client, &base_url, &auth_header, &tx).await;
            }
        }
    }
}

async fn fetch_metrics(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: &Option<String>,
    tx: &mpsc::SyncSender<Msg>,
) {
    let url = format!("{base_url}/metrics");
    let mut req = client.get(&url);
    if let Some(h) = auth_header {
        req = req.header("Authorization", h);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(body) => {
                let families = parse_prometheus_text(&body);
                let _ = tx.send(Msg::MetricsLoaded(families));
            }
            Err(e) => {
                let _ = tx.send(Msg::MetricsError(format!("Read error: {e}")));
            }
        },
        Ok(r) => {
            let _ = tx.send(Msg::MetricsError(format!("HTTP {}", r.status())));
        }
        Err(e) => {
            let _ = tx.send(Msg::MetricsError(format!("Error: {e}")));
        }
    }
}

/// Minimal parser for Prometheus text exposition format. Recognizes `# HELP`,
/// `# TYPE`, and metric lines `name[{labels}] value [timestamp]`. Output is
/// grouped by family (the base name preceding any `_bucket`/`_sum`/`_count`
/// histogram suffix is collapsed onto its declared family).
fn parse_prometheus_text(text: &str) -> Vec<MetricFamily> {
    use std::collections::HashMap;

    let mut families: HashMap<String, MetricFamily> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# HELP ") {
            let mut it = rest.splitn(2, ' ');
            if let Some(name) = it.next() {
                let help = it.next().unwrap_or("").to_string();
                let entry = families.entry(name.to_string()).or_insert_with(|| {
                    order.push(name.to_string());
                    MetricFamily {
                        name: name.to_string(),
                        help: String::new(),
                        kind: "untyped".to_string(),
                        samples: Vec::new(),
                    }
                });
                entry.help = help;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            let mut it = rest.splitn(2, ' ');
            if let Some(name) = it.next() {
                let kind = it.next().unwrap_or("untyped").to_string();
                let entry = families.entry(name.to_string()).or_insert_with(|| {
                    order.push(name.to_string());
                    MetricFamily {
                        name: name.to_string(),
                        help: String::new(),
                        kind: "untyped".to_string(),
                        samples: Vec::new(),
                    }
                });
                entry.kind = kind;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        // Parse `name[{labels}] value [timestamp]`
        let (name, rest) = match line.find(['{', ' ']) {
            Some(idx) => (&line[..idx], &line[idx..]),
            None => continue,
        };
        let (labels, value_part) = if let Some(rest) = rest.strip_prefix('{') {
            match rest.find('}') {
                Some(end) => (parse_labels(&rest[..end]), rest[end + 1..].trim()),
                None => continue,
            }
        } else {
            (Vec::new(), rest.trim())
        };
        let value = value_part
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if value.is_empty() {
            continue;
        }

        // Collapse histogram/summary suffixes onto base family name.
        let family_name = histogram_base_name(name).to_string();
        let mut sample_labels = labels;
        if family_name != name {
            sample_labels.insert(0, ("__suffix__".to_string(), suffix_for(name).to_string()));
        }

        let entry = families.entry(family_name.clone()).or_insert_with(|| {
            order.push(family_name.clone());
            MetricFamily {
                name: family_name.clone(),
                help: String::new(),
                kind: "untyped".to_string(),
                samples: Vec::new(),
            }
        });
        entry.samples.push(MetricSample {
            labels: sample_labels,
            value,
        });
    }

    order
        .into_iter()
        .filter_map(|n| families.remove(&n))
        .collect()
}

fn histogram_base_name(name: &str) -> &str {
    for suffix in ["_bucket", "_count", "_sum"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}

fn suffix_for(name: &str) -> &str {
    for suffix in ["_bucket", "_count", "_sum"] {
        if name.ends_with(suffix) {
            return &suffix[1..];
        }
    }
    ""
}

fn parse_labels(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // skip whitespace and commas
        while i < bytes.len() && (bytes[i] == b',' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        // read key
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key = s[key_start..i].trim().to_string();
        i += 1; // skip '='
        if i >= bytes.len() || bytes[i] != b'"' {
            break;
        }
        i += 1; // opening quote
        let mut value = String::new();
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                let next = bytes[i + 1];
                value.push(match next {
                    b'n' => '\n',
                    b'\\' => '\\',
                    b'"' => '"',
                    other => other as char,
                });
                i += 2;
            } else {
                value.push(bytes[i] as char);
                i += 1;
            }
        }
        if i < bytes.len() {
            i += 1; // closing quote
        }
        out.push((key, value));
    }
    out
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn parse_cost_summary(body: &serde_json::Value) -> CostSummary {
    let cost = body.get("cost").unwrap_or(body);
    let mut by_model: Vec<ModelRow> = cost
        .get("by_model")
        .and_then(|m| m.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| ModelRow {
                    model: v
                        .get("model")
                        .and_then(|x| x.as_str())
                        .unwrap_or(k.as_str())
                        .to_string(),
                    cost_usd: v.get("cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0),
                    total_tokens: v.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                    request_count: v.get("request_count").and_then(|x| x.as_u64()).unwrap_or(0)
                        as usize,
                    channel: v
                        .get("channel")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    CostSummary {
        hourly_cost_usd: cost
            .get("hourly_cost_usd")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0),
        daily_cost_usd: cost
            .get("daily_cost_usd")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0),
        monthly_cost_usd: cost
            .get("monthly_cost_usd")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0),
        total_tokens: cost
            .get("total_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0),
        request_count: cost
            .get("request_count")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as usize,
        by_model,
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect()
            }
        })
        .collect()
}

// ── Token / pairing ───────────────────────────────────────────────────────────

const TOKEN_CACHE_FILENAME: &str = "cli-token";

fn token_cache_path(config: &Config) -> std::path::PathBuf {
    config
        .config_path
        .parent()
        .map(|p| p.join(TOKEN_CACHE_FILENAME))
        .unwrap_or_else(|| std::path::PathBuf::from(TOKEN_CACHE_FILENAME))
}

fn load_cached_token(config: &Config) -> Option<String> {
    std::fs::read_to_string(token_cache_path(config))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_cached_token(config: &Config, token: &str) {
    let _ = std::fs::write(token_cache_path(config), token);
}

async fn pair_with_gateway(base_url: &str, code: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/pair"))
        .header("X-Pairing-Code", code)
        .header("X-Device-Name", "TUI")
        .send()
        .await?;
    anyhow::ensure!(
        resp.status().is_success(),
        "Pairing failed: HTTP {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No token in /pair response"))
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run(config: &Config) -> Result<()> {
    let host = &config.gateway.host;
    let port = config.gateway.port;
    let base_url = format!("http://{host}:{port}");
    let sse_url = format!("{base_url}/api/events");

    let token = if let Some(t) = load_cached_token(config) {
        Some(t)
    } else if config.gateway.require_pairing {
        eprintln!(
            "No cached token found. Enter the 6-digit pairing code shown in the gateway terminal:"
        );
        eprint!("> ");
        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        let code = code.trim().to_string();
        let token = pair_with_gateway(&base_url, &code).await?;
        save_cached_token(config, &token);
        Some(token)
    } else {
        None
    };

    let (msg_tx, msg_rx) = mpsc::sync_channel::<Msg>(512);
    let (action_tx, action_rx) = mpsc::sync_channel::<Action>(64);
    let (cron_action_tx, cron_action_rx) = mpsc::sync_channel::<CronAction>(64);
    let (costs_action_tx, costs_action_rx) = mpsc::sync_channel::<CostsAction>(64);
    let (metrics_action_tx, metrics_action_rx) = mpsc::sync_channel::<MetricsAction>(64);

    tokio::spawn(run_event_stream_task(
        sse_url.clone(),
        token.clone(),
        msg_tx.clone(),
    ));
    tokio::spawn(run_memory_task(
        base_url.clone(),
        token.clone(),
        msg_tx.clone(),
        action_rx,
    ));
    tokio::spawn(run_cron_task(
        base_url.clone(),
        token.clone(),
        msg_tx.clone(),
        cron_action_rx,
    ));
    tokio::spawn(run_costs_task(
        base_url.clone(),
        token.clone(),
        msg_tx.clone(),
        costs_action_rx,
    ));
    tokio::spawn(run_metrics_task(base_url, token, msg_tx, metrics_action_rx));

    tokio::task::block_in_place(move || {
        run_tui(
            msg_rx,
            action_tx,
            cron_action_tx,
            costs_action_tx,
            metrics_action_tx,
            sse_url,
        )
    })
}

// ── TUI loop ──────────────────────────────────────────────────────────────────

fn run_tui(
    rx: mpsc::Receiver<Msg>,
    action_tx: mpsc::SyncSender<Action>,
    cron_action_tx: mpsc::SyncSender<CronAction>,
    costs_action_tx: mpsc::SyncSender<CostsAction>,
    metrics_action_tx: mpsc::SyncSender<MetricsAction>,
    sse_url: String,
) -> Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = DashboardApp::new(sse_url);
    let tick = Duration::from_millis(TICK_MS);

    let result: Result<()> = (|| {
        loop {
            terminal.draw(|f| draw(f, &app))?;

            // Drain messages
            loop {
                match rx.try_recv() {
                    Ok(Msg::EventStreamEvent(e)) => app.events.push(e),
                    Ok(Msg::EventStreamConnected) => {
                        app.events.connected = true;
                        app.events.status_msg = String::new();
                    }
                    Ok(Msg::EventStreamDisconnected(msg)) => {
                        app.events.connected = false;
                        app.events.status_msg = msg;
                    }
                    Ok(Msg::MemoryLoaded(entries)) => {
                        app.memory.entries = entries;
                        app.memory.loading = false;
                        app.memory.cursor = 0;
                        app.memory.scroll_offset = 0;
                        app.memory.status = None;
                    }
                    Ok(Msg::MemoryError(msg)) => {
                        app.memory.loading = false;
                        app.memory.status = Some(format!("Error: {msg}"));
                    }
                    Ok(Msg::MemoryDeleted(key)) => {
                        app.memory.entries.retain(|e| e.key != key);
                        if app.memory.cursor >= app.memory.entries.len()
                            && !app.memory.entries.is_empty()
                        {
                            app.memory.cursor = app.memory.entries.len() - 1;
                        }
                        app.memory.mode = MemoryMode::List;
                        app.memory.status = Some(format!("Deleted: {key}"));
                    }
                    Ok(Msg::CronLoaded(jobs)) => {
                        app.cron.jobs = jobs;
                        app.cron.loading = false;
                        app.cron.cursor = 0;
                        app.cron.scroll_offset = 0;
                        app.cron.status = None;
                    }
                    Ok(Msg::CronError(msg)) => {
                        app.cron.loading = false;
                        app.cron.status = Some(format!("Error: {msg}"));
                    }
                    Ok(Msg::CronDeleted(id)) => {
                        app.cron.jobs.retain(|j| j.id != id);
                        if app.cron.cursor >= app.cron.jobs.len() && !app.cron.jobs.is_empty() {
                            app.cron.cursor = app.cron.jobs.len() - 1;
                        }
                        app.cron.mode = CronMode::List;
                        app.cron.status = Some("Job deleted".to_string());
                    }
                    Ok(Msg::CostsLoaded(snap)) => {
                        app.costs.summary = snap.summary;
                        app.costs.version = snap.version;
                        app.costs.uptime_seconds = snap.uptime_seconds;
                        app.costs.loading = false;
                        if app.costs.cursor >= app.costs.summary.by_model.len() {
                            app.costs.cursor = app.costs.summary.by_model.len().saturating_sub(1);
                        }
                        app.costs.scroll_offset = 0;
                        app.costs.status = None;
                    }
                    Ok(Msg::CostsError(msg)) => {
                        app.costs.loading = false;
                        app.costs.status = Some(format!("Error: {msg}"));
                    }
                    Ok(Msg::MetricsLoaded(families)) => {
                        app.metrics.families = families;
                        app.metrics.loading = false;
                        app.metrics.status = None;
                    }
                    Ok(Msg::MetricsError(msg)) => {
                        app.metrics.loading = false;
                        app.metrics.status = Some(format!("Error: {msg}"));
                    }
                    Err(_) => break,
                }
            }

            if event::poll(tick)? {
                if let Event::Key(key) = event::read()? {
                    // Global quit
                    if key.code == KeyCode::Char('q')
                        && !app.events.detail_open
                        && !matches!(app.memory.mode, MemoryMode::Search)
                        && !matches!(app.cron.mode, CronMode::Detail | CronMode::ConfirmDelete)
                    {
                        return Ok(());
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(());
                    }

                    // Tab switching
                    if key.code == KeyCode::Char('1') {
                        if app.tab != Tab::Costs {
                            app.tab = Tab::Costs;
                            app.costs.loading = true;
                            let _ = costs_action_tx.try_send(CostsAction::Fetch);
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('2') {
                        if app.tab != Tab::Memory {
                            app.tab = Tab::Memory;
                            app.memory.loading = true;
                            let _ = action_tx.try_send(Action::FetchMemory { query: None });
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('3') {
                        if app.tab != Tab::Cron {
                            app.tab = Tab::Cron;
                            app.cron.loading = true;
                            let _ = cron_action_tx.try_send(CronAction::Fetch);
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('4') {
                        app.tab = Tab::MissionControl;
                        continue;
                    }
                    if key.code == KeyCode::Char('5') {
                        if app.tab != Tab::Metrics {
                            app.tab = Tab::Metrics;
                            app.metrics.loading = true;
                            let _ = metrics_action_tx.try_send(MetricsAction::Fetch);
                        }
                        continue;
                    }

                    match app.tab {
                        Tab::MissionControl => handle_events_key(&mut app.events, key),
                        Tab::Memory => handle_memory_key(&mut app.memory, &action_tx, key),
                        Tab::Cron => handle_cron_key(&mut app.cron, &cron_action_tx, key),
                        Tab::Costs => handle_costs_key(&mut app.costs, &costs_action_tx, key),
                        Tab::Metrics => {
                            handle_metrics_key(&mut app.metrics, &metrics_action_tx, key)
                        }
                    }
                }
            }
        }
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), Show, LeaveAlternateScreen)?;
    result
}

fn handle_events_key(s: &mut EventsState, key: crossterm::event::KeyEvent) {
    if s.detail_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => s.detail_open = false,
            _ => {}
        }
        return;
    }

    if s.filter_mode {
        match key.code {
            KeyCode::Esc | KeyCode::Char('f') => s.filter_mode = false,
            KeyCode::Left => s.filter_cursor = s.filter_cursor.saturating_sub(1),
            KeyCode::Right => {
                if s.filter_cursor + 1 < s.all_types.len() {
                    s.filter_cursor += 1;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(t) = s.all_types.get(s.filter_cursor).cloned() {
                    s.toggle_filter(&t);
                }
            }
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Char('p') | KeyCode::Char(' ') => s.paused = !s.paused,
            KeyCode::Char('f') => {
                s.filter_mode = true;
                s.filter_cursor = s.filter_cursor.min(s.all_types.len().saturating_sub(1));
            }
            KeyCode::Char('G') | KeyCode::End => s.jump_to_bottom(),
            KeyCode::Up | KeyCode::Char('k') => s.scroll_up(),
            KeyCode::Down | KeyCode::Char('j') => s.scroll_down(),
            KeyCode::PageUp => {
                for _ in 0..10 {
                    s.scroll_up();
                }
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    s.scroll_down();
                }
            }
            KeyCode::Enter => {
                if !s.filtered.is_empty() {
                    s.detail_open = true;
                }
            }
            _ => {}
        }
    }
}

fn handle_memory_key(
    s: &mut MemoryState,
    action_tx: &mpsc::SyncSender<Action>,
    key: crossterm::event::KeyEvent,
) {
    match s.mode {
        MemoryMode::Search => match key.code {
            KeyCode::Esc => {
                s.mode = MemoryMode::List;
                s.search_input.clear();
            }
            KeyCode::Enter => {
                s.mode = MemoryMode::List;
                s.loading = true;
                s.status = None;
                let q = if s.search_input.is_empty() {
                    None
                } else {
                    Some(s.search_input.clone())
                };
                let _ = action_tx.try_send(Action::FetchMemory { query: q });
            }
            KeyCode::Backspace => {
                s.search_input.pop();
            }
            KeyCode::Char(c) => s.search_input.push(c),
            _ => {}
        },

        MemoryMode::Detail => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => s.mode = MemoryMode::List,
            KeyCode::Char('d') => s.mode = MemoryMode::ConfirmDelete,
            _ => {}
        },

        MemoryMode::ConfirmDelete => match key.code {
            KeyCode::Char('y') => {
                if let Some(entry) = s.selected() {
                    let _ = action_tx.try_send(Action::DeleteMemory {
                        key: entry.key.clone(),
                    });
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => s.mode = MemoryMode::Detail,
            _ => {}
        },

        MemoryMode::List => {
            // compute visible for scroll
            const APPROX_VISIBLE: usize = 20;
            match key.code {
                KeyCode::Char('/') => {
                    s.mode = MemoryMode::Search;
                    s.search_input.clear();
                }
                KeyCode::Char('r') => {
                    s.loading = true;
                    s.status = None;
                    let _ = action_tx.try_send(Action::FetchMemory { query: None });
                }
                KeyCode::Up | KeyCode::Char('k') => s.move_up(),
                KeyCode::Down | KeyCode::Char('j') => s.move_down(APPROX_VISIBLE),
                KeyCode::PageUp => {
                    for _ in 0..10 {
                        s.move_up();
                    }
                }
                KeyCode::PageDown => {
                    for _ in 0..10 {
                        s.move_down(APPROX_VISIBLE);
                    }
                }
                KeyCode::Enter => {
                    if s.selected().is_some() {
                        s.mode = MemoryMode::Detail;
                    }
                }
                KeyCode::Char('d') => {
                    if s.selected().is_some() {
                        s.mode = MemoryMode::ConfirmDelete;
                    }
                }
                _ => {}
            }
        }
    }
}

fn handle_costs_key(
    s: &mut CostsState,
    action_tx: &mpsc::SyncSender<CostsAction>,
    key: crossterm::event::KeyEvent,
) {
    const APPROX_VISIBLE: usize = 20;
    match key.code {
        KeyCode::Char('r') => {
            s.loading = true;
            s.status = None;
            let _ = action_tx.try_send(CostsAction::Fetch);
        }
        KeyCode::Up | KeyCode::Char('k') => s.move_up(),
        KeyCode::Down | KeyCode::Char('j') => s.move_down(APPROX_VISIBLE),
        KeyCode::PageUp => {
            for _ in 0..10 {
                s.move_up();
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                s.move_down(APPROX_VISIBLE);
            }
        }
        _ => {}
    }
}

fn handle_metrics_key(
    s: &mut MetricsState,
    action_tx: &mpsc::SyncSender<MetricsAction>,
    key: crossterm::event::KeyEvent,
) {
    // Approximate total render lines for clamping. We don't know visible height
    // here; rely on draw() to clamp scroll. Treat max as a large number.
    let max = usize::MAX;
    match key.code {
        KeyCode::Char('r') => {
            s.loading = true;
            s.status = None;
            let _ = action_tx.try_send(MetricsAction::Fetch);
        }
        KeyCode::Up | KeyCode::Char('k') => s.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => s.scroll_down(max),
        KeyCode::PageUp => {
            for _ in 0..10 {
                s.scroll_up();
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                s.scroll_down(max);
            }
        }
        _ => {}
    }
}

fn handle_cron_key(
    s: &mut CronState,
    action_tx: &mpsc::SyncSender<CronAction>,
    key: crossterm::event::KeyEvent,
) {
    match s.mode {
        CronMode::Detail => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => s.mode = CronMode::List,
            KeyCode::Char('d') => s.mode = CronMode::ConfirmDelete,
            _ => {}
        },

        CronMode::ConfirmDelete => match key.code {
            KeyCode::Char('y') => {
                if let Some(job) = s.selected() {
                    let _ = action_tx.try_send(CronAction::Delete { id: job.id.clone() });
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => s.mode = CronMode::Detail,
            _ => {}
        },

        CronMode::List => {
            const APPROX_VISIBLE: usize = 20;
            match key.code {
                KeyCode::Char('r') => {
                    s.loading = true;
                    s.status = None;
                    let _ = action_tx.try_send(CronAction::Fetch);
                }
                KeyCode::Up | KeyCode::Char('k') => s.move_up(),
                KeyCode::Down | KeyCode::Char('j') => s.move_down(APPROX_VISIBLE),
                KeyCode::PageUp => {
                    for _ in 0..10 {
                        s.move_up();
                    }
                }
                KeyCode::PageDown => {
                    for _ in 0..10 {
                        s.move_down(APPROX_VISIBLE);
                    }
                }
                KeyCode::Enter => {
                    if s.selected().is_some() {
                        s.mode = CronMode::Detail;
                    }
                }
                KeyCode::Char('d') => {
                    if s.selected().is_some() {
                        s.mode = CronMode::ConfirmDelete;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_help_type_and_simple_counter() {
        let text = "\
# HELP agentzero_heartbeat_ticks_total Total heartbeat ticks
# TYPE agentzero_heartbeat_ticks_total counter
agentzero_heartbeat_ticks_total 7
";
        let families = parse_prometheus_text(text);
        assert_eq!(families.len(), 1);
        let f = &families[0];
        assert_eq!(f.name, "agentzero_heartbeat_ticks_total");
        assert_eq!(f.kind, "counter");
        assert_eq!(f.help, "Total heartbeat ticks");
        assert_eq!(f.samples.len(), 1);
        assert!(f.samples[0].labels.is_empty());
        assert_eq!(f.samples[0].value, "7");
    }

    #[test]
    fn parses_labels_with_escapes() {
        // Two label values: one plain, one using \" \\ \n escapes.
        let text = "\
# TYPE agentzero_errors_total counter
agentzero_errors_total{component=\"provider\"} 2
agentzero_errors_total{component=\"a\\\"b\\\\c\\nd\"} 1
";
        let families = parse_prometheus_text(text);
        assert_eq!(families.len(), 1);
        let samples = &families[0].samples;
        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples[0].labels,
            vec![("component".into(), "provider".into())]
        );
        assert_eq!(samples[0].value, "2");
        // After unescaping: a "  \  <LF> d
        assert_eq!(samples[1].labels[0].1, "a\"b\\c\nd");
    }

    #[test]
    fn collapses_histogram_suffixes_onto_base_family() {
        let text = "\
# HELP agentzero_tool_duration_seconds Tool execution duration
# TYPE agentzero_tool_duration_seconds histogram
agentzero_tool_duration_seconds_bucket{tool=\"shell\",le=\"0.1\"} 3
agentzero_tool_duration_seconds_bucket{tool=\"shell\",le=\"+Inf\"} 5
agentzero_tool_duration_seconds_sum{tool=\"shell\"} 0.42
agentzero_tool_duration_seconds_count{tool=\"shell\"} 5
";
        let families = parse_prometheus_text(text);
        assert_eq!(families.len(), 1);
        let f = &families[0];
        assert_eq!(f.name, "agentzero_tool_duration_seconds");
        assert_eq!(f.kind, "histogram");
        assert_eq!(f.samples.len(), 4);
        let suffixes: Vec<&str> = f
            .samples
            .iter()
            .map(|s| {
                s.labels
                    .iter()
                    .find(|(k, _)| k == "__suffix__")
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
            })
            .collect();
        assert_eq!(suffixes, vec!["bucket", "bucket", "sum", "count"]);
    }
}
