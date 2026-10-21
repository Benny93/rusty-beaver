//! The window: Database Navigator on the left, editor tabs (SQL editor + result grid, or
//! table data / columns) on the right, a toolbar, a status bar, the New Connection dialog
//! and the splash screen. MCP commands (see `mcp.rs`) are handled in `handle`.

mod connections;
mod diagram;
mod dialogs;
mod editor_area;
mod execute;
mod grid_ops;
mod handle;
mod navigator;
mod scripts;
mod views;

use crate::complete::{SharedCatalog, SqlCompletion};
use crate::db::{self, Auth, ConnConfig, Driver, QueryResult, Session};
use crate::grid::Grid;
use crate::history;
use crate::mcp;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{
    ColumnSort, DataTable, TableDelegate as _, TableEvent, TableState,
};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

actions!(
    rusty_beaver,
    [
        Quit,
        RunQuery,
        RunScript,
        ExplainQuery,
        FormatSql,
        OpenScript,
        SaveScriptAs,
        ShowShortcuts,
        FollowSystemTheme,
        CloseOtherTabs,
        CloseAllTabs,
        ToggleComment,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9,
        CancelQuery,
        NewEditor,
        NewConnection,
        CloseTab,
        ToggleTheme
    ]
);
actions!(
    grid,
    [
        CopyValue,
        CopyRow,
        CopyInsert,
        CopyAllMarkdown,
        CopyColumnInList,
        FilterByValue,
        FilterExcludeValue,
        DuplicateRowAction,
        OpenReferencedRow,
        EditCell,
        SetNull,
        DeleteRow,
        AddRow,
        SaveChanges,
        RevertChanges,
        CancelEdit
    ]
);

/// What a grid command does; shared by the context menu, shortcuts and MCP.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GridOp {
    CopyValue,
    CopyRow,
    CopyInsert,
    /// The whole grid as a Markdown table (handy for pasting into chats and docs).
    CopyAllMarkdown,
    /// The selected column's distinct values as an SQL IN list.
    CopyColumnIn,
    EditCell,
    SetNull,
    DeleteRow,
    AddRow,
    Save,
    Revert,
    /// Filter the table tab by the selected cell: `col = value` (or IS NULL), `<>` with Exclude.
    FilterBy,
    FilterExclude,
    /// Copy the selected row into a new one (key columns left empty).
    DuplicateRow,
    /// Open the table a foreign-key cell points to, filtered to the referenced row.
    FollowFk,
}

/// Beaver orange: links in the ER diagram and navigator.
const BRAND: Rgba = Rgba {
    r: 0.878,
    g: 0.439,
    b: 0.165,
    a: 1.,
};

/// How long a copy button shows its tick.
const COPIED_FOR: Duration = Duration::from_millis(1500);

/// Rows a table tab loads per page.
const PAGE: usize = 200;
const SPLASH_HOLD: Duration = Duration::from_millis(2200);
const SPLASH_FADE: Duration = Duration::from_millis(400);
static SPLASH_PNG: &[u8] = include_bytes!("../assets/splash.png");
static ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

