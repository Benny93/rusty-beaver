//! MCP server (streamable HTTP on 127.0.0.1) so agents can drive the running app.
//! Each tool is a semantic UI command (no simulated mouse): it is handed to the UI thread,
//! which changes what the user sees and replies once the work it started has finished.

use base64::Engine;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

pub const DEFAULT_PORT: u16 = 7979;

/// A tool call for the UI thread: the command name, its arguments and where to reply.
pub struct Call {
    pub name: &'static str,
    pub args: Value,
    pub reply: oneshot::Sender<Result<Value, String>>,
}

#[derive(Clone)]
pub struct Tools {
    tx: mpsc::UnboundedSender<Call>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConnArg {
    /// Connection name as shown in the Database Navigator.
    pub connection: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct TableArg {
    /// Connection name as shown in the Database Navigator.
    pub connection: String,
    /// Table or view name as listed by list_tables (Postgres: schema.table).
    pub table: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct OptConnArg {
    /// Connection for the new editor; defaults to the selected one.
    pub connection: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SqlArg {
    /// Run in this connection's SQL editor (reused or opened). Omit for the active tab.
    pub connection: Option<String>,
    /// SQL text. Omit to run what is already in the active editor.
    pub sql: Option<String>,
    /// Max rows returned to you (the grid still shows everything), default 50.
    pub max_rows: Option<usize>,
    /// Values for `:name` parameters in the SQL, inserted as typed (quote strings: "'Private'").
    pub params: Option<std::collections::BTreeMap<String, String>>,
    /// Required (true) to run data-changing SQL on a production connection.
    pub confirm: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TextArg {
    /// New editor content.
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct IndexArg {
    /// 0-based tab index as listed by get_state.
    pub index: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct CancelArg {
    /// 0-based tab index as listed by get_state; default: the query that has run longest.
    pub index: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ViewArg {
    /// "data", "columns" or "ddl" (table tabs only).
    pub view: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LimitArg {
    /// A script produces one result per statement: show and return this one (0-based).
    pub index: Option<usize>,
    /// Max rows, default 50.
    pub max_rows: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PathArg {
    /// Absolute output path; .csv, .tsv, .json, .md or .sql.
    pub path: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct OptPathArg {
    /// Absolute path; omit to use the tab's own file.
    pub path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ThemeArg {
    /// "dark", "light" or "system" (follow the OS appearance).
    pub mode: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct FilterArg {
    /// WHERE condition for the active table tab, e.g. `rank = 'Private'`; empty clears it.
    pub filter: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CellArg {
    /// 0-based row as currently shown in the grid (after sorting).
    pub row: usize,
    /// Column name.
    pub column: String,
    /// New value; omit (or null) to set SQL NULL.
    pub value: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ImportArg {
    pub connection: String,
    pub table: String,
    /// Absolute path of the CSV file.
    pub path: String,
    /// Required (true) on production connections.
    pub confirm: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AutoCommitArg {
    pub connection: String,
    /// true: auto-commit (default); false: manual commit.
    pub on: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConfirmArg {
    /// Required (true) on production connections.
    pub confirm: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CellRefArg {
    /// 0-based row as shown in the grid.
    pub row: usize,
    /// The foreign-key column.
    pub column: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct KeepArg {
    /// Keep the active tab open.
    pub keep_active: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExecArg {
    /// Run the whole script instead of the statement at the cursor.
    pub script: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ChartArg {
    /// Show the chart (default true) or the grid again.
    pub on: Option<bool>,
    /// Numeric column to plot; default the first.
    pub column: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct OnArg {
    /// Show it (default true), or false to go back.
    pub on: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct OpenArg {
    /// true opens it, false closes it.
    pub open: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ValueArg {
    /// 0-based row as currently shown in the grid.
    pub row: Option<usize>,
    /// Column name.
    pub column: Option<String>,
    /// false closes the value panel.
    pub on: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RowArg {
    /// 0-based row as currently shown in the grid.
    pub row: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddRowArg {
    /// Column name -> value for the new row; columns left out stay NULL / default.
    pub values: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SortArg {
    /// Column name.
    pub column: String,
    /// Sort descending (default ascending).
    pub descending: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CompleteArg {
    /// Connection whose tables and columns to complete.
    pub connection: String,
    /// SQL text.
    pub sql: String,
    /// Cursor position in bytes; default: end of text.
    pub offset: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct HistoryArg {
    /// Only entries whose SQL or connection contains this text.
    pub search: Option<String>,
    /// Newest first, default 20.
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SidebarArg {
    /// "navigator" or "history".
    pub panel: String,
    /// Text for the sidebar's filter box (tables or history); empty clears it.
    pub filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct EditConnArg {
    /// Current connection name.
    pub connection: String,
    /// Fields to change; the others stay.
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    /// Mark as production (red, writes need confirm) or not.
    pub production: Option<bool>,
    /// Read-only: refuse statements that change data or schema.
    pub read_only: Option<bool>,
    /// How credentials are obtained: password (kept in the OS keychain), vault_kv (user +
    /// password from a field of a Vault KV secret) or vault_dynamic (Vault database secrets engine).
    pub auth: Option<String>,
    /// TLS (PostgreSQL, MySQL): prefer (default: TLS if offered), disable, require, or verify_full
    /// (certificate checked against the OS trust store).
    pub ssl: Option<String>,
    /// vault_kv: the secret path (secret/data/team/db); vault_dynamic: database/creds/<role>.
    pub vault_path: Option<String>,
    /// vault_kv: the field holding the password (default "password").
    pub vault_field: Option<String>,
    /// Vault address (default VAULT_ADDR) / OIDC role / OIDC auth mount (default oidc).
    pub vault_addr: Option<String>,
    pub vault_role: Option<String>,
    pub vault_mount: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GenerateArg {
    pub connection: String,
    pub table: String,
    /// select | insert | update | delete | count
    pub kind: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct OptSqlArg {
    /// SQL to use; omit for the active editor (explain: the statement at the cursor).
    pub sql: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RecordArg {
    /// Show (true) or hide (false) the record view; omit to toggle.
    pub on: Option<bool>,
    /// 0-based row to show.
    pub row: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DialogArg {
    /// true opens the dialog, false closes it.
    pub open: bool,
    /// Open the Edit Connection dialog for this connection instead of New Connection.
    pub connection: Option<String>,
}

impl Tools {
    async fn ui(&self, name: &'static str, args: Value) -> Result<CallToolResult, McpError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Call { name, args, reply })
            .map_err(|_| McpError::internal_error("app closed", None))?;
        match rx.await {
            Ok(Ok(v)) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&v).unwrap_or_default(),
            )])),
            Ok(Err(e)) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
            Err(_) => Err(McpError::internal_error("app closed", None)),
        }
    }
}

#[tool_router]
impl Tools {
    #[tool(
        description = "Everything visible: connections (status, expanded), open tabs (title, kind, connection, editor SQL, result summary), active tab, selection, theme, dialogs, status bar."
    )]
    async fn get_state(&self) -> Result<CallToolResult, McpError> {
        self.ui("get_state", json!({})).await
    }

    #[tool(description = "PNG screenshot of the app window, exactly as the user sees it.")]
    async fn screenshot(&self) -> Result<CallToolResult, McpError> {
        // Bring the window to the front (a covered window does not repaint), then let it paint.
        let _ = self.ui("reveal", json!({})).await;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let png = tokio::task::spawn_blocking(capture)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let png = png.map_err(|e| McpError::internal_error(e, None))?;
        let mut content = vec![ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(png),
            "image/png",
        )];
        if screen_locked() {
            content.push(ContentBlock::text("warning: the screen is locked, so the window cannot repaint; this image may show an older frame"));
        }
        Ok(CallToolResult::success(content))
    }

    #[tool(
        description = "Add (and save) a connection to the navigator. Fields: name, driver (sqlite|postgres|mysql), host, port, database (file path for sqlite), user, password."
    )]
    async fn add_connection(
        &self,
        Parameters(c): Parameters<crate::db::ConnConfig>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("add_connection", serde_json::to_value(c).unwrap())
            .await
    }

    #[tool(description = "Remove a saved connection (closes its tabs).")]
    async fn delete_connection(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("delete_connection", json!({ "connection": a.connection }))
            .await
    }

    #[tool(
        description = "Connect and expand the connection in the navigator; returns its tables and views."
    )]
    async fn connect(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("connect", json!({ "connection": a.connection }))
            .await
    }

    #[tool(description = "Disconnect a connection.")]
    async fn disconnect(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("disconnect", json!({ "connection": a.connection }))
            .await
    }

    #[tool(description = "Tables and views of a connection (connects if needed).")]
    async fn list_tables(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("connect", json!({ "connection": a.connection }))
            .await
    }

    #[tool(
        description = "Expand a table in the navigator; returns its columns (name, type, nullable, key)."
    )]
    async fn describe_table(
        &self,
        Parameters(a): Parameters<TableArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "describe_table",
            json!({ "connection": a.connection, "table": a.table }),
        )
        .await
    }

    #[tool(
        description = "Open a table's data editor tab (like double-clicking it in DBeaver); returns the first rows."
    )]
    async fn open_table(
        &self,
        Parameters(a): Parameters<TableArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "open_table",
            json!({ "connection": a.connection, "table": a.table }),
        )
        .await
    }

    #[tool(description = "Open a new SQL editor tab.")]
    async fn new_sql_editor(
        &self,
        Parameters(a): Parameters<OptConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("new_sql_editor", json!({ "connection": a.connection }))
            .await
    }

    #[tool(description = "Replace the text of the active tab's SQL editor.")]
    async fn set_sql(
        &self,
        Parameters(a): Parameters<TextArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("set_sql", json!({ "text": a.text })).await
    }

    #[tool(
        description = "Execute SQL in the active tab, or in the SQL editor of `connection` (opens one when needed), shows the result grid, returns columns + rows."
    )]
    async fn run_sql(&self, Parameters(a): Parameters<SqlArg>) -> Result<CallToolResult, McpError> {
        self.ui(
            "run_sql",
            json!({ "sql": a.sql, "max_rows": a.max_rows, "connection": a.connection, "params": a.params, "confirm": a.confirm }),
        )
        .await
    }

    #[tool(description = "The result grid of the active tab.")]
    async fn get_results(
        &self,
        Parameters(a): Parameters<LimitArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "get_results",
            json!({ "max_rows": a.max_rows, "index": a.index }),
        )
        .await
    }

    #[tool(description = "Activate a tab.")]
    async fn select_tab(
        &self,
        Parameters(a): Parameters<IndexArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("select_tab", json!({ "index": a.index })).await
    }

    #[tool(description = "Close a tab.")]
    async fn close_tab(
        &self,
        Parameters(a): Parameters<IndexArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("close_tab", json!({ "index": a.index })).await
    }

    #[tool(
        description = "Switch the active table tab between its Data grid, Columns (properties) and DDL views; returns the DDL for \"ddl\"."
    )]
    async fn set_table_view(
        &self,
        Parameters(a): Parameters<ViewArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("set_table_view", json!({ "view": a.view })).await
    }

    #[tool(description = "Filter the active table tab with a WHERE condition and reload it.")]
    async fn set_filter(
        &self,
        Parameters(a): Parameters<FilterArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("set_filter", json!({ "filter": a.filter })).await
    }

    #[tool(
        description = "Change a cell in the active table tab's grid (pending, highlighted until save_changes)."
    )]
    async fn edit_cell(
        &self,
        Parameters(a): Parameters<CellArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "edit_cell",
            json!({ "row": a.row, "column": a.column, "value": a.value }),
        )
        .await
    }

    #[tool(description = "Add a row to the active table tab's grid (pending until save_changes).")]
    async fn add_row(
        &self,
        Parameters(a): Parameters<AddRowArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("add_row", json!({ "values": a.values })).await
    }

    #[tool(
        description = "Mark a row of the active table tab for deletion (again: unmark); applied by save_changes."
    )]
    async fn delete_row(
        &self,
        Parameters(a): Parameters<RowArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("delete_row", json!({ "row": a.row })).await
    }

    #[tool(
        description = "Save the pending grid changes of the active table tab in one transaction (INSERT/UPDATE/DELETE by primary key), then reload."
    )]
    async fn save_changes(
        &self,
        Parameters(a): Parameters<ConfirmArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("save_changes", json!({ "confirm": a.confirm }))
            .await
    }

    #[tool(description = "Drop the pending grid changes of the active tab.")]
    async fn revert_changes(&self) -> Result<CallToolResult, McpError> {
        self.ui("revert_changes", json!({})).await
    }

    #[tool(description = "Sort the active result grid by a column (client side).")]
    async fn sort_results(
        &self,
        Parameters(a): Parameters<SortArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "sort_results",
            json!({ "column": a.column, "descending": a.descending.unwrap_or(false) }),
        )
        .await
    }

    #[tool(
        description = "What the SQL editor's autocompletion offers at a cursor position (keywords, tables, columns; alias-aware after `x.`)."
    )]
    async fn sql_completions(
        &self,
        Parameters(a): Parameters<CompleteArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "sql_completions",
            json!({ "connection": a.connection, "sql": a.sql, "offset": a.offset }),
        )
        .await
    }

    #[tool(
        description = "Executed statements (persisted across restarts), newest first, with connection, age, duration and outcome."
    )]
    async fn get_history(
        &self,
        Parameters(a): Parameters<HistoryArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "get_history",
            json!({ "search": a.search, "limit": a.limit }),
        )
        .await
    }

    #[tool(
        description = "Show the Database Navigator or the query History in the sidebar, optionally filtered."
    )]
    async fn set_sidebar(
        &self,
        Parameters(a): Parameters<SidebarArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "set_sidebar",
            json!({ "panel": a.panel, "filter": a.filter.unwrap_or_default() }),
        )
        .await
    }

    #[tool(
        description = "Cancel a running query (default: the one that has run longest). The run_sql call waiting for it returns the cancellation error."
    )]
    async fn cancel_query(
        &self,
        Parameters(a): Parameters<CancelArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("cancel_query", json!({ "index": a.index })).await
    }

    #[tool(
        description = "Change a saved connection (rename, host, credentials, …); it reconnects on next use when connection settings changed."
    )]
    async fn edit_connection(
        &self,
        Parameters(a): Parameters<EditConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("edit_connection", json!({ "connection": a.connection, "name": a.name, "host": a.host, "port": a.port, "database": a.database, "user": a.user, "password": a.password, "production": a.production, "read_only": a.read_only, "auth": a.auth, "ssl": a.ssl, "vault_path": a.vault_path, "vault_field": a.vault_field, "vault_addr": a.vault_addr, "vault_role": a.vault_role, "vault_mount": a.vault_mount })).await
    }

    #[tool(
        description = "Reload a connection's table list in the navigator (after schema changes made elsewhere)."
    )]
    async fn refresh_connection(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("refresh_connection", json!({ "connection": a.connection }))
            .await
    }

    #[tool(
        description = "Generate a SELECT / INSERT / UPDATE / DELETE / count statement for a table (like DBeaver's Generate SQL) and open it in a new SQL editor."
    )]
    async fn generate_sql(
        &self,
        Parameters(a): Parameters<GenerateArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "generate_sql",
            json!({ "connection": a.connection, "table": a.table, "kind": a.kind }),
        )
        .await
    }

    #[tool(
        description = "Fetch the next 200 rows of the active table tab (table tabs load 200 at a time; get_state shows total_rows)."
    )]
    async fn load_more(&self) -> Result<CallToolResult, McpError> {
        self.ui("load_more", json!({})).await
    }

    #[tool(
        description = "Open the ER diagram of a connection (tables with columns, keys and foreign-key links; boxes can be dragged); returns tables and relations."
    )]
    async fn open_er_diagram(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("open_er_diagram", json!({ "connection": a.connection }))
            .await
    }

    #[tool(description = "Open a .sql file in a new SQL editor tab (on the selected connection).")]
    async fn open_script(
        &self,
        Parameters(a): Parameters<PathArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("open_script", json!({ "path": a.path })).await
    }

    #[tool(
        description = "Save the active SQL editor to a file (path omitted: the file it came from)."
    )]
    async fn save_script(
        &self,
        Parameters(a): Parameters<OptPathArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("save_script", json!({ "path": a.path })).await
    }

    #[tool(
        description = "Select a cell of the active grid and show it in the value panel (full text, JSON pretty-printed); returns the value and its column's statistics (rows, NULL, distinct, min, max, sum, avg)."
    )]
    async fn show_value(
        &self,
        Parameters(a): Parameters<ValueArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "show_value",
            json!({ "row": a.row, "column": a.column, "on": a.on }),
        )
        .await
    }

    #[tool(
        description = "Import a CSV file into a table: the header row names the columns, empty fields become NULL; all rows in one transaction (nothing is written on error)."
    )]
    async fn import_csv(
        &self,
        Parameters(a): Parameters<ImportArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "import_csv",
            json!({ "connection": a.connection, "table": a.table, "path": a.path, "confirm": a.confirm }),
        )
        .await
    }

    #[tool(
        description = "Turn auto-commit on or off for a connection. Off (manual commit): statements and grid saves run in a transaction that stays open until commit / rollback."
    )]
    async fn set_auto_commit(
        &self,
        Parameters(a): Parameters<AutoCommitArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "set_auto_commit",
            json!({ "connection": a.connection, "on": a.on }),
        )
        .await
    }

    #[tool(description = "Commit the open transaction of a manual-commit connection.")]
    async fn commit(&self, Parameters(a): Parameters<ConnArg>) -> Result<CallToolResult, McpError> {
        self.ui("commit", json!({ "connection": a.connection }))
            .await
    }

    #[tool(
        description = "Roll back the open transaction of a manual-commit connection (open table tabs reload)."
    )]
    async fn rollback(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("rollback", json!({ "connection": a.connection }))
            .await
    }

    #[tool(
        description = "Show the active result as a bar chart (a text column labels bars, a numeric column sizes them; pick it with column) or back as a grid (on: false)."
    )]
    async fn chart(&self, Parameters(a): Parameters<ChartArg>) -> Result<CallToolResult, McpError> {
        self.ui("chart", json!({ "on": a.on, "column": a.column }))
            .await
    }

    #[tool(
        description = "Record view: show one row of the active grid as a field/value form; returns that row."
    )]
    async fn record_view(
        &self,
        Parameters(a): Parameters<RecordArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("record_view", json!({ "on": a.on, "row": a.row }))
            .await
    }

    #[tool(
        description = "Show the execution plan (EXPLAIN / EXPLAIN QUERY PLAN) of a statement in the active tab's grid and return it."
    )]
    async fn explain(
        &self,
        Parameters(a): Parameters<OptSqlArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("explain", json!({ "sql": a.sql })).await
    }

    #[tool(
        description = "Format the active editor's SQL (keywords upper-case, one clause per line); with sql, put that in the editor first. Returns the formatted text."
    )]
    async fn format_sql(
        &self,
        Parameters(a): Parameters<OptSqlArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("format_sql", json!({ "sql": a.sql })).await
    }

    #[tool(
        description = "Export the active result grid (as shown: sorted, with pending edits) to a file; the extension picks the format: .csv, .tsv, .json, .md or .sql (INSERT statements)."
    )]
    async fn export_csv(
        &self,
        Parameters(a): Parameters<PathArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("export_csv", json!({ "path": a.path })).await
    }

    #[tool(description = "Switch the color theme.")]
    async fn set_theme(
        &self,
        Parameters(a): Parameters<ThemeArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("set_theme", json!({ "mode": a.mode })).await
    }

    #[tool(description = "Open or close the keyboard shortcut overview (F1).")]
    async fn shortcuts(
        &self,
        Parameters(a): Parameters<OpenArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("shortcuts", json!({ "open": a.open })).await
    }

    #[tool(
        description = "Open or close the New Connection dialog (to show it to the user; use add_connection to create one)."
    )]
    async fn connection_dialog(
        &self,
        Parameters(a): Parameters<DialogArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "connection_dialog",
            json!({ "open": a.open, "connection": a.connection }),
        )
        .await
    }

    #[tool(
        description = "Execute the active editor like the user pressing Cmd/Ctrl+Enter (statement at the cursor) or, with script, the whole script. :name parameters open the parameter dialog for the user. Returns at once; poll get_state / get_results."
    )]
    async fn execute_editor(
        &self,
        Parameters(a): Parameters<ExecArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("execute_editor", json!({ "script": a.script }))
            .await
    }

    #[tool(
        description = "Quick search in the active result grid: only rows with a cell containing the text stay visible (empty text shows all). Returns the visible rows."
    )]
    async fn search_results(
        &self,
        Parameters(a): Parameters<TextArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("search_results", json!({ "text": a.text })).await
    }

    #[tool(
        description = "Show the active result as aligned plain text (DBeaver's Text view) and return that text; on: false goes back to the grid."
    )]
    async fn results_as_text(
        &self,
        Parameters(a): Parameters<OnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("results_as_text", json!({ "on": a.on })).await
    }

    #[tool(
        description = "Copy a row of the active table tab into a new pending row (key columns left empty); save_changes inserts it."
    )]
    async fn duplicate_row(
        &self,
        Parameters(a): Parameters<RowArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("duplicate_row", json!({ "row": a.row })).await
    }

    #[tool(
        description = "Follow a foreign key: open the table a cell of the active table tab references, filtered to that row."
    )]
    async fn open_referenced_row(
        &self,
        Parameters(a): Parameters<CellRefArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui(
            "open_referenced_row",
            json!({ "row": a.row, "column": a.column }),
        )
        .await
    }

    #[tool(
        description = "Try connection settings without saving them (same fields as add_connection); returns the server version and table count, or the error."
    )]
    async fn test_connection(
        &self,
        Parameters(c): Parameters<crate::db::ConnConfig>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("test_connection", serde_json::to_value(c).unwrap())
            .await
    }

    #[tool(
        description = "Count the rows of the tables of a connection, up to 200 tables (opens a SQL tab with one UNION ALL query and runs it); returns table_name, row_count, biggest first."
    )]
    async fn row_counts(
        &self,
        Parameters(a): Parameters<ConnArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("row_counts", json!({ "connection": a.connection }))
            .await
    }

    #[tool(description = "Close all tabs, or (keep_active) all but the active one.")]
    async fn close_all_tabs(
        &self,
        Parameters(a): Parameters<KeepArg>,
    ) -> Result<CallToolResult, McpError> {
        self.ui("close_all_tabs", json!({ "keep_active": a.keep_active }))
            .await
    }
}

