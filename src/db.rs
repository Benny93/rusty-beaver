//! Database drivers (SQLite, PostgreSQL, MySQL) behind one tiny text-only API.
//! Every value comes back as display text, so the grid, CSV export and MCP all share one shape.

use crate::secrets;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    #[default]
    Sqlite,
    Postgres,
    Mysql,
}

impl Driver {
    pub const ALL: [Driver; 3] = [Driver::Sqlite, Driver::Postgres, Driver::Mysql];

    pub fn label(self) -> &'static str {
        match self {
            Driver::Sqlite => "SQLite",
            Driver::Postgres => "PostgreSQL",
            Driver::Mysql => "MySQL",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Sqlite => 0,
            Driver::Postgres => 5432,
            Driver::Mysql => 3306,
        }
    }

    /// A string as an SQL literal. MySQL also treats backslashes as escapes.
    /// Servers coerce '42' to numbers and dates.
    pub fn literal(self, v: &str) -> String {
        let v = v.replace('\'', "''");
        match self {
            Driver::Mysql => format!("'{}'", v.replace('\\', "\\\\")),
            _ => format!("'{v}'"),
        }
    }

    /// `literal`, with None as NULL.
    pub fn literal_opt(self, v: Option<&str>) -> String {
        v.map(|v| self.literal(v)).unwrap_or_else(|| "NULL".into())
    }

    /// An identifier quoted for this dialect.
    pub fn quote(self, ident: &str) -> String {
        match self {
            Driver::Mysql => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// A table name as SQL; Postgres names come back schema-qualified ("public.users").
    pub fn table_ref(self, table: &str) -> String {
        match (self, table.split_once('.')) {
            (Driver::Postgres, Some((schema, t))) => {
                format!("{}.{}", self.quote(schema), self.quote(t))
            }
            _ => self.quote(table),
        }
    }

    /// `SELECT * FROM <table> [WHERE <filter>] LIMIT n`, as the table editor runs it.
    pub fn select_all(self, table: &str, filter: &str, limit: usize) -> String {
        // a filter that is only "ORDER BY y" sorts: no WHERE before it
        let (cond, order) = split_order(filter.trim());
        let filter = match (cond.trim(), order.trim()) {
            ("", "") => String::new(),
            ("", o) => format!(" {o}"),
            (c, "") => format!(" WHERE {c}"),
            (c, o) => format!(" WHERE {c} {o}"),
        };
        format!(
            "SELECT * FROM {}{filter} LIMIT {limit};",
            self.table_ref(table)
        )
    }

    /// Statement returning a table's DDL in its first row, second or only column (where the
    /// server has one).
    fn ddl_sql(self, table: &str) -> Option<String> {
        match self {
            Driver::Sqlite => Some(format!(
                "SELECT sql FROM sqlite_master WHERE name = {}",
                self.literal(table)
            )),
            Driver::Mysql => Some(format!("SHOW CREATE TABLE {}", self.quote(table))),
            Driver::Postgres => None,
        }
    }

    /// Query listing tables and views: rows of (name, kind).
    fn tables_sql(self) -> &'static str {
        match self {
            Driver::Sqlite => {
                "SELECT name, upper(type) FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name"
            }
            Driver::Postgres => {
                "SELECT table_schema || '.' || table_name, CASE table_type WHEN 'BASE TABLE' THEN 'TABLE' ELSE table_type END FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY 1"
            }
            Driver::Mysql => {
                "SELECT table_name, CASE table_type WHEN 'BASE TABLE' THEN 'TABLE' ELSE table_type END FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY 1"
            }
        }
    }

    /// Every foreign key of the schema: rows of (table, column, referenced table, referenced column).
    fn foreign_keys_sql(self) -> &'static str {
        match self {
            Driver::Sqlite => {
                // REFERENCES t without columns means t's primary key
                "SELECT m.name, p.\"from\", p.\"table\", coalesce(p.\"to\", (SELECT k.name FROM pragma_table_info(p.\"table\") k WHERE k.pk = p.seq + 1), '') FROM sqlite_master m JOIN pragma_foreign_key_list(m.name) p WHERE m.type = 'table' ORDER BY 1"
            }
            Driver::Postgres => {
                // pg_constraint pairs the columns of composite keys, and constraint names are
                // only unique per table
                "SELECT cn.nspname || '.' || cr.relname, a.attname, fn.nspname || '.' || fr.relname, af.attname \
                FROM pg_constraint c CROSS JOIN LATERAL unnest(c.conkey, c.confkey) AS k(att, fatt) \
                JOIN pg_class cr ON cr.oid = c.conrelid JOIN pg_namespace cn ON cn.oid = cr.relnamespace \
                JOIN pg_class fr ON fr.oid = c.confrelid JOIN pg_namespace fn ON fn.oid = fr.relnamespace \
                JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.att \
                JOIN pg_attribute af ON af.attrelid = c.confrelid AND af.attnum = k.fatt \
                WHERE c.contype = 'f' AND cn.nspname NOT IN ('pg_catalog','information_schema') ORDER BY 1"
            }
            Driver::Mysql => {
                "SELECT table_name, column_name, referenced_table_name, referenced_column_name FROM information_schema.key_column_usage \
                WHERE table_schema = DATABASE() AND referenced_table_name IS NOT NULL ORDER BY 1"
            }
        }
    }

    /// Query describing a table's columns: rows of (name, type, nullable, key).
    fn columns_sql(self, table: &str) -> String {
        let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
        match self {
            Driver::Sqlite => format!(
                "SELECT name, type, CASE \"notnull\" WHEN 1 THEN 'NO' ELSE 'YES' END, CASE WHEN pk > 0 THEN 'PK' ELSE '' END FROM pragma_table_info({})",
                lit(table)
            ),
            Driver::Postgres => {
                let (schema, t) = table.split_once('.').unwrap_or(("public", table));
                format!(
                    // format_type: varchar(50), numeric(10,2), int[], enum names (data_type has
                    // character varying, numeric, ARRAY, USER-DEFINED)
                    "SELECT c.column_name, (SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a \
                     WHERE a.attrelid = (quote_ident(c.table_schema) || '.' || quote_ident(c.table_name))::regclass AND a.attname = c.column_name), c.is_nullable, CASE WHEN k.column_name IS NOT NULL THEN 'PK' ELSE '' END \
                     FROM information_schema.columns c LEFT JOIN (SELECT ku.column_name FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage ku ON tc.constraint_name = ku.constraint_name AND tc.table_schema = ku.table_schema \
                     WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = {s} AND tc.table_name = {t}) k ON k.column_name = c.column_name \
                     WHERE c.table_schema = {s} AND c.table_name = {t} ORDER BY c.ordinal_position",
                    s = lit(schema),
                    t = lit(t)
                )
            }
            Driver::Mysql => format!(
                "SELECT column_name, column_type, is_nullable, IF(column_key = 'PRI', 'PK', '') FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = {} ORDER BY ordinal_position",
                lit(table)
            ),
        }
    }
}

/// Pending grid edits of a table editor, by original row index.
#[derive(Default, Clone, Debug)]
pub struct Changes {
    /// (row, column) -> new value; None = NULL.
    pub cells: std::collections::BTreeMap<(usize, usize), Option<String>>,
    pub deleted: std::collections::BTreeSet<usize>,
    /// Rows added in the grid (not in the database yet).
    pub inserted: std::collections::BTreeSet<usize>,
}

impl Changes {
    pub fn count(&self) -> usize {
        let edited_rows: std::collections::BTreeSet<usize> = self
            .cells
            .keys()
            .map(|k| k.0)
            .filter(|r| !self.deleted.contains(r) && !self.inserted.contains(r))
            .collect();
        // a row added and deleted again is no change
        edited_rows.len()
            + self.deleted.difference(&self.inserted).count()
            + self
                .inserted
                .iter()
                .filter(|r| !self.deleted.contains(r))
                .count()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// The INSERT / UPDATE / DELETE statements that save these changes. Existing rows are
    /// found by their primary key (`key` = column indexes), with the values as loaded.
    pub fn to_sql(
        &self,
        driver: Driver,
        table: &str,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        key: &[usize],
    ) -> Result<Vec<String>, String> {
        let t = driver.table_ref(table);
        let q = |c: usize| driver.quote(&columns[c]);
        let where_key = |row: usize| {
            key.iter()
                .map(|&c| match rows[row][c].as_deref() {
                    None => format!("{} IS NULL", q(c)),
                    Some(v) => format!("{} = {}", q(c), driver.literal(v)),
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        if key.is_empty() && self.cells.keys().any(|k| !self.inserted.contains(&k.0))
            || key.is_empty() && self.deleted.iter().any(|r| !self.inserted.contains(r))
        {
            return Err(
                "this table has no primary key, so rows cannot be changed from the grid; use SQL"
                    .into(),
            );
        }
        let mut out = vec![];
        for &row in &self.deleted {
            if !self.inserted.contains(&row) {
                out.push(format!("DELETE FROM {t} WHERE {};", where_key(row)));
            }
        }
        let mut by_row: std::collections::BTreeMap<usize, Vec<(usize, &Option<String>)>> =
            Default::default();
        for ((row, col), v) in &self.cells {
            if !self.deleted.contains(row) {
                by_row.entry(*row).or_default().push((*col, v));
            }
        }
        for &row in self.inserted.iter().filter(|r| !self.deleted.contains(r)) {
            let cells = by_row.remove(&row).unwrap_or_default();
            if cells.is_empty() {
                out.push(if driver == Driver::Mysql {
                    format!("INSERT INTO {t} () VALUES ();")
                } else {
                    format!("INSERT INTO {t} DEFAULT VALUES;")
                });
            } else {
                let cols = cells
                    .iter()
                    .map(|(c, _)| q(*c))
                    .collect::<Vec<_>>()
                    .join(", ");
                let vals = cells
                    .iter()
                    .map(|(_, v)| driver.literal_opt(v.as_deref()))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(format!("INSERT INTO {t} ({cols}) VALUES ({vals});"));
            }
        }
        for (row, cells) in by_row {
            let set = cells
                .iter()
                .map(|(c, v)| format!("{} = {}", q(*c), driver.literal_opt(v.as_deref())))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!("UPDATE {t} SET {set} WHERE {};", where_key(row)));
        }
        Ok(out)
    }
}

/// How a connection gets its credentials.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// User name and password; the password is kept in the OS keychain.
    #[default]
    Password,
    /// User name, and the password read from one field of a Vault KV secret.
    VaultKv,
    /// Vault's database secrets engine creates a temporary user name and password.
    VaultDynamic,
}

impl Auth {
    pub const ALL: [Auth; 3] = [Auth::Password, Auth::VaultKv, Auth::VaultDynamic];

    pub fn label(self) -> &'static str {
        match self {
            Auth::Password => "Password",
            Auth::VaultKv => "Vault secret",
            Auth::VaultDynamic => "Vault dynamic credentials",
        }
    }

    pub fn is_vault(self) -> bool {
        self != Auth::Password
    }

    pub fn parse(s: &str) -> Option<Auth> {
        Auth::ALL.into_iter().find(|a| {
            serde_json::to_value(a)
                .ok()
                .and_then(|v| v.as_str().map(|v| v == s))
                .unwrap_or(false)
        })
    }
}

/// TLS for PostgreSQL connections, like libpq's sslmode.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Ssl {
    /// TLS when the server offers it (sends the host name, which poolers like Supavisor need).
    #[default]
    Prefer,
    /// Plain TCP.
    Disable,
    /// TLS or fail; the certificate is not checked.
    Require,
    /// TLS, and the certificate must be trusted by the OS and match the host name.
    VerifyFull,
}