pub fn run(mcp_port: u16) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            crate::platform::set_app_icon(ICON_PNG);
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-enter", RunQuery, None),
                KeyBinding::new("ctrl-enter", RunQuery, None),
                KeyBinding::new("cmd-shift-enter", RunScript, None),
                KeyBinding::new("f1", ShowShortcuts, None),
                KeyBinding::new("cmd-shift-/", ShowShortcuts, None),
                KeyBinding::new("ctrl-shift-/", ShowShortcuts, None),
                KeyBinding::new("cmd-/", ToggleComment, None),
                KeyBinding::new("cmd-1", Tab1, None),
                KeyBinding::new("cmd-2", Tab2, None),
                KeyBinding::new("cmd-3", Tab3, None),
                KeyBinding::new("cmd-4", Tab4, None),
                KeyBinding::new("cmd-5", Tab5, None),
                KeyBinding::new("cmd-6", Tab6, None),
                KeyBinding::new("cmd-7", Tab7, None),
                KeyBinding::new("cmd-8", Tab8, None),
                KeyBinding::new("cmd-9", Tab9, None),
                KeyBinding::new("alt-1", Tab1, None),
                KeyBinding::new("alt-2", Tab2, None),
                KeyBinding::new("alt-3", Tab3, None),
                KeyBinding::new("alt-4", Tab4, None),
                KeyBinding::new("alt-5", Tab5, None),
                KeyBinding::new("alt-6", Tab6, None),
                KeyBinding::new("alt-7", Tab7, None),
                KeyBinding::new("alt-8", Tab8, None),
                KeyBinding::new("alt-9", Tab9, None),
                KeyBinding::new("ctrl-/", ToggleComment, None),
                KeyBinding::new("cmd-o", OpenScript, None),
                KeyBinding::new("ctrl-o", OpenScript, None),
                KeyBinding::new("cmd-shift-s", SaveScriptAs, None),
                KeyBinding::new("ctrl-shift-s", SaveScriptAs, None),
                KeyBinding::new("cmd-shift-e", ExplainQuery, None),
                KeyBinding::new("ctrl-shift-e", ExplainQuery, None),
                KeyBinding::new("cmd-shift-f", FormatSql, None),
                KeyBinding::new("ctrl-shift-f", FormatSql, None),
                KeyBinding::new("ctrl-shift-enter", RunScript, None),
                KeyBinding::new("alt-x", RunScript, None),
                KeyBinding::new("cmd-.", CancelQuery, None),
                KeyBinding::new("ctrl-.", CancelQuery, None),
                KeyBinding::new("cmd-t", NewEditor, None),
                KeyBinding::new("ctrl-t", NewEditor, None),
                KeyBinding::new("cmd-n", NewConnection, None),
                KeyBinding::new("ctrl-n", NewConnection, None),
                KeyBinding::new("cmd-w", CloseTab, None),
                KeyBinding::new("ctrl-w", CloseTab, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-c", CopyValue, Some("DataTable")),
                KeyBinding::new("ctrl-c", CopyValue, Some("DataTable")),
                KeyBinding::new("cmd-shift-c", CopyRow, Some("DataTable")),
                KeyBinding::new("ctrl-shift-c", CopyRow, Some("DataTable")),
                KeyBinding::new("f2", EditCell, Some("DataTable")),
                KeyBinding::new("cmd-backspace", DeleteRow, Some("DataTable")),
                KeyBinding::new("ctrl-delete", DeleteRow, Some("DataTable")),
                KeyBinding::new("cmd-s", SaveChanges, None),
                KeyBinding::new("ctrl-s", SaveChanges, None),
                KeyBinding::new("escape", CancelEdit, Some("CellEdit")),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus([
                Menu {
                    name: "Rusty Beaver".into(),
                    items: vec![MenuItem::action("Quit Rusty Beaver", Quit)],
                    disabled: false,
                },
                Menu {
                    name: "Database".into(),
                    items: vec![
                        MenuItem::action("New Connection…", NewConnection),
                        MenuItem::action("New SQL Editor", NewEditor),
                        MenuItem::action("Open SQL Script…", OpenScript),
                        MenuItem::action("Save Script / Changes", SaveChanges),
                        MenuItem::action("Save Script As…", SaveScriptAs),
                        MenuItem::action("Execute Statement", RunQuery),
                        MenuItem::action("Execute Script", RunScript),
                        MenuItem::action("Explain Plan", ExplainQuery),
                        MenuItem::action("Format SQL", FormatSql),
                        MenuItem::action("Cancel Query", CancelQuery),
                        MenuItem::separator(),
                        MenuItem::action("Close Tab", CloseTab),
                        MenuItem::action("Close Other Tabs", CloseOtherTabs),
                        MenuItem::action("Close All Tabs", CloseAllTabs),
                        MenuItem::action("Toggle Dark Theme", ToggleTheme),
                        MenuItem::action("Theme: Follow System", FollowSystemTheme),
                        MenuItem::separator(),
                        MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
                    ],
                    disabled: false,
                },
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let (calls, mcp_status) = mcp::spawn(mcp_port);
            cx.spawn(async move |cx| {
                let options = WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Rusty Beaver".into()),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(80.), px(60.)),
                        size: size(px(1360.), px(860.)),
                    })),
                    window_min_size: Some(size(px(900.), px(560.))),
                    app_id: Some("rusty-beaver".into()),
                    ..Default::default()
                };
                cx.open_window(options, |window, cx| {
                    let dark = match settings()["theme"].as_str() {
                        Some("light") => false,
                        Some("system") => system_dark(window),
                        _ => true,
                    };
                    Theme::change(
                        if dark {
                            ThemeMode::Dark
                        } else {
                            ThemeMode::Light
                        },
                        Some(window),
                        cx,
                    );
                    show_scrollbars(cx);
                    let mcp = match mcp_status.recv_timeout(Duration::from_secs(2)) {
                        Ok(Ok(())) => format!("MCP: http://127.0.0.1:{mcp_port}/mcp"),
                        Ok(Err(e)) => format!("MCP off ({e})"),
                        Err(_) => "MCP starting…".into(),
                    };
                    let view = cx.new(|cx| {
                        let mut v = AppView::new(calls, mcp, window, cx);
                        v.restore_session(window, cx);
                        v
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open window");
            })
            .detach();
            // Automation (tests, screenshots) starts the app without taking keyboard focus from the user.
            if std::env::var_os("RUSTY_BEAVER_BACKGROUND").is_none() {
                cx.activate(true);
            }
        });
}

#[derive(Clone, Debug, PartialEq)]
enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

struct Conn {
    id: u64,
    cfg: ConnConfig,
    /// Server product and version, once connected.
    server: Option<String>,
    session: Option<Arc<Mutex<Session>>>,
    /// Stops the running statement while `session` is locked by it.
    canceller: Option<Arc<db::Canceller>>,
    /// Manual commit: a transaction is open, with this many statements run in it.
    in_tx: bool,
    tx_statements: usize,
    status: ConnStatus,
    expanded: bool,
    /// (name, TABLE|VIEW) once loaded.
    tables: Vec<(String, String)>,
    /// Expanded tables in the navigator with their columns (name, type, nullable, key).
    columns: Vec<(String, QueryResult)>,
    /// Tables and columns for SQL autocompletion.
    catalog: SharedCatalog,
    /// Callers waiting for the connect in progress.
    waiting: Vec<Done>,
    /// Bumped by every connect and disconnect: a connect that finishes late sees it changed.
    generation: u64,
}

/// What `run` executes.
enum Source {
    /// This SQL, also put into the editor.
    Given(String),
    /// The selection, else the statement under the cursor.
    Statement,
    /// The whole editor.
    Script,
    /// This SQL; the editor is left alone (EXPLAIN of its statement).
    Raw(String),
}