#[tool_handler]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        let mut app = Implementation::from_build_env();
        app.name = "rusty-beaver".into();
        app.title = Some("Rusty Beaver".into());
        app.version = env!("CARGO_PKG_VERSION").into();
        let mut info = ServerConfig::default();
        info.server_info = app;
        info.instructions = Some(
            "Drives the Rusty Beaver database tool (a DBeaver clone) the user is looking at; every tool changes what they see. \
             Start with get_state, then connect / describe_table / open_table / run_sql; screenshot shows the window. \
             Table tabs edit data: edit_cell / add_row / delete_row, then save_changes (one transaction). \
             Connections marked production need confirm: true for data-changing SQL and saves; read-only ones refuse them. \
             Manual-commit connections keep a transaction open until commit / rollback."
                .into(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// macOS stops drawing windows while the session is locked.
fn screen_locked() -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("ioreg")
            .args(["-n", "Root", "-d1", "-a"])
            .output()
            .is_ok_and(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split("CGSSessionScreenIsLocked</key>")
                    .nth(1)
                    .is_some_and(|rest| rest.trim_start().starts_with("<true/>"))
            })
}

/// PNG of this process's window: xcap on every platform.
fn capture() -> Result<Vec<u8>, String> {
    let pid = std::process::id();
    let window = xcap::Window::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.pid().ok() == Some(pid) && !w.is_minimized().unwrap_or(true))
        .ok_or("app window not found")?;
    let image = window.capture_image().map_err(|e| e.to_string())?;
    let mut png = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            xcap::image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(png)
}