impl Ssl {
    pub const ALL: [Ssl; 4] = [Ssl::Prefer, Ssl::Require, Ssl::VerifyFull, Ssl::Disable];

    pub fn label(self) -> &'static str {
        match self {
            Ssl::Prefer => "Prefer",
            Ssl::Disable => "Off",
            Ssl::Require => "Require",
            Ssl::VerifyFull => "Verify certificate",
        }
    }
}

/// The OS secure storage, as users know it.
pub fn keychain_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "system keyring (Secret Service)"
    }
}

/// A saved connection, as in DBeaver's Database Navigator.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConnConfig {
    /// Stable id; names the connection's entry in the OS keychain.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub driver: Driver,
    /// Host name (PostgreSQL / MySQL).
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    /// Database name, or the file path for SQLite.
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub user: String,
    /// For `auth: password`. Saved in the OS keychain, not in connections.json.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// password (default), vault_kv or vault_dynamic.
    #[serde(default)]
    pub auth: Auth,
    /// TLS (PostgreSQL, MySQL): prefer (default), disable, require or verify_full.
    #[serde(default)]
    pub ssl: Ssl,
    /// Vault path: the KV secret (`secret/data/team/db`) or the credentials
    /// (`database/creds/<role>`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vault_path: String,
    /// For vault_kv: the field holding the password; empty: "password".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vault_field: String,
    /// The password is in the OS keychain (connections.json holds none).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keychain: bool,
    /// Vault address for the Vault modes; empty: VAULT_ADDR.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vault_addr: String,
    /// Vault OIDC auth mount; empty: "oidc".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vault_mount: String,
    /// Vault OIDC role; empty: the mount's default role.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vault_role: String,
    /// Statements run in a transaction that stays open until Commit / Rollback.
    #[serde(default)]
    pub manual_commit: bool,
    /// A production database: shown in red, and changing data asks for confirmation.
    #[serde(default)]
    pub production: bool,
    /// Refuse every statement that changes data or schema.
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// None is SQL NULL (a text 'NULL' is Some("NULL")).
    pub rows: Vec<Vec<Option<String>>>,
    /// Rows changed by the last statement without a result set.
    pub affected: Option<u64>,
    #[serde(skip)]
    pub elapsed: Duration,
    /// More rows existed than `MAX_ROWS`.
    pub truncated: bool,
}

/// A cell as text: NULL for SQL NULL (the grid, text exports, catalog queries).
pub fn text(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("NULL")
}

/// Rows kept per result set; the grid is virtual, but memory isn't free.
pub const MAX_ROWS: usize = 10_000;

pub enum Session {
    Sqlite(rusqlite::Connection),
    Postgres(postgres::Client),
    Mysql(mysql::Conn),
}

/// Open a session. Vault credentials are fetched first (which may open the browser
/// for the sign-in); callers that need the resolved credentials call `vault::resolve` themselves.
pub fn connect(c: &ConnConfig) -> Result<Session, String> {
    let c = &crate::vault::resolve(c, &crate::vault::open_browser)?;
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let mut s = match c.driver {
        Driver::Sqlite => {
            // ~/data/app.db: the shell would expand it, SQLite does not
            let path = match c.database.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                None => PathBuf::from(&c.database),
            };
            Session::Sqlite(rusqlite::Connection::open(&path).map_err(|x| e(&x))?)
        }
        Driver::Postgres => Session::Postgres(
            pg_config(c)
                .connect(pg_tls(c.ssl == Ssl::VerifyFull)?)
                .map_err(pg_err)?,
        ),
        Driver::Mysql => {
            let opts = mysql::OptsBuilder::new()
                .ip_or_hostname(Some(if c.host.is_empty() {
                    "localhost"
                } else {
                    &c.host
                }))
                .tcp_port(if c.port == 0 { 3306 } else { c.port })
                .user(Some(&c.user))
                .pass(Some(&c.password))
                .db_name((!c.database.is_empty()).then_some(&c.database))
                .tcp_connect_timeout(Some(Duration::from_secs(10)))
                // UPDATE counts the rows it matched (like Postgres), not only those it changed
                .additional_capabilities(mysql::consts::CapabilityFlags::CLIENT_FOUND_ROWS);
            let tls = match c.ssl {
                Ssl::Disable => None,
                // Encrypted, certificate not checked (like Postgres prefer/require).
                Ssl::Prefer | Ssl::Require => Some(
                    mysql::SslOpts::default()
                        .with_danger_accept_invalid_certs(true)
                        .with_danger_skip_domain_validation(true),
                ),
                Ssl::VerifyFull => {
                    Some(mysql::SslOpts::default().with_root_cert_path(Some(os_roots_pem()?)))
                }
            };
            let conn = match mysql::Conn::new(opts.clone().ssl_opts(tls)) {
                Err(mysql::Error::DriverError(mysql::DriverError::TlsNotSupported))
                    if c.ssl == Ssl::Prefer =>
                {
                    mysql::Conn::new(opts)
                }
                r => r,
            };
            Session::Mysql(conn.map_err(|x| e(&x))?)
        }
    };
    // The server enforces read-only too: `is_write` is a keyword scan that quoting can fool.
    if c.read_only {
        s.query(match c.driver {
            Driver::Sqlite => "PRAGMA query_only = ON",
            Driver::Postgres => "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY",
            Driver::Mysql => "SET SESSION TRANSACTION READ ONLY",
        })?;
    }
    Ok(s)
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ForeignKey {
    pub table: String,
    pub column: String,
    pub ref_table: String,
    /// Empty when SQLite's REFERENCES names no column and the table has no primary key.
    pub ref_column: String,
}

/// Stops the statement a session is running, from another thread.
pub enum Canceller {
    Sqlite(rusqlite::InterruptHandle),
    Postgres(postgres::CancelToken),
    /// MySQL has no out-of-band cancel: a second connection runs `KILL QUERY <id>`.
    Mysql(ConnConfig, u32),
}

impl Canceller {
    pub fn cancel(&self) -> Result<(), String> {
        match self {
            Canceller::Sqlite(h) => {
                h.interrupt();
                Ok(())
            }
            Canceller::Postgres(t) => t.cancel_query(pg_tls(false)?).map_err(|e| e.to_string()),
            Canceller::Mysql(cfg, id) => {
                connect(cfg)?.query(&format!("KILL QUERY {id}")).map(|_| ())
            }
        }
    }
}

impl Session {
    pub fn canceller(&self, cfg: &ConnConfig) -> Canceller {
        match self {
            Session::Sqlite(c) => Canceller::Sqlite(c.get_interrupt_handle()),
            Session::Postgres(c) => Canceller::Postgres(c.cancel_token()),
            Session::Mysql(c) => Canceller::Mysql(cfg.clone(), c.connection_id()),
        }
    }

    /// Run one or more statements; the last result set (or affected count) is returned.
    pub fn query(&mut self, sql: &str) -> Result<QueryResult, String> {
        Ok(self.query_all(sql)?.pop().unwrap_or_default())
    }

    /// Every result of a script: one per statement (rows, or the count of changed rows).
    /// Each carries the time of the whole script.
    pub fn query_all(&mut self, sql: &str) -> Result<Vec<QueryResult>, String> {
        let start = Instant::now();
        let mut v = match self {
            Session::Sqlite(c) => sqlite_query(c, sql),
            Session::Postgres(c) => match pg_capped(sql).map(|capped| c.simple_query(&capped)) {
                Some(Ok(messages)) => Ok(pg_results(messages)),
                // Only a capped form that does not parse runs as typed: nothing of it ran. Any
                // other error (cancel, division by zero) is the answer, not a reason to run the
                // query again, uncapped.
                Some(Err(e)) if e.code() == Some(&postgres::error::SqlState::SYNTAX_ERROR) => {
                    pg_query(c, sql)
                }
                Some(Err(e)) => Err(pg_err(e)),
                None => pg_query(c, sql),
            },
            Session::Mysql(c) => mysql_query(c, sql),
        }?;
        for r in &mut v {
            r.elapsed = start.elapsed();
        }
        Ok(v)
    }

    fn driver(&self) -> Driver {
        match self {
            Session::Sqlite(_) => Driver::Sqlite,
            Session::Postgres(_) => Driver::Postgres,
            Session::Mysql(_) => Driver::Mysql,
        }
    }

    /// (name, TABLE|VIEW) pairs.
    pub fn tables(&mut self) -> Result<Vec<(String, String)>, String> {
        let r = self.query(self.driver().tables_sql())?;
        Ok(r.rows
            .into_iter()
            .map(|row| (text(&row[0]).to_string(), text(&row[1]).to_string()))
            .collect())
    }

    /// CREATE statement of a table or view.
    pub fn ddl(&mut self, table: &str) -> Result<String, String> {
        let driver = self.driver();
        if let Some(sql) = driver.ddl_sql(table) {
            let r = self.query(&sql)?;
            // MySQL: (name, CREATE ...) for tables, plus charset columns for views
            if let Some(v) = r.rows.first().and_then(|row| row.get(1).or(row.last())) {
                return Ok(format!("{};", text(v)));
            }
        }
        // Postgres has no SHOW CREATE: rebuild it from the columns.
        let cols = self.columns(table)?;
        let mut lines: Vec<String> = cols
            .rows
            .iter()
            .map(|c| {
                format!(
                    "    {} {}{}",
                    driver.quote(text(&c[0])),
                    text(&c[1]),
                    if text(&c[2]) == "NO" { " NOT NULL" } else { "" }
                )
            })
            .collect();
        let pk: Vec<String> = cols
            .rows
            .iter()
            .filter(|c| text(&c[3]) == "PK")
            .map(|c| driver.quote(text(&c[0])))
            .collect();
        if !pk.is_empty() {
            lines.push(format!("    PRIMARY KEY ({})", pk.join(", ")));
        }
        Ok(format!(
            "CREATE TABLE {} (\n{}\n);",
            driver.table_ref(table),
            lines.join(",\n")
        ))
    }

