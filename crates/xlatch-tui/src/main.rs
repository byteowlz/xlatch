//! Reference TUI built on [`byteowlz_tui_kit`].
//!
//! Renders the "modern" byteowlz look: air over borders, weight-based hierarchy, one
//! accent, a fuzzy command palette, and key progressions with an on-demand WhichKey hint.
//! New byteowlz TUIs should start from this shape; see the `tui-design` skill.

use std::time::Duration;

use anyhow::Result;
use byteowlz_tui_kit::action::{Action, ActionId, Key, KeyRouter, Route};
use byteowlz_tui_kit::event::{AppEvent, poll_event};
use byteowlz_tui_kit::palette::{CommandPalette, PaletteOutcome};
use byteowlz_tui_kit::prelude::*;
use byteowlz_tui_kit::terminal::TerminalGuard;
use byteowlz_tui_kit::whichkey;
use byteowlz_tui_kit::widgets::{bar, panel};
use clap::Parser;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Padding, Paragraph};

/// The actions available in Normal mode. Defined as data — adding one never adds a mode.
fn actions() -> Vec<Action> {
    vec![
        Action::new(ActionId::new("quit"), "Quit").key(Key::ctrl_char('c')),
        Action::new(ActionId::new("item.open"), "Open in editor").key(Key::enter()),
        Action::new(ActionId::new("item.delete"), "Delete").key(Key::char('d')),
        Action::new(ActionId::new("item.add"), "Add new").key(Key::char('a')),
        Action::new(ActionId::new("palette.open"), "Command palette").key(Key::char(':')),
        Action::new(ActionId::new("help"), "Help").key(Key::char('?')),
        Action::new(ActionId::new("sort.date"), "Sort by date")
            .keys(&[Key::char('s'), Key::char('d')]),
        Action::new(ActionId::new("sort.importance"), "Sort by importance")
            .keys(&[Key::char('s'), Key::char('i')]),
        Action::new(ActionId::new("sort.category"), "Sort by category")
            .keys(&[Key::char('s'), Key::char('c')]),
        Action::new(ActionId::new("nav.down"), "Cursor down").key(Key::char('j')),
        Action::new(ActionId::new("nav.up"), "Cursor up").key(Key::char('k')),
    ]
}

/// One demo item, shaped like a memory entry.
#[derive(Debug, Clone)]
struct Item {
    title: String,
    meta: String,
    body: String,
}

/// Seed items for the demo.
const SEED: &[(&str, &str, &str)] = &[
    (
        "Port cleanup needs wait for exit",
        "2d · debugging",
        "Ports linger until the process fully exits; poll until the PID is gone.",
    ),
    (
        "OpenCode PATCH renames session",
        "3d · debugging",
        "PATCH /session/{id} accepts {title} to rename a session.",
    ),
    (
        "Voice mode uses eaRS for STT",
        "5d · architecture",
        "Voice mode uses eaRS for STT and kokorox for TTS over a WebSocket.",
    ),
    (
        "sqlx offline mode for CI",
        "1w · tooling",
        "sqlx offline mode lets CI build without a live database.",
    ),
    (
        "tokio task abort ordering",
        "1w · patterns",
        "Aborting a tokio task mid-await can drop guards before completion.",
    ),
];

fn seed_items() -> Vec<Item> {
    SEED.iter()
        .map(|(title, meta, body)| Item {
            title: (*title).to_string(),
            meta: (*meta).to_string(),
            body: (*body).to_string(),
        })
        .collect()
}