/// What the left sidebar shows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sidebar {
    Navigator,
    History,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TableView {
    Data,
    Columns,
    Ddl,
}

#[derive(Clone, PartialEq)]
enum TabKind {
    Sql,
    Table(String),
    /// ER diagram of a connection.
    Diagram,
}

struct EditorTab {
    title: String,
    conn: u64,
    kind: TabKind,
    /// The SQL of a script tab; for a table tab the SELECT its filter produced.
    editor: Entity<EditorState>,
    grid: Entity<TableState<Grid>>,
    /// Columns view of a table tab.
    columns: Entity<TableState<Grid>>,
    view: TableView,
    /// CREATE statement of a table tab (read-only editor).
    ddl: Entity<EditorState>,
    /// The result as a plain-text table (read-only editor, so it can be selected and copied).
    text: Entity<EditorState>,
    /// WHERE clause of a table tab.
    filter: Entity<InputState>,
    /// Cell the user last clicked: (display row, column).
    cell: Option<(usize, usize)>,
    /// Cell being edited: (row id, column), and its input.
    editing: Option<(usize, usize)>,
    edit_input: Entity<InputState>,
    /// Quick search over the result rows (SQL tabs).
    search: Entity<InputState>,
    /// Primary key column names of a table tab.
    key: Vec<String>,
    running: bool,
    /// When the running statement started (for the live timer).
    started: Option<Instant>,
    /// Show the selected row as a vertical form instead of the grid.
    record: bool,
    /// Side panel with the selected cell's full value.
    value_panel: bool,
    /// Restored from the last session and not loaded yet.
    stale: bool,
    /// The result as aligned plain text instead of the grid.
    text_view: bool,
    /// Bar chart of the result instead of the grid, with the value column it plots.
    chart: bool,
    chart_col: Option<usize>,
    /// Production safety: the next write was confirmed (dialog or `confirm`), or comes from an agent.
    confirmed: bool,
    agent: bool,
    /// Last values of the `:name` parameters, and whether an agent just supplied them.
    param_values: std::collections::HashMap<String, String>,
    params_given: bool,
    /// File a script tab was opened from / saved to.
    path: Option<std::path::PathBuf>,
    /// Every result of the last execution (a script has one per statement); `result_ix` is shown.
    results: Vec<QueryResult>,
    result_ix: usize,
    /// ER diagram tab: the model, and the box being dragged (name, mouse start, box start).
    diagram: Option<crate::er::Diagram>,
    drag: Option<(String, Point<Pixels>, (f32, f32))>,
    /// Rows a table tab fetches ("Load more" raises it) and the table's row count.
    limit: usize,
    total: Option<u64>,
    /// "42 rows in 3 ms" or the error.
    message: Option<Result<String, String>>,
}

struct ConnDialog {
    driver: Driver,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    database: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    auth: crate::db::Auth,
    ssl: crate::db::Ssl,
    /// Vault modes: secret or credentials path, the secret's field, address, OIDC role and mount.
    vault_path: Entity<InputState>,
    vault_field: Entity<InputState>,
    vault_addr: Entity<InputState>,
    vault_role: Entity<InputState>,
    vault_mount: Entity<InputState>,
    test: Option<Result<String, String>>,
    testing: bool,
    /// Editing this saved connection instead of adding one.
    editing: Option<u64>,
    production: bool,
    read_only: bool,
}

/// Values asked for the `:name` parameters of a statement before it runs.
struct ParamPrompt {
    /// `tab_key` of the tab.
    tab: EntityId,
    sql: String,
    inputs: Vec<(String, Entity<InputState>)>,
    max_rows: usize,
    done: Option<Done>,
}

/// Blocking work on a session, re-runnable after a reconnect.
type Work<T> = Arc<dyn Fn(&mut Session) -> Result<T, String> + Send + Sync>;

type Callback<T> =
    Box<dyn FnOnce(&mut AppView, Result<T, String>, &mut Window, &mut Context<AppView>)>;
type Done = Callback<Value>;