    /// "PostgreSQL 17.6", "MySQL 8.4.2", "SQLite 3.50.1".
    pub fn server_version(&mut self) -> String {
        let sql = match self.driver() {
            Driver::Sqlite => "SELECT 'SQLite ' || sqlite_version()",
            Driver::Postgres => "SELECT 'PostgreSQL ' || current_setting('server_version')",
            Driver::Mysql => "SELECT CONCAT('MySQL ', VERSION())",
        };
        self.query(sql)
            .ok()
            .and_then(|r| r.rows.first()?.first().cloned().flatten())
            .map(|v| v.split(" (").next().unwrap_or(&v).to_string())
            .unwrap_or_default()
    }

    /// After an error: is the connection itself gone (server restart, network)?
    pub fn is_broken(&mut self) -> bool {
        match self {
            Session::Sqlite(_) => false,
            Session::Postgres(c) => c.is_closed(),
            Session::Mysql(c) => c.ping().is_err(),
        }
    }

    pub fn begin(&mut self) -> Result<(), String> {
        let begin = if self.driver() == Driver::Mysql {
            "START TRANSACTION"
        } else {
            "BEGIN"
        };
        self.query(begin).map(|_| ())
    }

    /// Run statements as one transaction: all of them or none. Inside a transaction the
    /// user opened (manual commit, `own_tx` false) they run behind a savepoint, so a failure
    /// takes back the ones before it too; committing is up to the user.
    pub fn apply_in(&mut self, statements: &[String], own_tx: bool) -> Result<QueryResult, String> {
        if !own_tx {
            // MySQL accepts a savepoint outside a transaction (and keeps nothing), so `run`
            // forgets the transaction after MySQL DDL, which commits it
            if self.query("SAVEPOINT rusty_beaver").is_err() {
                // no transaction after all (COMMIT typed in a script)
                return self.apply(statements);
            }
            let mut affected = 0;
            for s in statements {
                match self.change(s) {
                    Ok(n) => affected += n,
                    Err(e) => {
                        let _ = self.query("ROLLBACK TO SAVEPOINT rusty_beaver");
                        // SQLite: an outermost savepoint is a transaction of its own
                        let _ = self.query("RELEASE SAVEPOINT rusty_beaver");
                        return Err(format!("{e}\n(none of them kept; in: {s})"));
                    }
                }
            }
            // every statement worked; MySQL may have no savepoint left to release (DDL ended
            // the transaction), which changes nothing
            let _ = self.query("RELEASE SAVEPOINT rusty_beaver");
            return Ok(QueryResult {
                affected: Some(affected),
                ..Default::default()
            });
        }
        self.apply(statements)
    }

    /// One statement of a save: its row count. An UPDATE or DELETE of the grid finds its row
    /// by key, so 0 rows means the row changed or went away meanwhile (or its key cannot be
    /// matched, like a binary one): an error, not a silent no-op.
    fn change(&mut self, s: &str) -> Result<u64, String> {
        let n = self.query(s)?.affected.unwrap_or(0);
        if n == 0 && (s.starts_with("UPDATE ") || s.starts_with("DELETE ")) {
            return Err("the row was not found: changed or deleted meanwhile? Reload".into());
        }
        Ok(n)
    }

    /// Run statements as one transaction: all of them or none.
    pub fn apply(&mut self, statements: &[String]) -> Result<QueryResult, String> {
        self.begin()?;
        let mut affected = 0;
        for s in statements {
            match self.change(s) {
                Ok(n) => affected += n,
                Err(e) => {
                    let _ = self.query("ROLLBACK");
                    return Err(format!("{e}\n(rolled back: {s})"));
                }
            }
        }
        self.query("COMMIT")?;
        Ok(QueryResult {
            affected: Some(affected),
            ..Default::default()
        })
    }