/// Rejects requests from web pages (DNS rebinding): only local Host/Origin headers pass.
async fn local_only(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    let is_local = |v: &str| {
        let host = v.split("://").last().unwrap_or(v);
        ["localhost", "127.0.0.1", "[::1]"]
            .iter()
            .any(|h| host == *h || host.starts_with(&format!("{h}:")))
    };
    let ok = ["host", "origin"].iter().all(|n| {
        req.headers()
            .get(*n)
            .and_then(|v| v.to_str().ok())
            .is_none_or(is_local)
    });
    if ok {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Serve MCP at http://127.0.0.1:<port>/mcp on its own thread; calls arrive on the receiver.
pub fn spawn(
    port: u16,
) -> (
    mpsc::UnboundedReceiver<Call>,
    std::sync::mpsc::Receiver<Result<(), String>>,
) {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let (status_tx, status) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let tools = Tools { tx };
            let config = StreamableHttpServerConfig::default().with_allowed_origins([
                "http://localhost:*",
                "http://127.0.0.1:*",
                "http://[::1]:*",
            ]);
            let service = StreamableHttpService::new(
                move || Ok(tools.clone()),
                LocalSessionManager::default().into(),
                config,
            );
            let router = axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(local_only));
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => {
                    let _ = status_tx.send(Ok(()));
                    if let Err(e) = axum::serve(listener, router).await {
                        eprintln!("mcp: {e}");
                    }
                }
                Err(e) => {
                    let _ = status_tx.send(Err(format!("port {port}: {e}")));
                }
            }
        });
    });
    (rx, status)
}