/// An in-progress key progression and its next-key options, for the WhichKey hint.
type PendingHint = (Vec<Key>, Vec<(Key, &'static str)>);

/// The app: state plus the kit pieces it drives.
struct App {
    theme: Theme,
    items: Vec<Item>,
    selection: Selection,
    router: KeyRouter<'static>,
    status: String,
    palette: Option<CommandPalette>,
    pending_hint: Option<PendingHint>,
}

impl App {
    fn new() -> Self {
        // NOTE: the router borrows the action table for its lifetime. We leak the boxed
        // table to a 'static reference so the borrow is valid for the whole session; the
        // process is short-lived so this bounded leak is acceptable for a demo.
        let actions: &'static [Action] = Box::leak(actions().into_boxed_slice());
        let items = seed_items();
        let mut selection = Selection::default();
        selection.next(items.len());
        Self {
            theme: Theme::ansi_default(),
            items,
            selection,
            router: KeyRouter::new(actions),
            status: format!("{} items", SEED.len()),
            palette: None,
            pending_hint: None,
        }
    }
}

#[derive(Parser)]
#[command(name = "xlatch-tui", version, about = "Reference byteowlz TUI")]
struct Cli {}

/// The top-level flow signal.
enum Flow {
    Quit,
    Continue,
}

fn main() -> Result<()> {
    let _ = Cli::parse();
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new();
    let tick = Duration::from_millis(120);
    loop {
        guard.draw(|frame| draw(frame, &mut app))?;
        let Some(event) = poll_event(tick)? else {
            continue;
        };
        if matches!(handle(event, &mut app), Flow::Quit) {
            break;
        }
    }
    Ok(())
}

/// Dispatch a normalized event, returning whether to quit.
fn handle(event: AppEvent, app: &mut App) -> Flow {
    let key = match event {
        AppEvent::Key(key) => key,
        AppEvent::Tick | AppEvent::Resize(_, _) => return Flow::Continue,
    };
    if app.palette.is_some() {
        return handle_palette(key, app);
    }
    handle_normal(key, app)
}

/// Handle a key while the command palette is open.
fn handle_palette(key: Key, app: &mut App) -> Flow {
    let outcome = app
        .palette
        .as_mut()
        .map_or(PaletteOutcome::Closed, |p| p.handle(key));
    match outcome {
        PaletteOutcome::Closed => app.palette = None,
        PaletteOutcome::Run(id) => {
            app.palette = None;
            apply_action(id, app);
            if id == ActionId::new("quit") {
                return Flow::Quit;
            }
        }
        PaletteOutcome::Open => {}
    }
    Flow::Continue
}

/// Handle a key in Normal mode: route through the prefix machine, or open the palette.
fn handle_normal(key: Key, app: &mut App) -> Flow {
    if key == Key::char('q') {
        return Flow::Quit;
    }
    if key == Key::char(':') {
        app.palette = Some(CommandPalette::new(app.router.actions_ref().to_vec()));
        app.pending_hint = None;
        app.router.reset();
        return Flow::Continue;
    }
    match app.router.feed(key) {
        Route::Action(id) => {
            app.pending_hint = None;
            apply_action(id, app);
            if id == ActionId::new("quit") {
                return Flow::Quit;
            }
        }
        Route::Prefix(options) => {
            app.pending_hint = Some((app.router.prefix_ref().to_vec(), options));
        }
        Route::Miss => app.pending_hint = None,
    }
    Flow::Continue
}

/// Apply an action by id (navigation + the few that mutate state).
fn apply_action(id: ActionId, app: &mut App) {
    let max = app.items.len();
    let moved = match id {
        x if x == ActionId::new("nav.down") => {
            app.selection.next(max);
            true
        }
        x if x == ActionId::new("nav.up") => {
            app.selection.previous(max);
            true
        }
        _ => false,
    };
    if !moved && id != ActionId::new("quit") {
        app.status = format!("ran: {id}");
    }
}

/// Render one frame.
fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let [header_area, main_area, status_area, hint_area] = layout(frame);
    let [list_area, detail_area] = body(main_area);
    draw_header(frame, app, header_area);
    draw_list(frame, app, list_area);
    draw_detail(frame, app, detail_area);
    draw_status_row(frame, app, status_area);
    if let Some((prefix, options)) = &app.pending_hint {
        whichkey::draw_hint(frame, hint_area, app.theme, prefix, options);
    }
    if let Some(palette) = app.palette.as_mut() {
        palette.draw(frame, app.theme);
    }
}

/// Split the frame into header, main, status, and hint rows.
fn layout(frame: &Frame<'_>) -> [Rect; 4] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    [chunks[0], chunks[1], chunks[2], chunks[3]]
}

/// Split the main area into list (left), gap, and detail (right) columns.
fn body(area: Rect) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(38),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    [chunks[0], chunks[2]]
}

/// Draw the header strip: app name + context, on a filled bar.
fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let line = Line::from(vec![
        Span::styled(" memory ", theme.on_bar_bold(Token::Accent)),
        Span::styled("  ·  ", theme.on_bar(Token::Muted)),
        Span::styled("default store", theme.on_bar(Token::Muted)),
    ]);
    frame.render_widget(bar(line, theme), area);
}

/// Draw the status line: counts on the left, key hints on the right.
fn draw_status_row(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let left = if app.selection.has_selection() {
        format!(
            "{} · {} of {} selected",
            app.status,
            app.selection.selected().len(),
            app.items.len()
        )
    } else {
        format!("{} · {} items", app.status, app.items.len())
    };
    let hints: [(&str, &str); 6] = [
        ("Ret", "open"),
        ("d", "delete"),
        ("s", "sort"),
        (":", "commands"),
        ("?", "help"),
        ("q", "quit"),
    ];
    draw_status_bar(frame, area, app.theme, &left, &hints);
}

/// Draw the item list inside an active titled panel.
fn draw_list(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let items: Vec<ListItem<'_>> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| list_row(i, item, app, theme))
        .collect();
    let list = List::new(items).highlight_style(theme.focus().add_modifier(Modifier::BOLD));
    let block = panel("items", theme, true);
    let mut state = app.selection.state();
    frame.render_stateful_widget(list.block(block), area, &mut state);
}

/// Build one list row.
fn list_row<'a>(index: usize, item: &'a Item, app: &App, theme: Theme) -> ListItem<'a> {
    let marker = if app.selection.is_selected(index) {
        "● "
    } else {
        "  "
    };
    let style = if index == app.selection.index() {
        theme.focus()
    } else {
        theme.fg(Token::Primary)
    };
    let line = Line::from(vec![
        Span::styled(marker, theme.fg(Token::Accent)),
        Span::styled(&item.title, style),
    ]);
    ListItem::new(line)
}

/// Draw the detail pane — progressive disclosure, inside a titled panel.
fn draw_detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let idx = app.selection.index() % app.items.len();
    let item = &app.items[idx];
    let lines = vec![
        Line::from(Span::styled(
            format!("MEMORY · {}", item.meta),
            theme.fg(Token::Muted),
        )),
        Line::from(""),
        Line::from(Span::styled(&item.title, theme.fg_bold(Token::Primary))),
        Line::from(Span::styled(&item.meta, theme.fg(Token::Muted))),
        Line::from(""),
        Line::from(Span::styled(&item.body, theme.fg(Token::Muted))),
        Line::from(""),
        Line::from(Span::styled(
            "Ret open full editor",
            theme.fg(Token::Accent),
        )),
    ];
    let block = panel("detail", theme, false).padding(Padding::uniform(1));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