    /// Columns of a table: name, type, nullable, key.
    /// Columns of a table: name, type, nullable, key, and what a foreign key references.
    pub fn columns(&mut self, table: &str) -> Result<QueryResult, String> {
        let mut r = self.query(&self.driver().columns_sql(table))?;
        r.columns = ["Column", "Type", "Nullable", "Key", "References"]
            .map(String::from)
            .to_vec();
        let fks: Vec<ForeignKey> = self
            .foreign_keys()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.table == table)
            .collect();
        for row in &mut r.rows {
            let refs = fks
                .iter()
                .filter(|f| f.column == text(&row[0]))
                .map(|f| {
                    if f.ref_column.is_empty() {
                        f.ref_table.clone()
                    } else {
                        format!("{}.{}", f.ref_table, f.ref_column)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            if !refs.is_empty() && text(&row[3]).is_empty() {
                row[3] = Some("FK".into());
            }
            row.push(Some(refs));
        }
        Ok(r)
    }

    /// Tables (at most `limit`) with their columns, and every foreign key: an ER diagram's input.
    pub fn schema(
        &mut self,
        limit: usize,
    ) -> Result<(Vec<crate::er::Table>, Vec<ForeignKey>), String> {
        let fks = self.foreign_keys()?;
        let mut tables = vec![];
        for (name, kind) in self.tables()?.into_iter().take(limit) {
            if kind == "VIEW" {
                continue;
            }
            let cols = self.query(&self.driver().columns_sql(&name))?;
            let columns = cols
                .rows
                .into_iter()
                .map(|mut c| {
                    let fk = fks
                        .iter()
                        .any(|f| f.table == name && f.column == text(&c[0]));
                    let key = if text(&c[3]) == "PK" {
                        "PK"
                    } else if fk {
                        "FK"
                    } else {
                        ""
                    };
                    (
                        c[0].take().unwrap_or_default(),
                        c[1].take().unwrap_or_default(),
                        key.to_string(),
                    )
                })
                .collect();
            tables.push(crate::er::Table { name, columns });
        }
        Ok((tables, fks))
    }

    pub fn foreign_keys(&mut self) -> Result<Vec<ForeignKey>, String> {
        let r = self.query(self.driver().foreign_keys_sql())?;
        Ok(r.rows
            .into_iter()
            .map(|mut x| ForeignKey {
                table: x[0].take().unwrap_or_default(),
                column: x[1].take().unwrap_or_default(),
                ref_table: x[2].take().unwrap_or_default(),
                ref_column: x[3].take().unwrap_or_default(),
            })
            .collect())
    }
}

fn push_row(r: &mut QueryResult, row: Vec<Option<String>>) {
    if r.rows.len() < MAX_ROWS {
        r.rows.push(row);
    } else {
        r.truncated = true;
    }
}

/// How a SQLite blob shows in the grid: a description, not the value.
pub fn is_blob_text(v: &str) -> bool {
    v.starts_with("[BLOB ") && v.ends_with(" bytes]")
}

fn sqlite_query(c: &rusqlite::Connection, sql: &str) -> Result<Vec<QueryResult>, String> {
    use rusqlite::fallible_iterator::FallibleIterator as _;
    use rusqlite::types::ValueRef;
    let mut out = vec![];
    let mut batch = rusqlite::Batch::new(c, sql);
    while let Some(mut stmt) = batch.next().map_err(|e| e.to_string())? {
        let n = stmt.column_count();
        if n == 0 {
            // changes() keeps the last INSERT/UPDATE/DELETE's count through DDL and COMMIT
            let before = c.total_changes();
            let changed = stmt.execute([]).map_err(|e| e.to_string())?;
            let changed = if c.total_changes() == before {
                0
            } else {
                changed
            };
            out.push(QueryResult {
                affected: Some(changed as u64),
                ..Default::default()
            });
            continue;
        }
        let mut r = QueryResult {
            columns: stmt.column_names().iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let cells = (0..n)
                .map(|i| match row.get_ref(i) {
                    Ok(ValueRef::Null) | Err(_) => None,
                    Ok(ValueRef::Integer(v)) => Some(v.to_string()),
                    Ok(ValueRef::Real(v)) => Some(v.to_string()),
                    Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
                    Ok(ValueRef::Blob(b)) => Some(format!("[BLOB {} bytes]", b.len())), // see is_blob_text
                })
                .collect();
            push_row(&mut r, cells);
            if r.truncated {
                break; // the rest would be read only to be dropped
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// A single read-only query with the row cap applied by the server: the Postgres client
/// collects a whole result in memory before `push_row` could stop it (50M rows: out of memory).
/// Writes, SELECT INTO and scripts stay as they are.
fn pg_capped(sql: &str) -> Option<String> {
    let heads = statement_heads(sql, false);
    let reads = heads.len() == 1
        && matches!(heads[0].0.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE")
        && !is_write(sql);
    let body = sql.trim_end().trim_end_matches(';');
    // "SELECT …; -- note": the ; inside would be a syntax error in the subquery
    let reads = reads && !body.contains(';');
    // FOR SHARE / KEY SHARE locks every row it reads: capped it would lock only the first ones
    let words: Vec<String> = body
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    let locks = words
        .windows(2)
        .any(|w| w[0] == "FOR" && matches!(w[1].as_str(), "SHARE" | "KEY" | "NO" | "UPDATE"));
    let reads = reads && !locks;
    // newlines: a trailing -- comment must not swallow the closing parenthesis
    reads.then(|| {
        format!(
            "SELECT * FROM (\n{body}\n) AS rusty_beaver LIMIT {}",
            MAX_ROWS + 1
        )
    })
}

fn pg_query(c: &mut postgres::Client, sql: &str) -> Result<Vec<QueryResult>, String> {
    Ok(pg_results(c.simple_query(sql).map_err(pg_err)?))
}

fn pg_results(messages: Vec<postgres::SimpleQueryMessage>) -> Vec<QueryResult> {
    use postgres::SimpleQueryMessage as M;
    let mut out = vec![];
    let mut current: Option<QueryResult> = None;
    for msg in messages {
        match msg {
            M::RowDescription(cols) => {
                current = Some(QueryResult {
                    columns: cols.iter().map(|c| c.name().to_string()).collect(),
                    ..Default::default()
                });
            }
            M::Row(row) => {
                let r = current.get_or_insert_with(|| QueryResult {
                    columns: row.columns().iter().map(|c| c.name().to_string()).collect(),
                    ..Default::default()
                });
                let cells = (0..row.len())
                    .map(|i| row.get(i).map(String::from))
                    .collect();
                push_row(r, cells);
            }
            M::CommandComplete(n) => {
                out.push(current.take().unwrap_or(QueryResult {
                    affected: Some(n),
                    ..Default::default()
                }));
            }
            _ => {}
        }
    }
    out
}

/// Startup parameters for PostgreSQL. Empty fields are left out: plain PostgreSQL ignores an
/// empty `database` (it falls back to the user name), but poolers such as Supavisor reject the
/// whole startup message ("bad_startup_payload").
fn pg_config(c: &ConnConfig) -> postgres::Config {
    let mut cfg = postgres::Config::new();
    cfg.host(if c.host.is_empty() {
        "localhost"
    } else {
        &c.host
    })
    .port(if c.port == 0 { 5432 } else { c.port })
    .connect_timeout(Duration::from_secs(10));
    if !c.user.is_empty() {
        cfg.user(&c.user);
    }
    if !c.password.is_empty() {
        cfg.password(&c.password);
    }
    if !c.database.is_empty() {
        cfg.dbname(&c.database);
    }
    cfg.ssl_mode(match c.ssl {
        // a Unix socket (host /var/run/postgresql) has no TLS; libpq ignores sslmode there
        _ if c.host.starts_with('/') => postgres::config::SslMode::Disable,
        Ssl::Disable => postgres::config::SslMode::Disable,
        Ssl::Prefer => postgres::config::SslMode::Prefer,
        Ssl::Require | Ssl::VerifyFull => postgres::config::SslMode::Require,
    });
    cfg
}

/// The CAs the OS trusts, as a PEM file: the MySQL driver checks certificates only against its
/// bundled public CAs plus such a file, so company CAs installed on the machine work too.
/// Written once per run into the user's own config dir (not a shared, cleaned-up temp dir
/// where another user could swap the CAs); a failed write is tried again next time.
fn os_roots_pem() -> Result<std::path::PathBuf, String> {
    use base64::Engine as _;
    static PEM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static WRITTEN: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    if let Some(p) = WRITTEN.get() {
        return Ok(p.clone());
    }
    let pem = PEM.get_or_init(|| {
        let mut out = String::new();
        let mut check = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            // The driver fails on any certificate rustls can't use: leave those out.
            if check.add(cert.clone()).is_ok() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(cert.as_ref());
                out.push_str("-----BEGIN CERTIFICATE-----\n");
                for line in b64.as_bytes().chunks(64) {
                    out.push_str(std::str::from_utf8(line).unwrap());
                    out.push('\n');
                }
                out.push_str("-----END CERTIFICATE-----\n");
            }
        }
        out
    });
    let dir = config_dir();
    let path = dir.join("os-roots.pem");
    std::fs::create_dir_all(&dir)
        .and_then(|_| write_atomic(&path, pem))
        .map_err(|e| format!("system CAs: {}: {e}", path.display()))?;
    Ok(WRITTEN.get_or_init(|| path).clone())
}

/// rustls for Postgres: with `verify`, the OS trust store checks the certificate; without, like
/// libpq's sslmode=prefer/require, the connection is encrypted but the certificate not checked.
fn pg_tls(verify: bool) -> Result<tokio_postgres_rustls::MakeRustlsConnect, String> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use std::sync::Arc;

    #[derive(Debug)]
    struct NoVerify(Arc<rustls::crypto::CryptoProvider>);
    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _: &CertificateDer,
            _: &[CertificateDer],
            _: &ServerName,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                m,
                c,
                d,
                &self.0.signature_verification_algorithms,
            )
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                m,
                c,
                d,
                &self.0.signature_verification_algorithms,
            )
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let config = if verify {
        use rustls_platform_verifier::BuilderVerifierExt;
        builder
            .with_platform_verifier()
            .map_err(|e| e.to_string())?
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
    }
    .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

/// postgres::Error displays as "db error": show the server's message, detail and hint.
fn pg_err(e: postgres::Error) -> String {
    match e.as_db_error() {
        Some(d) => {
            let mut s = format!("{}: {}", d.severity(), d.message());
            if let Some(x) = d.detail() {
                s += &format!("\nDETAIL: {x}");
            }
            if let Some(x) = d.hint() {
                s += &format!("\nHINT: {x}");
            }
            s
        }
        // "error connecting to server" alone hides why: add the causes ("Connection refused").
        None => {
            let mut s = e.to_string();
            let mut cause = std::error::Error::source(&e);
            while let Some(c) = cause {
                let c_s = c.to_string();
                if !s.contains(&c_s) {
                    s += &format!(": {c_s}");
                }
                cause = c.source();
            }
            s
        }
    }
}

fn mysql_query(c: &mut mysql::Conn, sql: &str) -> Result<Vec<QueryResult>, String> {
    use mysql::Value as V;
    use mysql::prelude::Queryable;
    let mut out = vec![];
    let mut result = c.query_iter(sql).map_err(|e| e.to_string())?;
    while let Some(set) = result.iter() {
        let columns: Vec<String> = set
            .columns()
            .as_ref()
            .iter()
            .map(|c| c.name_str().into_owned())
            .collect();
        let affected = set.affected_rows();
        let mut r = QueryResult {
            columns,
            ..Default::default()
        };
        for row in set {
            let row = row.map_err(|e| e.to_string())?;
            if r.rows.len() >= MAX_ROWS {
                // past the cap: the rest is only read off the connection (MySQL needs that)
                r.truncated = true;
                continue;
            }
            let cells = (0..row.len())
                .map(|i| match row.as_ref(i) {
                    None | Some(V::NULL) => None,
                    Some(V::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
                    Some(V::Int(v)) => Some(v.to_string()),
                    Some(V::UInt(v)) => Some(v.to_string()),
                    Some(V::Float(v)) => Some(v.to_string()),
                    Some(V::Double(v)) => Some(v.to_string()),
                    Some(v @ (V::Date(..) | V::Time(..))) => {
                        Some(v.as_sql(true).trim_matches('\'').to_string())
                    }
                })
                .collect();
            push_row(&mut r, cells);
        }
        if r.columns.is_empty() {
            r.affected = Some(affected);
        }
        out.push(r);
    }
    Ok(out)
}

/// Write a file through a temporary one renamed over it: a crash midway leaves the old file,
/// not half of the new one.
pub fn write_atomic(path: &std::path::Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    // a symlinked file (dotfiles) is written where it points, the link stays
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // unique per process and call: two writes of the same file at once don't share it
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    let r = std::fs::write(&tmp, data).and_then(|_| {
        if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions()); // keep a chmod 600
        }
        std::fs::rename(&tmp, path)
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp); // a full disk leaves no half file behind
    }
    r
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rusty-beaver")
}

fn connections_path() -> PathBuf {
    std::env::var_os("RUSTY_BEAVER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("connections.json"))
}

/// Saved connections; on first start a bundled demo SQLite database.
/// Passwords this session already has in the keychain, per connection id: saves skip
/// unchanged ones (each keychain write can mean a system prompt).
static IN_KEYCHAIN: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
    std::sync::Mutex::new(None);

fn keychain_has(id: &str, password: Option<&str>) -> bool {
    let k = IN_KEYCHAIN.lock().unwrap();
    k.as_ref()
        .and_then(|m| m.get(id))
        .is_some_and(|p| password.is_none_or(|q| p == q))
}

fn keychain_note(id: &str, password: Option<&str>) {
    let mut k = IN_KEYCHAIN.lock().unwrap();
    let m = k.get_or_insert_with(Default::default);
    match password {
        Some(p) => m.insert(id.to_string(), p.to_string()),
        None => m.remove(id),
    };
}

/// Saved connections, passwords read back from the keychain; on first start a bundled demo
/// SQLite database. The flag says the file should be saved again (new ids, or plain-text
/// passwords to move into the keychain). A file that does not parse is moved aside (the
/// message says where) instead of being overwritten by the next save.
pub fn load_connections() -> (Vec<ConnConfig>, bool, Option<String>) {
    let path = connections_path();
    let mut note = None;
    let mut list: Vec<ConnConfig> = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        bad => {
            // only a missing file is a first start; any other error keeps the file aside
            if let Err(e) = bad.as_ref().map(|_| ())
                && path.exists()
            {
                let aside = path.with_extension("json.invalid");
                note = Some(match std::fs::rename(&path, &aside) {
                    Ok(()) => format!(
                        "{} did not load ({e}); it is now {}",
                        path.display(),
                        aside.display()
                    ),
                    Err(r) => format!("{} did not load ({e}) nor move aside ({r})", path.display()),
                });
            }
            return match demo_db() {
                Ok(path) => (
                    vec![ConnConfig {
                        name: "Beaver Demo (SQLite)".into(),
                        database: path.to_string_lossy().into_owned(),
                        id: secrets::new_id(),
                        ..Default::default()
                    }],
                    true,
                    note,
                ),
                Err(_) => (Vec::new(), false, note),
            };
        }
    };
    let mut resave = false;
    for c in &mut list {
        if c.id.is_empty() {
            c.id = secrets::new_id();
            resave = true;
        }
        if c.keychain {
            match secrets::load(&c.id) {
                Ok(Some(p)) => {
                    keychain_note(&c.id, Some(&p));
                    c.password = p;
                }
                Ok(None) => eprintln!("{}: no password in the system keychain", c.name),
                Err(e) => eprintln!("{}: could not read the system keychain: {e}", c.name),
            }
        } else if crate::vault::upgrade_legacy(c) {
            resave = true; // `vault:` in the password field, from before the explicit modes
        } else if secrets::enabled() && c.auth == Auth::Password && !c.password.is_empty() {
            resave = true; // a plain-text password from before: move it into the keychain
        }
    }
    (list, resave, None)
}

/// Save connections. Passwords go into the OS keychain and connections.json keeps none; Vault
/// references are no secrets and stay in the file. Returns warnings for passwords that had to
/// stay in the file because the keychain is not available.
pub fn save_connections(list: &[ConnConfig]) -> Result<Vec<String>, String> {
    let mut warnings = vec![];
    let file: Vec<ConnConfig> = list
        .iter()
        .map(|c| {
            let mut f = c.clone();
            f.keychain = false;
            let secret = c.auth == Auth::Password && !c.password.is_empty();
            if secrets::enabled() && secret {
                let stored = keychain_has(&c.id, Some(&c.password)) || secrets::store(&c.id, &c.password).inspect(|_| keychain_note(&c.id, Some(&c.password))).inspect_err(|e| warnings.push(format!("{}: the system keychain is not available ({e}); its password stays in connections.json", c.name))).is_ok();
                if stored {
                    f.password.clear();
                    f.keychain = true;
                }
            } else if c.keychain && c.auth == Auth::Password && c.password.is_empty() {
                // the keychain could not be read this run: the password is still there
                f.keychain = true;
            } else if secrets::enabled() && keychain_has(&c.id, None) {
                // no longer a stored password (now empty or a Vault reference)
                secrets::delete(&c.id);
                keychain_note(&c.id, None);
            }
            f
        })
        .collect();
    let path = connections_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // A new file is for its owner only: without a keychain it holds passwords. write_atomic
    // keeps the mode of the file it replaces.
    #[cfg(unix)]
    if !path.exists() {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path);
    }
    write_atomic(&path, serde_json::to_string_pretty(&file).unwrap()).map_err(|e| e.to_string())?;
    Ok(warnings)
}

/// A deleted connection's keychain entry goes with it.
pub fn forget_password(c: &ConnConfig) {
    if secrets::enabled() && !c.id.is_empty() {
        secrets::delete(&c.id);
        keychain_note(&c.id, None);
    }
}

/// A small demo database so the app has something to show out of the box.
fn demo_db() -> Result<PathBuf, String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("demo.db");
    if path.exists() {
        return Ok(path);
    }
    let c = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
    c.execute_batch(include_str!("demo.sql"))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// SQL templates for a table, like DBeaver's "Generate SQL" menu.
/// `cols` are (name, is_key); kind: select | insert | update | delete | count.
pub fn generate(
    driver: Driver,
    table: &str,
    cols: &[(String, bool)],
    kind: &str,
) -> Result<String, String> {
    let t = driver.table_ref(table);
    let names = || {
        cols.iter()
            .map(|(c, _)| driver.quote(c))
            .collect::<Vec<_>>()
    };
    let key: Vec<String> = cols
        .iter()
        .filter(|(_, k)| *k)
        .map(|(c, _)| format!("{} = ?", driver.quote(c)))
        .collect();
    let where_ = if key.is_empty() {
        "<condition>".to_string()
    } else {
        key.join(" AND ")
    };
    Ok(match kind {
        "select" => format!("SELECT {}\nFROM {t};", names().join(",\n       ")),
        "insert" => format!(
            "INSERT INTO {t} ({})\nVALUES ({});",
            names().join(", "),
            vec!["?"; cols.len()].join(", ")
        ),
        "update" => format!(
            "UPDATE {t}\nSET {}\nWHERE {where_};",
            cols.iter()
                .filter(|(_, k)| !k)
                .map(|(c, _)| format!("{} = ?", driver.quote(c)))
                .collect::<Vec<_>>()
                .join(",\n    ")
        ),
        "delete" => format!("DELETE FROM {t}\nWHERE {where_};"),
        "count" => format!("SELECT count(*) FROM {t};"),
        other => {
            return Err(format!(
                "unknown kind {other:?}: select, insert, update, delete or count"
            ));
        }
    })
}

/// Byte ranges and names of `:name` parameters, outside quotes and comments (`::type` casts
/// and `:=` are not parameters).
fn param_spans(sql: &str) -> Vec<(std::ops::Range<usize>, String)> {
    // ponytail: standard quoting; after a MySQL \' a :name goes unseen and the server
    // reports it, nothing runs
    let mysql = false;
    let b = sql.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => i = quote_end(b, i, mysql),
            b'-' | b'#' if b[i] == b'#' && mysql || b[i] == b'-' && b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
            }
            b'$' => {
                if let Some(end) = dollar_quote_end(sql, i) {
                    i = end;
                    continue;
                }
            }
            b':' if b.get(i + 1) == Some(&b':') => i += 1,
            b':' if i > 0 && b[i - 1] == b':' => {}
            // arr[2:n], arr[i:j]: a slice, not a parameter
            b':' if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b']') => {}
            b':' if b
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') =>
            {
                let start = i;
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push((start..i, sql[start + 1..i].to_string()));
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Does the SQL change data or schema? Looks for INSERT, UPDATE, DELETE, MERGE, DROP, ALTER,
/// TRUNCATE, CREATE, GRANT, REVOKE, RENAME, REPLACE, COPY, CALL, ... as words outside strings
/// and comments.
/// ponytail: keyword scan errs on the side of asking (a column named "update" asks too); it
/// reads the SQL both as standard SQL and as MySQL (\' escapes, # comments): a write either
/// way counts
pub fn is_write(sql: &str) -> bool {
    writes(sql, false) || writes(sql, true)
}

/// Index of the closing quote of the string or quoted name opening at `i` (or the end).
/// MySQL: a backslash escapes the next byte.
fn quote_end(b: &[u8], i: usize, mysql: bool) -> usize {
    let q = b[i];
    let mut i = i + 1;
    while i < b.len() && b[i] != q {
        i += if mysql && b[i] == b'\\' { 2 } else { 1 };
    }
    i.min(b.len())
}

fn writes(sql: &str, mysql: bool) -> bool {
    const WRITES: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "MERGE", "DROP", "ALTER", "TRUNCATE", "CREATE", "GRANT",
        "REVOKE", "RENAME", "INTO",
    ];
    // Only as a statement's first word: "comment" and "lock" are column names too.
    const LEADING: &[&str] = &[
        "REPLACE", "COPY", "CALL", "DO", "LOAD", "VACUUM", "REINDEX", "CLUSTER", "COMMENT", "LOCK",
        "IMPORT", "REFRESH", "ANALYZE", "OPTIMIZE", "REPAIR", "FLUSH",
    ];
    let b = sql.as_bytes();
    let mut i = 0;
    let mut first = true;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => i = quote_end(b, i, mysql),
            b'-' | b'#' if b[i] == b'#' && mysql || b[i] == b'-' && b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
            }
            b'$' => {
                if let Some(end) = dollar_quote_end(sql, i) {
                    i = end;
                    continue;
                }
            }
            b';' => first = true,
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let word = &sql[start..i];
                let is = |list: &[&str]| list.iter().any(|w| w.eq_ignore_ascii_case(word));
                if is(WRITES) || first && is(LEADING) {
                    return true;
                }
                first = false;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The first two words of each statement, upper case, outside strings and comments: what a