pub struct AppView {
    conns: Vec<Conn>,
    next_id: u64,
    tabs: Vec<EditorTab>,
    active: usize,
    /// Navigator selection: connection id, optional table.
    selected: Option<(u64, Option<String>)>,
    dialog: Option<ConnDialog>,
    dark: bool,
    status: String,
    mcp: String,
    started: Instant,
    splash_image: Arc<Image>,
    focus: FocusHandle,
    sidebar: Sidebar,
    /// Filters the navigator's tables or the history.
    side_filter: Entity<InputState>,
    history: Vec<history::Entry>,
    /// The query parameter dialog, while open.
    params: Option<ParamPrompt>,
    /// The theme follows the OS appearance.
    follow_system: bool,
    /// The keyboard shortcut overlay is open.
    shortcuts: bool,
    /// Horizontal scroll of the tab bar, and the tab last scrolled into view.
    tab_scroll: ScrollHandle,
    revealed_tab: std::cell::Cell<Option<usize>>,
    /// The copy button that was just used, and when (it shows a tick for a moment).
    copied: Option<(&'static str, Instant)>,
    /// A save of the open scripts is scheduled.
    session_dirty: bool,
}

impl AppView {
    fn new(
        mut calls: tokio::sync::mpsc::UnboundedReceiver<mcp::Call>,
        mcp: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(call) = calls.recv().await {
                if this
                    .update_in(cx, |v, window, cx| v.handle(call, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // Passwords move into the OS keychain the first time (and new connections get ids).
        let (loaded, resave) = db::load_connections();
        let mut status = "Ready".to_string();
        if resave {
            match db::save_connections(&loaded) {
                Ok(w) if !w.is_empty() => status = w.join("; "),
                Err(e) => status = format!("Could not save connections: {e}"),
                _ => {}
            }
        }
        let conns: Vec<Conn> = loaded
            .into_iter()
            .enumerate()
            .map(|(i, cfg)| Conn {
                id: i as u64 + 1,
                cfg,
                session: None,
                status: ConnStatus::Disconnected,
                expanded: false,
                tables: vec![],
                columns: vec![],
                catalog: Default::default(),
                canceller: None,
                server: None,
                in_tx: false,
                tx_statements: 0,
                waiting: vec![],
                generation: 0,
            })
            .collect();
        let next_id = conns.len() as u64 + 1;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_window_appearance(window, |v: &mut Self, window, cx| {
            if v.follow_system {
                v.apply_theme(system_dark(window), window, cx);
            }
        })
        .detach();
        let side_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tables…"));
        cx.subscribe(&side_filter, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        Self {
            selected: conns.first().map(|c| (c.id, None)),
            conns,
            next_id,
            tabs: vec![],
            active: 0,
            dialog: None,
            dark: match settings()["theme"].as_str() {
                Some("light") => false,
                Some("system") => system_dark(window),
                _ => true,
            },
            follow_system: settings()["theme"].as_str() == Some("system"),
            status,
            mcp,
            started: Instant::now(),
            splash_image: Arc::new(Image::from_bytes(ImageFormat::Png, SPLASH_PNG.to_vec())),
            focus,
            sidebar: Sidebar::Navigator,
            side_filter,
            history: history::load(),
            session_dirty: false,
            shortcuts: false,
            tab_scroll: ScrollHandle::new(),
            revealed_tab: Default::default(),
            copied: None,
            params: None,
        }
    }

    /// A tab's key for async work: its grid's entity, which (unlike its index) survives
    /// other tabs closing.
    fn tab_key(&self, tab: usize) -> Option<EntityId> {
        self.tabs.get(tab).map(|t| t.grid.entity_id())
    }

    fn tab_ix(&self, key: EntityId) -> Option<usize> {
        self.tabs.iter().position(|t| t.grid.entity_id() == key)
    }

    /// Make a script tab of connection `id` active (the active tab if it is one). SQL other
    /// than its own SELECT never runs in a table tab: its grid saves edits into the table.
    fn activate_script_tab(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let script = |t: &EditorTab| t.conn == id && t.kind == TabKind::Sql;
        if self.tabs.get(self.active).is_some_and(script) {
            return Ok(self.active);
        }
        match self.tabs.iter().position(script) {
            Some(i) => {
                self.active = i;
                cx.notify();
                Ok(i)
            }
            None => self.new_editor(Some(id), window, cx),
        }
    }

    /// MySQL quotes differently (backslash escapes, # comments).
    fn mysql_tab(&self, tab: usize) -> bool {
        self.tabs
            .get(tab)
            .and_then(|t| self.conn_ix(t.conn))
            .is_some_and(|i| self.conns[i].cfg.driver == Driver::Mysql)
    }

    fn conn_ix(&self, id: u64) -> Option<usize> {
        self.conns.iter().position(|c| c.id == id)
    }

    fn conn_by_name(&self, name: &str) -> Result<u64, String> {
        self.conns
            .iter()
            .find(|c| c.cfg.name.eq_ignore_ascii_case(name))
            .map(|c| c.id)
            .ok_or_else(|| {
                format!(
                    "no connection named {name:?}; have: {}",
                    self.conns
                        .iter()
                        .map(|c| c.cfg.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    fn save(&mut self) {
        let list: Vec<ConnConfig> = self.conns.iter().map(|c| c.cfg.clone()).collect();
        match db::save_connections(&list) {
            Ok(w) if !w.is_empty() => self.status = w.join("; "),
            Err(e) => self.status = format!("Could not save connections: {e}"),
            _ => {}
        }
    }

    // ---- background work -------------------------------------------------------------

    fn open_tab(
        &mut self,
        conn: u64,
        kind: TabKind,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.conn == conn && t.kind == kind && kind != TabKind::Sql)
        {
            self.active = i;
            cx.notify();
            return i;
        }
        let conn_name = self
            .conn_ix(conn)
            .map(|i| self.conns[i].cfg.name.clone())
            .unwrap_or_default();
        // one more than the highest open Script-N: closing Script-1 must not give two Script-2
        let n = self
            .tabs
            .iter()
            .filter_map(|t| {
                t.title
                    .strip_prefix("Script-")?
                    .split(' ')
                    .next()?
                    .parse()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        let title = match &kind {
            // Script name first: long titles are cut off in the tab bar.
            TabKind::Sql => format!("Script-{n} <{conn_name}>"),
            TabKind::Diagram => format!("ER: {conn_name}"),
            TabKind::Table(t) => t.clone(),
        };
        let catalog = self
            .conn_ix(conn)
            .map(|i| self.conns[i].catalog.clone())
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            let mut e = EditorState::new(window, cx)
                .language("sql")
                .default_value(sql);
            e.lsp_mut().completion_provider = Some(std::rc::Rc::new(SqlCompletion { catalog }));
            e
        });
        if kind == TabKind::Sql {
            cx.subscribe(&editor, |v, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    v.save_session_soon(cx);
                }
            })
            .detach();
        }
        let grid = cx.new(|cx| TableState::new(Grid::default(), window, cx).cell_selectable(true));
        let columns = cx.new(|cx| TableState::new(Grid::default(), window, cx));
        let ddl = cx.new(|cx| EditorState::new(window, cx).language("sql"));
        let text = cx.new(|cx| EditorState::new(window, cx));
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("WHERE …  (Enter applies)"));
        let edit_input = cx.new(|cx| InputState::new(window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search results…"));
        cx.subscribe(&search, |v, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change)
                && let Some(t) = v.tabs.iter().find(|t| t.search == input)
            {
                let text = input.read(cx).value().to_string();
                t.grid.update(cx, |g, cx| {
                    g.delegate_mut().set_search(&text);
                    cx.notify();
                });
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&grid, window, |v, grid, ev: &TableEvent, w, cx| {
            let Some(i) = v.tabs.iter().position(|t| t.grid == *grid) else {
                return;
            };
            match *ev {
                TableEvent::SelectCell(r, c) | TableEvent::RightClickedCell(r, c) => {
                    v.tabs[i].cell = Some((r, c))
                }
                TableEvent::DoubleClickedCell(r, c) => {
                    v.tabs[i].cell = Some((r, c));
                    v.active = i;
                    v.report(GridOp::EditCell, w, cx);
                }
                _ => {}
            }
        })
        .detach();
        cx.subscribe_in(&filter, window, |v, input, ev: &InputEvent, w, cx| {
            if matches!(ev, InputEvent::PressEnter { .. })
                && let Some(i) = v.tabs.iter().position(|t| t.filter == *input)
            {
                v.tabs[i].limit = PAGE;
                v.reload_table(i, w, cx, Box::new(|_, _, _, _| {}));
            }
        })
        .detach();
        cx.subscribe_in(&edit_input, window, |v, input, ev: &InputEvent, w, cx| {
            if matches!(ev, InputEvent::PressEnter { .. })
                && let Some(i) = v.tabs.iter().position(|t| t.edit_input == *input)
            {
                let value = input.read(cx).value().to_string();
                v.finish_edit(i, Some(value), w, cx);
            }
        })
        .detach();
        self.tabs.push(EditorTab {
            title,
            conn,
            kind,
            editor,
            grid,
            columns,
            view: TableView::Data,
            ddl,
            text,
            filter,
            cell: None,
            editing: None,
            edit_input,
            search,
            key: vec![],
            running: false,
            started: None,
            limit: PAGE,
            total: None,
            record: false,
            results: vec![],
            result_ix: 0,
            path: None,
            value_panel: false,
            chart: false,
            chart_col: None,
            text_view: false,
            stale: false,
            param_values: Default::default(),
            params_given: false,
            confirmed: false,
            agent: false,
            diagram: None,
            drag: None,
            message: None,
        });
        self.active = self.tabs.len() - 1;
        if self.tabs[self.active].kind == TabKind::Sql {
            self.tabs[self.active]
                .editor
                .update(cx, |e, cx| e.focus(window, cx));
        }
        self.save_session_soon(cx);
        cx.notify();
        self.active
    }

    fn open_table(
        &mut self,
        id: u64,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(ix) = self.conn_ix(id) else { return };
        let sql = self.conns[ix].cfg.driver.select_all(&table, "", 200);
        self.selected = Some((id, Some(table.clone())));
        let tab = self.open_tab(id, TabKind::Table(table.clone()), sql.clone(), window, cx);
        let key = self.tabs[tab].grid.entity_id();
        // Columns view (and the key for editing), DDL, then the data.
        let t = table.clone();
        self.with_session::<(QueryResult, String)>(
            id,
            move |s| {
                Ok::<_, String>((
                    s.columns(&t)?,
                    s.ddl(&t).unwrap_or_else(|e| format!("-- {e}")),
                ))
            },
            window,
            cx,
            Box::new(move |v, r, window, cx| {
                let Some(tab) = v.tab_ix(key) else {
                    return done(v, Err("the tab was closed".into()), window, cx);
                };
                if let (Ok((cols, ddl)), Some(t)) = (r, v.tabs.get_mut(tab)) {
                    t.key = cols
                        .rows
                        .iter()
                        .filter(|c| c[3] == "PK")
                        .map(|c| c[0].clone())
                        .collect();
                    t.ddl.update(cx, |e, cx| e.set_value(ddl, window, cx));
                    t.columns.update(cx, |g, cx| {
                        g.delegate_mut().set(cols);
                        g.refresh(cx);
                    });
                }
                v.reload_table(tab, window, cx, done);
            }),
        );
    }

    fn set_sidebar(
        &mut self,
        panel: Sidebar,
        filter: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar = panel;
        let hint = if panel == Sidebar::History {
            "Filter history…"
        } else {
            "Filter tables…"
        };
        self.side_filter.update(cx, |i, cx| {
            i.set_value(filter, window, cx);
            i.set_placeholder(hint, window, cx);
        });
        cx.notify();
    }

    fn close_tab(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.tabs.len() {
            self.save_session_soon(cx);
            self.tabs.remove(i);
            // the active tab stays active when one left of it closes
            if i < self.active || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
            cx.notify();
        }
    }

    fn new_editor(
        &mut self,
        conn: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let conn = conn
            .or(self.selected.as_ref().map(|s| s.0))
            .or(self.conns.first().map(|c| c.id))
            .ok_or("add a connection first")?;
        Ok(self.open_tab(conn, TabKind::Sql, String::new(), window, cx))
    }

    /// Put text on the clipboard; the copy button `id` shows a tick for a moment.
    fn copy_text(&mut self, id: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some((id, Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    /// A small button that copies `text`, e.g. an error message.
    fn copy_button(&self, id: &'static str, text: String, cx: &mut Context<Self>) -> Button {
        let done = self
            .copied
            .is_some_and(|(i, at)| i == id && at.elapsed() < COPIED_FOR);
        Button::new(id)
            .xsmall()
            .ghost()
            .icon(if done {
                IconName::Check
            } else {
                IconName::Copy
            })
            .tooltip(if done { "Copied" } else { "Copy to clipboard" })
            .on_click(cx.listener(move |v, _, _, cx| v.copy_text(id, text.clone(), cx)))
    }

    fn set_theme(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_system = false;
        self.apply_theme(dark, window, cx);
        self.save_theme(if dark { "dark" } else { "light" });
    }

    /// Follow the OS light / dark appearance from now on (also across restarts).
    fn follow_system_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_system = true;
        self.apply_theme(system_dark(window), window, cx);
        self.save_theme("system");
    }

    fn save_theme(&self, mode: &str) {
        let mut st = settings();
        st["theme"] = json!(mode);
        let _ = std::fs::write(settings_path(), serde_json::to_string_pretty(&st).unwrap());
    }

    fn apply_theme(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.dark = dark;
        Theme::change(
            if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            Some(window),
            cx,
        );
        show_scrollbars(cx);
        cx.notify();
    }
}

/// Scrollbars stay visible wherever there is more to scroll (the default fades them out).
fn show_scrollbars(cx: &mut App) {
    Theme::set_scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Always, cx);
}

fn system_dark(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

fn settings_path() -> std::path::PathBuf {
    session_path().with_file_name("settings.json")
}

/// UI preferences (`settings.json` next to the session file): currently the theme.
fn settings() -> Value {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn session_path() -> std::path::PathBuf {
    std::env::var_os("RUSTY_BEAVER_SESSION")
        .map(Into::into)
        .unwrap_or_else(|| db::config_dir().join("session.json"))
}

fn tables_json(t: &[(String, String)]) -> Value {
    json!({ "tables": t.iter().map(|(n, k)| json!({ "name": n, "type": k })).collect::<Vec<_>>() })
}

fn result_json(r: &QueryResult, max_rows: usize) -> Value {
    json!({
        "columns": r.columns,
        "rows": r.rows.iter().take(max_rows).collect::<Vec<_>>(),
        "row_count": r.rows.len(),
        "truncated": r.truncated || r.rows.len() > max_rows,
        "affected": r.affected,
        "elapsed_ms": r.elapsed.as_secs_f64() * 1000.,
    })
}

// ---- rendering -------------------------------------------------------------------------

impl AppView {
    /// Opacity of the splash: held, then faded out; 0 = gone.
    fn splash_alpha(&self) -> f32 {
        splash_alpha(self.started.elapsed())
    }

    fn splash(&self, alpha: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let progress = (self.started.elapsed().as_secs_f32() / SPLASH_HOLD.as_secs_f32()).min(1.);
        div()
            .id("splash")
            .absolute()
            .inset_0()
            .occlude()
            .opacity(alpha)
            .bg(rgb(0x1a1410))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .child(img(self.splash_image.clone()).size(px(256.)))
            .child(
                div()
                    .text_3xl()
                    .font_weight(FontWeight::BLACK)
                    .text_color(rgb(0xf2c46a))
                    .child("RUSTY BEAVER"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xc8b49a))
                    .child("Universal database tool · SQLite · PostgreSQL · MySQL"),
            )
            .child(
                div()
                    .w(px(260.))
                    .h(px(10.))
                    .border_2()
                    .border_color(rgb(0xf2c46a))
                    .child(div().h_full().w(relative(progress)).bg(rgb(0xe0702a))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8c7a64))
                    .child(if progress < 1. {
                        "Chewing through drivers…"
                    } else {
                        "Mission start!"
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    v.started = Instant::now() - SPLASH_HOLD - SPLASH_FADE;
                    cx.notify();
                }),
            )
    }
}

/// "1 row", "3 rows".
fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// A context-menu entry that runs `f` on the app view.
fn menu_item(
    view: &WeakEntity<AppView>,
    label: &str,
    f: impl Fn(&mut AppView, &mut Window, &mut Context<AppView>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label.to_string()).on_click(move |_, w, cx| {
        let _ = view.update(cx, |v, cx| {
            f(v, w, cx);
            cx.notify();
        });
    })
}

/// (keys, what) for the shortcut overlay; "Mod" is Cmd on macOS, Ctrl elsewhere.
const SHORTCUTS: &[(&str, &str)] = &[
    (
        "Mod+Enter",
        "Execute the statement at the cursor (or the selection)",
    ),
    ("Mod+Shift+Enter / Alt+X", "Execute the whole script"),
    ("Mod+.", "Cancel the running query"),
    ("Mod+Shift+E", "Explain plan"),
    ("Mod+Shift+F", "Format SQL"),
    ("Mod+T", "New SQL editor"),
    (
        "Mod+O / Mod+S / Mod+Shift+S",
        "Open / save / save script as",
    ),
    ("Mod+N", "New connection"),
    ("Mod+W", "Close tab"),
    ("Mod+S (table tab)", "Save grid changes"),
    ("Double-click / F2", "Edit cell (table tabs)"),
    ("Cmd+Backspace / Ctrl+Delete", "Delete row (table tabs)"),
    ("Mod+C / Mod+Shift+C", "Copy value / row"),
    ("Right-click", "Grid, table and connection menus"),
    ("Mod+/", "Comment / uncomment lines"),
    ("Cmd+1 … 9 / Alt+1 … 9", "Switch to tab 1 … 9"),
    ("F1 / Mod+Shift+/", "This overview"),
];

/// Segmented-control look: the chosen button is filled.
fn pick(b: Button, on: bool) -> Button {
    if on { b.primary() } else { b.outline() }
}

fn splash_alpha(elapsed: Duration) -> f32 {
    if elapsed <= SPLASH_HOLD {
        1.
    } else if elapsed >= SPLASH_HOLD + SPLASH_FADE {
        0.
    } else {
        1. - (elapsed - SPLASH_HOLD).as_secs_f32() / SPLASH_FADE.as_secs_f32()
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let alpha = self.splash_alpha();
        if alpha > 0. {
            window.request_animation_frame();
        }
        let t = cx.theme().clone();
        let dialog = self.dialog(window, cx);
        let params_dialog = self.params_dialog(cx);
        // "PG · PostgreSQL 17.11" for the active tab's connection.
        let active_conn = self
            .tabs
            .get(self.active)
            .and_then(|t| self.conn_ix(t.conn))
            .map(|i| {
                let c = &self.conns[i];
                match &c.server {
                    Some(v) => format!("{} · {v}", c.cfg.name),
                    None => c.cfg.name.clone(),
                }
            });
        let tabs = self.tabs.len();
        v_flex()
            .id("app")
            .key_context("RustyBeaver")
            .track_focus(&self.focus)
            .size_full()
            .bg(t.background)
            .text_color(t.foreground)
            .on_action(cx.listener(|v, _: &RunQuery, w, cx| {
                v.run(
                    v.active,
                    Source::Statement,
                    0,
                    w,
                    cx,
                    Box::new(|_, _, _, _| {}),
                )
            }))
            .on_action(cx.listener(|v, _: &RunScript, w, cx| {
                v.run(
                    v.active,
                    Source::Script,
                    0,
                    w,
                    cx,
                    Box::new(|_, _, _, _| {}),
                )
            }))
            .on_action(cx.listener(|v, _: &ExplainQuery, w, cx| {
                v.explain(v.active, None, w, cx, Box::new(|_, _, _, _| {}))
            }))
            .on_action(cx.listener(|v, _: &FormatSql, w, cx| {
                if let Err(e) = v.format(v.active, w, cx) {
                    v.status = e;
                }
            }))
            .on_action(cx.listener(|v, _: &CancelQuery, _, cx| {
                if let Err(e) = v.cancel(v.active, cx) {
                    v.status = e;
                }
            }))
            .on_action(cx.listener(|v, _: &NewEditor, w, cx| {
                if let Err(e) = v.new_editor(None, w, cx) {
                    v.status = e;
                }
            }))
            .on_action(cx.listener(|v, _: &NewConnection, w, cx| v.open_dialog(None, w, cx)))
            .on_action(cx.listener(|v, _: &CloseTab, _, cx| v.close_tab(v.active, cx)))
            .on_action(cx.listener(|v, _: &ToggleTheme, w, cx| v.set_theme(!v.dark, w, cx)))
            .on_action(cx.listener(|v, _: &CopyValue, w, cx| v.report(GridOp::CopyValue, w, cx)))
            .on_action(cx.listener(|v, _: &CopyRow, w, cx| v.report(GridOp::CopyRow, w, cx)))
            .on_action(cx.listener(|v, _: &CopyInsert, w, cx| v.report(GridOp::CopyInsert, w, cx)))
            .on_action(cx.listener(|v, _: &FilterByValue, w, cx| v.report(GridOp::FilterBy, w, cx)))
            .on_action(cx.listener(|v, _: &FilterExcludeValue, w, cx| {
                v.report(GridOp::FilterExclude, w, cx)
            }))
            .on_action(
                cx.listener(|v, _: &CopyAllMarkdown, w, cx| {
                    v.report(GridOp::CopyAllMarkdown, w, cx)
                }),
            )
            .on_action(cx.listener(|v, _: &EditCell, w, cx| v.report(GridOp::EditCell, w, cx)))
            .on_action(cx.listener(|v, _: &SetNull, w, cx| v.report(GridOp::SetNull, w, cx)))
            .on_action(cx.listener(|v, _: &AddRow, w, cx| v.report(GridOp::AddRow, w, cx)))
            .on_action(cx.listener(|v, _: &DeleteRow, w, cx| v.report(GridOp::DeleteRow, w, cx)))
            .on_action(cx.listener(|v, _: &SaveChanges, w, cx| {
                // Cmd/Ctrl+S: a script saves its file, a table tab its grid changes.
                match v.tabs.get(v.active) {
                    Some(t) if t.kind == TabKind::Sql && t.path.is_some() => {
                        let r = v.save_script(v.active, None, cx);
                        if let Err(e) = r {
                            v.status = e;
                        }
                    }
                    Some(t) if t.kind == TabKind::Sql => v.save_script_as(w, cx),
                    _ => v.report(GridOp::Save, w, cx),
                }
            }))
            .on_action(cx.listener(|v, _: &SaveScriptAs, w, cx| v.save_script_as(w, cx)))
            .on_action(cx.listener(|v, _: &ToggleComment, w, cx| v.toggle_comment(w, cx)))
            .on_action(
                cx.listener(|v, _: &CopyColumnInList, w, cx| v.report(GridOp::CopyColumnIn, w, cx)),
            )
            .on_action(cx.listener(|v, _: &FollowSystemTheme, w, cx| v.follow_system_theme(w, cx)))
            .on_action(cx.listener(|v, _: &CloseOtherTabs, _, cx| v.close_tabs(true, cx)))
            .on_action(cx.listener(|v, _: &CloseAllTabs, _, cx| v.close_tabs(false, cx)))
            .on_action(
                cx.listener(|v, _: &OpenReferencedRow, w, cx| v.report(GridOp::FollowFk, w, cx)),
            )
            .on_action(
                cx.listener(|v, _: &DuplicateRowAction, w, cx| {
                    v.report(GridOp::DuplicateRow, w, cx)
                }),
            )
            .on_action(cx.listener(|v, _: &Tab1, w, cx| v.select_tab_ix(0, w, cx)))
            .on_action(cx.listener(|v, _: &Tab2, w, cx| v.select_tab_ix(1, w, cx)))
            .on_action(cx.listener(|v, _: &Tab3, w, cx| v.select_tab_ix(2, w, cx)))
            .on_action(cx.listener(|v, _: &Tab4, w, cx| v.select_tab_ix(3, w, cx)))
            .on_action(cx.listener(|v, _: &Tab5, w, cx| v.select_tab_ix(4, w, cx)))
            .on_action(cx.listener(|v, _: &Tab6, w, cx| v.select_tab_ix(5, w, cx)))
            .on_action(cx.listener(|v, _: &Tab7, w, cx| v.select_tab_ix(6, w, cx)))
            .on_action(cx.listener(|v, _: &Tab8, w, cx| v.select_tab_ix(7, w, cx)))
            .on_action(cx.listener(|v, _: &Tab9, w, cx| v.select_tab_ix(8, w, cx)))
            .on_action(cx.listener(|v, _: &ShowShortcuts, _, cx| {
                v.shortcuts = !v.shortcuts;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &OpenScript, w, cx| {
                let rx = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Open".into()),
                });
                cx.spawn_in(w, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = rx.await {
                        let _ = this.update_in(cx, |v, w, cx| {
                            for p in paths {
                                if let Err(e) = v.open_script(&p, w, cx) {
                                    v.status = e;
                                }
                            }
                        });
                    }
                })
                .detach();
            }))
            .on_action(cx.listener(|v, _: &RevertChanges, w, cx| v.report(GridOp::Revert, w, cx)))
            .child(self.toolbar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main-split")
                        .child(
                            resizable_panel()
                                .size(px(290.))
                                .size_range(px(180.)..px(600.))
                                .child(self.navigator(cx)),
                        )
                        .child(resizable_panel().child(self.editor_area(window, cx))),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.title_bar)
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .when(!self.status.is_empty(), |d| {
                        d.child(self.copy_button("copy-status", self.status.clone(), cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.status.clone()),
                    )
                    .children(active_conn)
                    .child(
                        h_flex()
                            .gap_1()
                            .when_some(self.mcp.strip_prefix("MCP: "), |d, url| {
                                d.child(self.copy_button("copy-mcp", url.to_string(), cx))
                            })
                            .child(self.mcp.clone()),
                    )
                    .child(format!(
                        "{} connection(s) · {tabs} tab(s)",
                        self.conns.len()
                    )),
            )
            .children(dialog)
            .children(params_dialog)
            .when(self.shortcuts, |d| {
                let modkey = if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Ctrl"
                };
                d.child(
                    div()
                        .id("shortcuts-backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(gpui_kit::black().opacity(0.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.shortcuts = false;
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("shortcuts")
                                .w(px(560.))
                                .h_auto()
                                .max_h(relative(0.9))
                                .overflow_y_scrollbar()
                                .gap_1()
                                .p_5()
                                .rounded_lg()
                                .border_1()
                                .border_color(t.border)
                                .bg(t.background)
                                .shadow_lg()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::BOLD)
                                        .mb_2()
                                        .child("Keyboard shortcuts"),
                                )
                                .children(SHORTCUTS.iter().map(|(keys, what)| {
                                    h_flex()
                                        .gap_3()
                                        .text_sm()
                                        .child(
                                            div()
                                                .w(px(230.))
                                                .flex_shrink_0()
                                                .font_family(t.mono_font_family.clone())
                                                .text_color(Hsla::from(BRAND))
                                                .child(keys.replace("Mod", modkey)),
                                        )
                                        .child(div().flex_1().min_w_0().child(*what))
                                }))
                                .child(
                                    div()
                                        .mt_2()
                                        .text_xs()
                                        .text_color(t.muted_foreground)
                                        .child("Click anywhere to close"),
                                ),
                        ),
                )
            })
            .when(alpha > 0., |d| d.child(self.splash(alpha, cx)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn splash_holds_then_fades() {
        use super::*;
        assert_eq!(splash_alpha(Duration::ZERO), 1.);
        assert_eq!(splash_alpha(SPLASH_HOLD), 1.);
        let mid = splash_alpha(SPLASH_HOLD + SPLASH_FADE / 2);
        assert!(mid > 0.4 && mid < 0.6);
        assert_eq!(splash_alpha(SPLASH_HOLD + SPLASH_FADE), 0.);
    }
}