/// script's statements are (`CREATE TEMPORARY`, `LOCK TABLES`, `COMMIT`).
pub fn statement_heads(sql: &str, mysql: bool) -> Vec<(String, String)> {
    let b = sql.as_bytes();
    let (mut out, mut cur) = (vec![], Vec::<String>::new());
    let mut flush = |cur: &mut Vec<String>| {
        if !cur.is_empty() {
            cur.resize(2, String::new());
            out.push((cur[0].clone(), cur[1].clone()));
            cur.clear();
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = quote_end(b, i, mysql);
                if cur.len() == 1 {
                    cur.push(String::new()); // CREATE "x": no second keyword
                }
            }
            b'-' | b'#' if b[i] == b'#' && mysql || b[i] == b'-' && b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'$' => {
                if let Some(end) = dollar_quote_end(sql, i) {
                    i = end;
                    continue;
                }
            }
            b';' => flush(&mut cur),
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if cur.len() < 2 {
                    cur.push(sql[start..i].to_ascii_uppercase());
                }
                continue;
            }
            c if !c.is_ascii_whitespace() && cur.len() == 1 => cur.push(String::new()),
            _ => {}
        }
        i += 1;
    }
    flush(&mut cur);
    out
}

/// Distinct parameter names in order of appearance.
pub fn params(sql: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for (_, n) in param_spans(sql) {
        if !v.contains(&n) {
            v.push(n);
        }
    }
    v
}

/// Replace each `:name` by its value, as typed (quote strings yourself, like DBeaver).
pub fn bind(
    sql: &str,
    values: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut last = 0;
    for (r, n) in param_spans(sql) {
        let v = values.get(&n).ok_or_else(|| format!("no value for :{n}"))?;
        out.push_str(&sql[last..r.start]);
        out.push_str(v);
        last = r.end;
    }
    out.push_str(&sql[last..]);
    Ok(out)
}

/// One query counting the rows of every table: (table, rows), biggest first.
pub fn row_counts_sql(driver: Driver, tables: &[String]) -> String {
    let parts: Vec<String> = tables
        .iter()
        .map(|t| {
            format!(
                "SELECT {} AS table_name, count(*) AS row_count FROM {}",
                driver.literal(t),
                driver.table_ref(t)
            )
        })
        .collect();
    format!("{}\nORDER BY row_count DESC;", parts.join("\nUNION ALL\n"))
}

/// End (past the closing tag) of a Postgres dollar-quoted string `$tag$ … $tag$` at `i`;
/// None when `i` starts none (`$1`, unclosed).
pub fn dollar_quote_end(sql: &str, i: usize) -> Option<usize> {
    let b = sql.as_bytes();
    let n = b[i + 1..]
        .iter()
        .position(|&c| !(c.is_ascii_alphanumeric() || c == b'_'))?;
    if b[i + 1 + n] != b'$' || b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let tag = &sql[i..i + n + 2];
    sql[i + n + 2..]
        .find(tag)
        .map(|j| i + n + 2 + j + tag.len())
}

/// A table filter split into its condition and a trailing `ORDER BY …` (outside quotes):
/// "x = 1 ORDER BY y" -> ("x = 1 ", "ORDER BY y").
pub fn split_order(filter: &str) -> (&str, &str) {
    // outside quotes and parentheses: id IN (SELECT … ORDER BY ts LIMIT 5) keeps its own
    let top = |s: &str| {
        s.matches('\'').count().is_multiple_of(2)
            && s.matches('(').count() == s.matches(')').count()
    };
    match filter.to_ascii_lowercase().rfind("order by") {
        Some(at) if top(&filter[..at]) => filter.split_at(at),
        _ => (filter, ""),
    }
}

/// Byte range of the statement at `offset`: statements end at `;` or at a blank line,
/// outside quotes and comments (DBeaver's "smart" delimiting). A cursor in the gap after a
/// statement belongs to that statement.
pub fn statement_at(sql: &str, offset: usize, mysql: bool) -> std::ops::Range<usize> {
    let b = sql.as_bytes();
    let mut bounds = vec![0];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => i = quote_end(b, i, mysql),
            b'-' | b'#' if b[i] == b'#' && mysql || b[i] == b'-' && b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'$' => {
                if let Some(end) = dollar_quote_end(sql, i) {
                    i = end;
                    continue;
                }
            }
            b';' => bounds.push(i + 1),
            b'\n' => {
                // a line holding only whitespace ends the statement
                let mut j = i + 1;
                while j < b.len() && (b[j] == b' ' || b[j] == b'\t' || b[j] == b'\r') {
                    j += 1;
                }
                if j < b.len() && b[j] == b'\n' {
                    bounds.push(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    bounds.push(sql.len());
    // Trimmed, non-empty pieces.
    let pieces: Vec<std::ops::Range<usize>> = bounds
        .windows(2)
        .filter_map(|w| {
            let text = &sql[w[0]..w[1]];
            let start = w[0] + (text.len() - text.trim_start().len());
            (!text.trim().is_empty()).then(|| start..w[0] + text.trim_end().len())
        })
        .collect();
    pieces
        .iter()
        .find(|r| r.start <= offset && offset <= r.end)
        .or_else(|| pieces.iter().rfind(|r| r.end <= offset))
        .or(pieces.first())
        .cloned()
        .unwrap_or(0..0)
}

/// A result as text in a format picked by file extension: csv, tsv, json, md, sql (INSERTs
/// into `table`), anything else is CSV.
pub fn export(r: &QueryResult, ext: &str, driver: Driver, table: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "json" => {
            // a repeated name (a.id, b.id) gets a suffix instead of overwriting: id, id_2 (one
            // that is no other column's name either)
            let mut used = std::collections::HashSet::new();
            let keys: Vec<String> = r
                .columns
                .iter()
                .map(|c| {
                    let mut key = c.clone();
                    let mut n = 1;
                    while used.contains(&key) || n > 1 && r.columns.contains(&key) {
                        n += 1;
                        key = format!("{c}_{n}");
                    }
                    used.insert(key.clone());
                    key
                })
                .collect();
            let rows: Vec<serde_json::Value> = r
                .rows
                .iter()
                .map(|row| {
                    serde_json::Value::Object(
                        keys.iter()
                            .zip(row)
                            .map(|(c, v)| {
                                (
                                    c.clone(),
                                    v.clone()
                                        .map_or(serde_json::Value::Null, serde_json::Value::String),
                                )
                            })
                            .collect(),
                    )
                })
                .collect();
            serde_json::to_string_pretty(&rows).unwrap() + "\n"
        }
        "md" | "markdown" => {
            let esc = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");
            let mut out = format!(
                "| {} |\n|{}|\n",
                r.columns
                    .iter()
                    .map(|c| esc(c))
                    .collect::<Vec<_>>()
                    .join(" | "),
                vec![" --- "; r.columns.len()].join("|")
            );
            for row in &r.rows {
                out += &format!(
                    "| {} |\n",
                    row.iter()
                        .map(|v| esc(text(v)))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
            out
        }
        "txt" | "text" => {
            use unicode_width::UnicodeWidthStr as _;
            // Aligned plain text, as DBeaver's Text view; long cells are cut at 60 characters.
            let cut = |v: &str| -> String {
                let v = v.replace(['\r', '\n', '\t'], " ");
                if v.chars().count() > 60 {
                    format!("{}…", v.chars().take(59).collect::<String>())
                } else {
                    v
                }
            };
            let cells: Vec<Vec<String>> =
                std::iter::once(r.columns.iter().map(|c| cut(c)).collect())
                    .chain(
                        r.rows
                            .iter()
                            .map(|row| row.iter().map(|v| cut(text(v))).collect()),
                    )
                    .collect();
            let widths: Vec<usize> = (0..r.columns.len())
                .map(|c| cells.iter().map(|row| row[c].width()).max().unwrap_or(0))
                .collect();
            let line = |row: &[String]| {
                row.iter()
                    .zip(&widths)
                    .map(|(v, w)| format!("{v}{}", " ".repeat(w - v.width()))) // by display width
                    .collect::<Vec<_>>()
                    .join(" | ")
                    .trim_end()
                    .to_string()
            };
            let mut out = line(&cells[0]) + "\n";
            out += &widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<_>>()
                .join("-+-");
            out += "\n";
            for row in &cells[1..] {
                out += &(line(row) + "\n");
            }
            out
        }
        "tsv" => {
            let line = |row: Vec<&str>| {
                row.iter()
                    .map(|v| v.replace(['\t', '\r', '\n'], " "))
                    .collect::<Vec<_>>()
                    .join("\t")
            };
            std::iter::once(line(r.columns.iter().map(String::as_str).collect()))
                .chain(
                    r.rows
                        .iter()
                        .map(|row| line(row.iter().map(text).collect())),
                )
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
        "sql" => {
            let cols = r
                .columns
                .iter()
                .map(|c| driver.quote(c))
                .collect::<Vec<_>>()
                .join(", ");
            r.rows
                .iter()
                .map(|row| {
                    format!(
                        "INSERT INTO {} ({cols}) VALUES ({});\n",
                        driver.table_ref(table),
                        row.iter()
                            .map(|v| driver.literal_opt(v.as_deref()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect()
        }
        _ => to_csv(r),
    }
}

/// Parse RFC 4180 CSV (quoted fields, "" escapes, CRLF or LF); blank lines are skipped, a
/// line holding just "" is a row with one empty field.
/// The separator is the one the header line has most of outside quotes: `,`, `;` (Excel in
/// German and other locales) or tab; `,` on a tie.
#[cfg(test)]
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    csv_fields(text)
        .into_iter()
        .map(|r| r.into_iter().map(|(v, _)| v).collect())
        .collect()
}

/// `parse_csv`, with each field's text and whether it was quoted ("" is an empty text, an
/// empty field is NULL).
fn csv_fields(text: &str) -> Vec<Vec<(String, bool)>> {
    let header = text
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .unwrap_or_default();
    let unquoted: String = header.split('"').step_by(2).collect();
    let sep = [',', ';', '\t']
        .into_iter()
        .rev()
        .max_by_key(|&c| unquoted.matches(c).count())
        .unwrap_or(',');
    let mut rows = vec![];
    let (mut row, mut field) = (vec![], String::new());
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let (mut quoted, mut had_quote, mut field_quoted) = (false, false, false);
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') if field.is_empty() => {
                (quoted, had_quote, field_quoted) = (true, true, true)
            }
            (false, c) if c == sep => row.push((
                std::mem::take(&mut field),
                std::mem::take(&mut field_quoted),
            )),
            (false, '\r') => {}
            (false, '\n') => {
                row.push((
                    std::mem::take(&mut field),
                    std::mem::take(&mut field_quoted),
                ));
                if had_quote || !(row.len() == 1 && row[0].0.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
                had_quote = false;
            }
            (false, c) => field.push(c),
        }
    }
    if had_quote || !field.is_empty() || !row.is_empty() {
        row.push((field, field_quoted));
        rows.push(row);
    }
    rows
}

/// A CSV file's text: UTF-8, else Latin-1 (what Excel writes in Western locales).
/// ponytail: Latin-1 stands in for Windows-1252 (all but € and a few others); add encoding_rs
/// if those matter
pub fn csv_text(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err("UTF-16 text: save it as CSV UTF-8".into());
    }
    String::from_utf8(bytes).or_else(|e| {
        let at = e.utf8_error().valid_up_to();
        let bytes = e.into_bytes();
        // UTF-8 before the bad byte (a BOM, umlauts): Latin-1 would garble all of them
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) || !bytes[..at].is_ascii() {
            return Err(format!("not valid UTF-8 at byte {at}"));
        }
        Ok(bytes.iter().map(|&b| b as char).collect())
    })
}

/// INSERT statements for CSV text whose header row names the table's columns.
/// Empty fields and NULL become SQL NULL; quoted ("", "NULL") they are text.
pub fn csv_inserts(driver: Driver, table: &str, text: &str) -> Result<Vec<String>, String> {
    let mut rows = csv_fields(text).into_iter();
    let header = rows.next().ok_or("the file is empty")?;
    let cols = header
        .iter()
        .map(|(h, _)| driver.quote(h.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    rows.enumerate()
        .map(|(i, r)| {
            if r.len() != header.len() {
                return Err(format!(
                    "line {}: {} fields, the header has {}",
                    i + 2,
                    r.len(),
                    header.len()
                ));
            }
            let vals = r
                .iter()
                .map(|(v, quoted)| {
                    if !quoted && (v.is_empty() || v == "NULL") {
                        "NULL".into()
                    } else {
                        driver.literal(v)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!(
                "INSERT INTO {} ({cols}) VALUES ({vals});",
                driver.table_ref(table)
            ))
        })
        .collect()
}

/// RFC 4180 CSV.
pub fn to_csv(r: &QueryResult) -> String {
    // NULL is a bare NULL, as the import reads it; an empty text and the text 'NULL' are
    // quoted so they stay text (and "" keeps a one-column row from reading as a blank line)
    let cell = |v: Option<&str>| match v {
        None => "NULL".to_string(),
        Some(s) if s.contains([',', ';', '\t', '"', '\n', '\r']) || s.is_empty() || s == "NULL" => {
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        Some(s) => s.to_string(),
    };
    let line = |row: Vec<Option<&str>>| row.into_iter().map(cell).collect::<Vec<_>>().join(",");
    let mut out = line(r.columns.iter().map(|c| Some(c.as_str())).collect());
    for row in &r.rows {
        out.push_str("\r\n");
        out.push_str(&line(row.iter().map(|v| v.as_deref()).collect()));
    }
    out.push_str("\r\n");
    out
}

/// Rows for tests: "NULL" is NULL.
#[cfg(test)]
pub fn cells(rows: Vec<Vec<&str>>) -> Vec<Vec<Option<String>>> {
    rows.into_iter()
        .map(|r| {
            r.into_iter()
                .map(|v| (v != "NULL").then(|| v.to_string()))
                .collect()
        })
        .collect()
}

/// Rows as text for tests: NULL is "NULL".
#[cfg(test)]
pub fn texts(rows: &[Vec<Option<String>>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|v| text(v).to_string()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_roundtrip_and_metadata() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        let r = s.query("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t(name) VALUES ('a'),(NULL);").unwrap();
        assert_eq!(r.affected, Some(2));
        let r = s.query("SELECT id, name FROM t ORDER BY id").unwrap();
        assert_eq!(r.columns, ["id", "name"]);
        assert_eq!(
            texts(&r.rows),
            [
                vec!["1".to_string(), "a".into()],
                vec!["2".into(), "NULL".into()]
            ]
        );
        assert_eq!(
            s.tables().unwrap(),
            [("t".to_string(), "TABLE".to_string())]
        );
        let cols = s.columns("t").unwrap();
        assert_eq!(texts(&cols.rows)[0], ["id", "INTEGER", "YES", "PK", ""]);
        s.query(
            "CREATE TABLE c(id INTEGER PRIMARY KEY, t_id INT REFERENCES t(id), x INT REFERENCES t)",
        )
        .unwrap();
        let fk = s.columns("c").unwrap();
        assert_eq!(texts(&fk.rows)[1][3..], ["FK", "t.id"]);
        assert_eq!(texts(&fk.rows)[2][3..], ["FK", "t.id"]); // bare REFERENCES t: its primary key
        assert_eq!(s.foreign_keys().unwrap().len(), 2);
        assert!(s.query("SELEC nope").is_err());
    }

    #[test]
    fn scripts_return_every_result() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        let v = s.query_all("CREATE TABLE t(a); INSERT INTO t VALUES (1),(2); SELECT * FROM t; SELECT count(*) FROM t;").unwrap();
        assert_eq!(v.len(), 4);
        assert_eq!(
            (v[1].affected, v[2].rows.len(), text(&v[3].rows[0][0])),
            (Some(2), 2, "2")
        );
        // DDL after a write reports no rows, not the write's count again
        let v = s.query_all("DELETE FROM t; CREATE TABLE x(a);").unwrap();
        assert_eq!((v[0].affected, v[1].affected), (Some(2), Some(0)));
        let mut c = Changes::default();
        c.inserted.insert(5);
        c.deleted.insert(5);
        assert!(c.is_empty()); // added, then deleted again
        assert_eq!(c.to_sql(Driver::Sqlite, "t", &[], &[], &[]), Ok(vec![]));
    }

    #[test]
    fn quoting_and_csv() {
        assert_eq!(
            Driver::Postgres.select_all("public.my\"t", "", 5),
            "SELECT * FROM \"public\".\"my\"\"t\" LIMIT 5;"
        );
        assert_eq!(
            Driver::Mysql.select_all("a`b", " id > 3 ", 1),
            "SELECT * FROM `a``b` WHERE id > 3 LIMIT 1;"
        );
        let r = QueryResult {
            columns: vec!["a".into(), "b".into()],
            rows: cells(vec![vec!["x,y", "q\""]]),
            ..Default::default()
        };
        assert_eq!(to_csv(&r), "a,b\r\n\"x,y\",\"q\"\"\"\r\n");
        let one = QueryResult {
            columns: vec!["note".into()],
            rows: cells(vec![vec![""], vec!["x"], vec![""]]),
            ..Default::default()
        };
        assert_eq!(parse_csv(&to_csv(&one)).len(), 4); // header + 3 rows, none lost
        assert_eq!(parse_csv("a\n\nb\n").len(), 2); // a blank line is still skipped
        assert_eq!(parse_csv("id;name\n1;\"a;b\"\n")[1], ["1", "a;b"]); // German Excel
        assert_eq!(
            parse_csv("Nachname, Vorname;Ort;PLZ\nA, B;C;1\n")[1],
            ["A, B", "C", "1"]
        ); // most wins
        assert_eq!(parse_csv("\"a;b\",c\n1,2\n")[0], ["a;b", "c"]); // quoted ; ignored
        assert_eq!(csv_text(b"n\xe4me".to_vec()).unwrap(), "n\u{e4}me"); // Latin-1
        assert!(csv_text("\u{e4}x".bytes().chain([0xFF]).collect()).is_err()); // mostly UTF-8
        assert!(
            csv_text(vec![0xFF, 0xFE, b'a', 0])
                .unwrap_err()
                .contains("UTF-16")
        );
    }

    #[test]
    fn a_text_null_is_no_null() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        s.query_all("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'NULL'), (2, NULL);")
            .unwrap();
        let r = s.query("SELECT * FROM t ORDER BY id").unwrap();
        assert_eq!(
            (r.rows[0][1].as_deref(), r.rows[1][1].as_deref()),
            (Some("NULL"), None)
        );
        // the row whose key is the text 'NULL' is found by = 'NULL', not IS NULL
        s.query_all(
            "CREATE TABLE k(name TEXT PRIMARY KEY, n INT); INSERT INTO k VALUES ('NULL', 1);",
        )
        .unwrap();
        let k = s.query("SELECT * FROM k").unwrap();
        let mut c = Changes::default();
        c.cells.insert((0, 1), Some("2".into()));
        let sql = c
            .to_sql(Driver::Sqlite, "k", &k.columns, &k.rows, &[0])
            .unwrap();
        s.apply(&sql).unwrap();
        assert_eq!(text(&s.query("SELECT n FROM k").unwrap().rows[0][0]), "2");
        // exports: JSON string vs null, SQL 'NULL' vs NULL
        let j: serde_json::Value =
            serde_json::from_str(&export(&r, "json", Driver::Sqlite, "t")).unwrap();
        assert_eq!(
            (&j[0]["v"], &j[1]["v"]),
            (&"NULL".into(), &serde_json::Value::Null)
        );
        assert!(export(&r, "sql", Driver::Sqlite, "t").contains("VALUES ('1', 'NULL')"));
    }

    #[test]
    fn csv_keeps_empty_text_and_text_null_apart_from_null() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        s.query("CREATE TABLE t(id INT, v TEXT)").unwrap();
        let r = QueryResult {
            columns: vec!["id".into(), "v".into()],
            rows: vec![
                vec![Some("1".into()), Some("".into())],
                vec![Some("2".into()), Some("NULL".into())],
                vec![Some("3".into()), None],
            ],
            ..Default::default()
        };
        s.apply(&csv_inserts(Driver::Sqlite, "t", &to_csv(&r)).unwrap())
            .unwrap();
        let back = s.query("SELECT v FROM t ORDER BY id").unwrap().rows;
        assert_eq!(
            back,
            [
                vec![Some("".to_string())],
                vec![Some("NULL".into())],
                vec![None]
            ]
        );
    }

    #[test]
    fn literals_escape_per_dialect() {
        assert_eq!(Driver::Sqlite.literal(r"it's C:\t"), r"'it''s C:\t'");
        assert_eq!(Driver::Postgres.literal(r"a\"), r"'a\'");
        assert_eq!(Driver::Mysql.literal(r"it's C:\t\"), r"'it''s C:\\t\\'");
    }

    #[test]
    fn grid_changes_become_sql_and_apply() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        s.query("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, n INT); INSERT INTO t VALUES (1,'a',1),(2,'b''s',2),(3,'c',3);").unwrap();
        let r = s.query("SELECT * FROM t ORDER BY id").unwrap();
        let mut rows = r.rows.clone();
        rows.push(vec![None; 3]); // added in the grid
        let mut c = Changes::default();
        c.cells.insert((1, 1), Some("it's".into()));
        c.cells.insert((1, 2), None);
        c.deleted.insert(2);
        c.inserted.insert(3);
        c.cells.insert((3, 1), Some("new".into()));
        assert_eq!(c.count(), 3);
        let sql = c
            .to_sql(Driver::Sqlite, "t", &r.columns, &rows, &[0])
            .unwrap();
        assert_eq!(
            sql,
            [
                "DELETE FROM \"t\" WHERE \"id\" = '3';",
                "INSERT INTO \"t\" (\"name\") VALUES ('new');",
                "UPDATE \"t\" SET \"name\" = 'it''s', \"n\" = NULL WHERE \"id\" = '2';",
            ]
        );
        s.apply(&sql).unwrap();
        let after = s.query("SELECT name, n FROM t ORDER BY id").unwrap();
        assert_eq!(
            texts(&after.rows),
            [
                vec!["a".to_string(), "1".into()],
                vec!["it's".into(), "NULL".into()],
                vec!["new".into(), "NULL".into()]
            ]
        );
        // A failing statement rolls everything back.
        assert!(s.apply(&["DELETE FROM t".into(), "nope".into()]).is_err());
        assert_eq!(
            text(&s.query("SELECT count(*) FROM t").unwrap().rows[0][0]),
            "3"
        );
        // A row gone meanwhile is an error, and nothing else is kept.
        let e = s.apply(&[
            "INSERT INTO t(name) VALUES ('x')".into(),
            "UPDATE t SET n = 1 WHERE id = 99".into(),
        ]);
        assert!(e.unwrap_err().contains("not found"));
        assert_eq!(
            text(&s.query("SELECT count(*) FROM t").unwrap().rows[0][0]),
            "3"
        );
        // In the user's transaction a failing batch takes back its earlier statements only.
        s.begin().unwrap();
        s.query("INSERT INTO t(name) VALUES ('kept')").unwrap();
        let bad = ["INSERT INTO t(name) VALUES ('x')".into(), "nope".into()];
        assert!(
            s.apply_in(&bad, false)
                .unwrap_err()
                .contains("none of them kept")
        );
        s.query("COMMIT").unwrap();
        assert_eq!(
            text(&s.query("SELECT count(*) FROM t").unwrap().rows[0][0]),
            "4"
        );
        // Not in a transaction after all (COMMIT typed in a script): still all or nothing,
        // and no transaction is left open.
        assert!(s.apply_in(&bad, false).is_err());
        assert!(s.query("COMMIT").is_err(), "no transaction is active");
        assert_eq!(
            text(&s.query("SELECT count(*) FROM t").unwrap().rows[0][0]),
            "4"
        );
        // No primary key: updates are refused, inserts are fine.
        assert!(
            c.to_sql(Driver::Sqlite, "t", &r.columns, &rows, &[])
                .is_err()
        );
        assert!(s.ddl("t").unwrap().starts_with("CREATE TABLE t("));
    }

    #[test]
    fn sqlite_query_can_be_cancelled() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        let cfg = ConnConfig {
            name: "m".into(),
            driver: Driver::Sqlite,
            host: String::new(),
            port: 0,
            database: String::new(),
            user: String::new(),
            password: String::new(),
            manual_commit: false,
            production: false,
            read_only: false,
            ..Default::default()
        };
        let c = s.canceller(&cfg);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c.cancel().unwrap();
        });
        let r = s.query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT count(*) FROM n");
        t.join().unwrap();
        assert!(r.unwrap_err().contains("interrupt"));
        assert!(s.query("SELECT 1").is_ok()); // the session is still usable
    }

    #[test]
    fn csv_roundtrip_and_import() {
        let r = QueryResult {
            columns: vec!["id".into(), "name".into()],
            rows: cells(vec![vec!["1", "a, \"b\"\nc"], vec!["2", "NULL"]]),
            ..Default::default()
        };
        let parsed = parse_csv(&to_csv(&r));
        assert_eq!(parsed[1], ["1", "a, \"b\"\nc"]);
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        s.query("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        s.apply(&csv_inserts(Driver::Sqlite, "t", &to_csv(&r)).unwrap())
            .unwrap();
        assert_eq!(
            s.query("SELECT name IS NULL FROM t WHERE id = 2")
                .unwrap()
                .rows[0][0]
                .as_deref()
                .unwrap_or("NULL"),
            "1"
        );
        assert!(
            csv_inserts(Driver::Sqlite, "t", "id,name\n1\n")
                .unwrap_err()
                .contains("line 2")
        );
    }

    #[test]
    fn export_formats() {
        let r = QueryResult {
            columns: vec!["a".into(), "b".into()],
            rows: cells(vec![vec!["1", "NULL"], vec!["x|y", "it's"]]),
            ..Default::default()
        };
        assert_eq!(
            export(&r, "md", Driver::Sqlite, "t"),
            "| a | b |\n| --- | --- |\n| 1 | NULL |\n| x\\|y | it's |\n"
        );
        assert_eq!(
            export(&r, "SQL", Driver::Mysql, "t"),
            "INSERT INTO `t` (`a`, `b`) VALUES ('1', NULL);\nINSERT INTO `t` (`a`, `b`) VALUES ('x|y', 'it''s');\n"
        );
        let j: serde_json::Value =
            serde_json::from_str(&export(&r, "json", Driver::Sqlite, "t")).unwrap();
        assert_eq!(j[0]["b"], serde_json::Value::Null);
        let dup = QueryResult {
            columns: vec!["id".into(), "id".into()],
            rows: cells(vec![vec!["1", "2"]]),
            ..Default::default()
        };
        let j: serde_json::Value =
            serde_json::from_str(&export(&dup, "json", Driver::Sqlite, "t")).unwrap();
        assert_eq!((&j[0]["id"], &j[0]["id_2"]), (&"1".into(), &"2".into()));
        let clash = QueryResult {
            columns: vec!["id".into(), "id".into(), "id_2".into()],
            rows: cells(vec![vec!["1", "2", "3"]]),
            ..Default::default()
        };
        let j: serde_json::Value =
            serde_json::from_str(&export(&clash, "json", Driver::Sqlite, "t")).unwrap();
        assert_eq!(j[0].as_object().unwrap().len(), 3, "{j}"); // no value lost
        assert_eq!(
            export(&r, "tsv", Driver::Sqlite, "t"),
            "a\tb\n1\tNULL\nx|y\tit's\n"
        );
        assert_eq!(
            export(&r, "txt", Driver::Sqlite, "t"),
            "a   | b\n----+-----\n1   | NULL\nx|y | it's\n"
        );
        let wide = QueryResult {
            columns: vec!["n".into(), "x".into()],
            rows: cells(vec![vec!["日本", "1"], vec!["ab", "2"]]),
            ..Default::default()
        };
        // 日本 is four columns wide on screen: the | lines up
        assert_eq!(
            export(&wide, "txt", Driver::Sqlite, "t"),
            "n    | x\n-----+--\n日本 | 1\nab   | 2\n"
        );
    }

    #[test]
    fn row_counts() {
        let mut s = Session::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        s.query("CREATE TABLE a(x); CREATE TABLE b(x); INSERT INTO b VALUES (1),(2);")
            .unwrap();
        let r = s
            .query(&row_counts_sql(Driver::Sqlite, &["a".into(), "b".into()]))
            .unwrap();
        assert_eq!(
            texts(&r.rows),
            [
                vec!["b".to_string(), "2".into()],
                vec!["a".into(), "0".into()]
            ]
        );
    }

    #[test]
    fn generated_sql() {
        let cols = [("id".to_string(), true), ("name".to_string(), false)];
        assert_eq!(
            generate(Driver::Mysql, "t", &cols, "update").unwrap(),
            "UPDATE `t`\nSET `name` = ?\nWHERE `id` = ?;"
        );
        assert_eq!(
            generate(Driver::Postgres, "public.t", &cols, "insert").unwrap(),
            "INSERT INTO \"public\".\"t\" (\"id\", \"name\")\nVALUES (?, ?);"
        );
        assert!(generate(Driver::Sqlite, "t", &cols, "nope").is_err());
    }

    #[test]
    fn passwords_live_in_the_keychain_not_the_file() {
        // secrets:: uses an in-memory store in tests; the file goes to a temp dir.
        let file = std::env::temp_dir().join(format!("rb-conns-{}.json", std::process::id()));
        unsafe { std::env::set_var("RUSTY_BEAVER_CONFIG", &file) };
        let list = vec![
            ConnConfig {
                id: "a".into(),
                name: "pg".into(),
                driver: Driver::Postgres,
                password: "s3cret".into(),
                ..Default::default()
            },
            ConnConfig {
                id: "b".into(),
                name: "vault".into(),
                driver: Driver::Postgres,
                auth: Auth::VaultDynamic,
                vault_path: "database/creds/ro".into(),
                ..Default::default()
            },
            ConnConfig {
                id: "c".into(),
                name: "none".into(),
                ..Default::default()
            },
        ];
        assert!(save_connections(&list).unwrap().is_empty());
        let raw = std::fs::read_to_string(&file).unwrap();
        assert!(!raw.contains("s3cret"), "the password is not in the file");
        assert!(
            raw.contains("database/creds/ro") && raw.contains("vault_dynamic"),
            "Vault paths are no secrets"
        );
        let (back, resave, _) = load_connections();
        assert!(!resave);
        assert_eq!(
            back.iter().map(|c| c.password.as_str()).collect::<Vec<_>>(),
            ["s3cret", "", ""]
        );
        // An old file with a plain-text password: loads, and asks to be saved (into the keychain).
        std::fs::write(
            &file,
            r#"[{"name":"old","driver":"mysql","password":"plain"}]"#,
        )
        .unwrap();
        let (old, resave, _) = load_connections();
        assert!(resave && !old[0].id.is_empty() && old[0].password == "plain");
        save_connections(&old).unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains("plain"));
        forget_password(&old[0]);
        assert_eq!(secrets::load(&old[0].id).unwrap(), None);
        // A new connections file is for its owner only (it can hold passwords).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::remove_file(&file);
            save_connections(&list).unwrap();
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        // A file that does not parse is moved aside, not overwritten by the next save.
        std::fs::write(&file, r#"[{"name":"typo",}]"#).unwrap();
        let (_, _, note) = load_connections();
        let aside = file.with_extension("json.invalid");
        assert!(note.unwrap().contains("did not load"));
        assert_eq!(
            std::fs::read_to_string(&aside).unwrap(),
            r#"[{"name":"typo",}]"#
        );
        let _ = std::fs::remove_file(&aside);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn empty_postgres_fields_are_not_sent() {
        let mut c = ConnConfig {
            name: "p".into(),
            driver: Driver::Postgres,
            host: "db".into(),
            port: 0,
            database: String::new(),
            user: "u".into(),
            password: String::new(),
            manual_commit: false,
            production: false,
            read_only: false,
            ..Default::default()
        };
        let cfg = pg_config(&c);
        assert_eq!(
            (
                cfg.get_dbname(),
                cfg.get_user(),
                cfg.get_password(),
                cfg.get_ports()
            ),
            (None, Some("u"), None, &[5432][..])
        );
        c.database = "postgres".into();
        assert_eq!(pg_config(&c).get_dbname(), Some("postgres"));
        c.host = "/var/run/postgresql".into();
        c.ssl = Ssl::Require;
        assert_eq!(
            pg_config(&c).get_ssl_mode(),
            postgres::config::SslMode::Disable
        );
    }

    #[test]
    fn read_only_is_enforced_by_the_server() {
        let mut s = connect(&ConnConfig {
            database: ":memory:".into(),
            read_only: true,
            ..Default::default()
        })
        .unwrap();
        assert!(s.query("SELECT 1").is_ok());
        assert!(s.query("CREATE TABLE t(a)").is_err());
    }

    #[test]
    fn write_detection() {
        assert!(!is_write(
            "SELECT 'drop table x', replace(name, 'a', 'b') FROM t -- delete\n/* update */"
        ));
        assert!(is_write("with x as (select 1) delete from t"));
        assert!(is_write("SELECT 1; Update t SET a = 1"));
        assert!(!is_write("EXPLAIN SELECT * FROM updates_log"));
        assert!(is_write("REPLACE INTO t VALUES (1)"));
        assert!(is_write("SELECT $a$'$a$; DELETE FROM t; --'"));
        assert!(!is_write("SELECT $$ delete $$, $1"));
        // MySQL and E'' backslash escapes, MySQL # comments; PG's #- operator is no comment
        assert!(is_write(
            r"SELECT * FROM t WHERE n = 'O\'Brien'; DELETE FROM t"
        ));
        assert!(is_write(r"SELECT E'it\'s'; DELETE FROM t; --'"));
        assert!(is_write("SELECT 1 # don't\nDELETE FROM t"));
        assert!(is_write("SELECT j #- '{a}' FROM t; DELETE FROM t"));
        assert!(!is_write(r"SELECT 'a\', 'b' -- update"));
        assert!(is_write("SELECT * INTO newt FROM t"));
        assert!(!is_write("SELECT id, comment, lock FROM reviews")); // column names
        for maintenance in [
            "REFRESH MATERIALIZED VIEW mv",
            "ANALYZE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
            "FLUSH TABLES",
        ] {
            assert!(is_write(maintenance), "{maintenance}");
        }
        assert!(is_write("SELECT 1; COMMENT ON TABLE t IS 'x'"));
    }

    #[test]
    fn query_parameters() {
        let sql =
            "SELECT ':skip', x::int FROM t -- :nope\nWHERE a = :rank AND b > :min AND c = :rank";
        assert_eq!(params(sql), ["rank", "min"]);
        assert_eq!(params("SELECT $$it's :no$$, :id"), ["id"]);
        assert_eq!(
            params("SELECT arr[2:n], arr[i:j] FROM t WHERE a = :id"),
            ["id"]
        );
        assert_eq!(split_order("a = 1 order by b"), ("a = 1 ", "order by b"));
        assert_eq!(
            pg_capped("SELECT * FROM t -- all\n;").unwrap(),
            format!(
                "SELECT * FROM (\nSELECT * FROM t -- all\n\n) AS rusty_beaver LIMIT {}",
                MAX_ROWS + 1
            )
        );
        for not in [
            "SELECT 1; SELECT 2",
            "SELECT * INTO t2 FROM t",
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            "SHOW search_path",
            "SELECT 1; -- note",
            "SELECT * FROM jobs WHERE state = 'new' FOR UPDATE",
            "SELECT * FROM t FOR SHARE",
        ] {
            assert_eq!(pg_capped(not), None, "{not}");
        }
        let h = |sql| statement_heads(sql, true);
        assert_eq!(
            h("-- note\nINSERT INTO t VALUES (';'); create temporary table x(a); COMMIT"),
            [
                ("INSERT".to_string(), "INTO".to_string()),
                ("CREATE".into(), "TEMPORARY".into()),
                ("COMMIT".into(), String::new())
            ]
        );
        assert_eq!(split_order("a = 'order by'"), ("a = 'order by'", ""));
        let sub = "id IN (SELECT id FROM x ORDER BY ts DESC LIMIT 5)";
        assert_eq!(split_order(sub), (sub, "")); // the subquery's own ORDER BY
        assert_eq!(
            Driver::Sqlite.select_all("t", "ORDER BY b", 5),
            "SELECT * FROM \"t\" ORDER BY b LIMIT 5;"
        );
        let v: std::collections::HashMap<String, String> = [
            ("rank".to_string(), "'Private'".to_string()),
            ("min".into(), "3".into()),
        ]
        .into();
        assert_eq!(
            bind(sql, &v).unwrap(),
            "SELECT ':skip', x::int FROM t -- :nope\nWHERE a = 'Private' AND b > 3 AND c = 'Private'"
        );
        assert!(
            bind("SELECT :x", &Default::default())
                .unwrap_err()
                .contains(":x")
        );
    }

    #[test]
    fn statement_under_cursor() {
        let sql = "SELECT 1;\nSELECT ';' AS semi -- ; comment\n  FROM t;\n\nUPDATE t SET a = 1\n\nSELECT 3";
        let at = |o: usize| &sql[statement_at(sql, o, false)];
        assert_eq!(at(0), "SELECT 1;");
        assert_eq!(at(9), "SELECT 1;"); // right after the ;
        assert_eq!(at(12), "SELECT ';' AS semi -- ; comment\n  FROM t;");
        assert_eq!(at(sql.find("UPDATE").unwrap() + 3), "UPDATE t SET a = 1");
        assert_eq!(at(sql.len()), "SELECT 3");
        assert_eq!(
            at(sql.find("\n\nUPDATE").unwrap() + 1),
            "SELECT ';' AS semi -- ; comment\n  FROM t;"
        ); // blank line gap
        assert_eq!(&"  "[statement_at("  ", 1, false)], "");
        let f = "CREATE FUNCTION f() RETURNS int AS $body$\nBEGIN\n  PERFORM 1;\n\n  RETURN 1;\nEND $body$ LANGUAGE plpgsql;";
        assert_eq!(&f[statement_at(f, 3, false)], f);
        let m = r"SELECT 'O\'Brien;'; SELECT 2";
        assert_eq!(&m[statement_at(m, 3, true)], r"SELECT 'O\'Brien;';");
    }

    #[test]
    fn demo_sql_loads() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("demo.sql")).unwrap();
    }
}
