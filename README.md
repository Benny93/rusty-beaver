# Rusty Beaver

A DBeaver-style database tool written in Rust with [GPUI](https://gpui.rs/) (via gpui-kit). It has a built-in **MCP server**, so AI agents can drive the UI and take screenshots of it. Agents use semantic commands, so there is no simulated mouse input.

![Rusty Beaver tour](docs/tour.gif)

*An AI agent drove every step of this tour through the app's MCP server. Regenerate it with `make db-up screenshots`.*

| | |
| --- | --- |
| ![Splash](docs/screenshots/01-splash.png) **Pixel-art splash screen** | ![Navigator](docs/screenshots/02-navigator.png) **Database Navigator**: tables, views, columns, keys, foreign keys |
| ![ER diagram](docs/screenshots/03-er-diagram.png) **ER diagram** with draggable tables | ![Editing](docs/screenshots/04-edit-data.png) **Edit data in the grid**: change, add and delete rows, then save in one transaction |
| ![Columns](docs/screenshots/05-table-columns.png) **Columns** with types, keys and references, plus a **DDL** view | ![Filter](docs/screenshots/06-filter.png) **Filter** rows with a WHERE condition; 200 rows per page |
| ![SQL editor](docs/screenshots/07-sql-editor.png) **SQL editor**: highlighting, autocompletion, statement under the cursor | ![Chart](docs/screenshots/08-chart.png) **Charts** from any result |
| ![Scripts](docs/screenshots/09-scripts.png) **Scripts**: one result per statement | ![Value panel](docs/screenshots/10-value-panel.png) **Value panel**: long text and pretty-printed JSON |
| ![Parameters](docs/screenshots/11-parameters.png) **Query parameters** (`:name`) | ![Format](docs/screenshots/12-format.png) **Format SQL** and **Explain** plans |
| ![SQL error](docs/screenshots/13-sql-error.png) **SQL errors** shown inline | ![History](docs/screenshots/14-history.png) **Query history**, searchable and kept across restarts |
| ![New connection](docs/screenshots/15-new-connection.png) **New / Edit Connection** dialog | ![Databases](docs/screenshots/16-multi-database.png) **SQLite, PostgreSQL and MySQL** side by side |
| ![Light theme](docs/screenshots/17-light-theme.png) **Light theme** (remembered) | ![Production](docs/screenshots/18-production.png) **Production connections** ask before changing data |

## Features

- **Drivers**: SQLite (bundled), PostgreSQL and MySQL/MariaDB. PostgreSQL errors show the server's message, detail and hint.
- **Database Navigator** (shows the server version after connecting): connections, then tables and views, then columns with primary-key and foreign-key markers (`→ beavers.id`). Status is shown in colour. A filter box narrows the table list. Right-click a connection for Connect, New SQL Editor, ER Diagram, Table Row Counts, Refresh, Edit and Delete. Right-click a table for Open, View DDL, Generate SELECT/INSERT/UPDATE/DELETE, Count Rows, Import CSV and Copy Name.
- **SQL editor**:
  - tree-sitter highlighting.
  - Autocompletion of keywords, tables and columns. It is alias-aware after `b.`.
  - Cmd/Ctrl+Enter runs the statement under the cursor or the selection. Statements end at `;` or at a blank line.
  - Cmd/Ctrl+Shift+Enter runs the whole script, with one result tab per statement.
  - **Explain**, **Format SQL** and **cancel** a running query (with a live timer).
  - `:name` **parameters** are asked for in a dialog before the statement runs.
  - Open and save `.sql` files. Open tabs come back after a restart: scripts with their text, and table tabs (with their filter) and ER diagrams, which load when first selected.
  - Mod+/ comments or uncomments lines. Mod+1 … Mod+9 (Alt on Linux/Windows) switches tabs.
- **Result grid**:
  - Virtualised, sortable by clicking a column header (numbers sort as numbers).
  - **Record view** shows one row as a form, and a **Text view** shows the result as an aligned plain-text table. The **value panel** shows long text, pretty-prints JSON, and has column statistics (NULLs, distinct values, min, max, sum, average). **Charts** draw the result as bars.
  - **Search results**: a quick text search narrows the rows of any result.
  - Copy a value, a row, the row as INSERT, a column as an SQL `IN (…)` list, or the whole grid as Markdown.
  - **Export** from the results toolbar or the grid's right-click menu (*Copy All As*, *Export to File*): CSV, TSV, JSON, Markdown, plain text or SQL INSERTs, to the clipboard or a file. Table tabs export the pages that are loaded, and the message says so when the table has more rows.
- **Table tabs**: double-click a table.
  - The *Data*, *Columns* and *DDL* views. The data has a WHERE filter, 200 rows per page with "Load more", and a total row count.
  - **Edit the data in place**: double-click or F2 to edit a cell, set NULL, add, duplicate and delete rows. Changes stay highlighted until you **Save**. Save runs UPDATE/INSERT/DELETE by primary key in one transaction, so it all goes through or nothing does. You can also **Revert**.
  - Right-click a cell to *Filter by / Exclude this value*, or *Open Referenced Row*, which follows a foreign key to the parent table, filtered to that row.
- **Transactions**: switch a connection between auto-commit and **manual commit**. In manual commit, the first change opens a transaction ("smart commit"), and reads run inside it. Commit and Rollback live in the toolbar, and grid saves and CSV imports join the open transaction.
- **Production connections** (a toggle in the connection dialog) get a red PROD tag and a warning banner. SQL that changes data or schema, and grid saves, ask for confirmation first. Agents have to pass `confirm: true`.
- **Read-only connections** (an RO tag) refuse SQL that changes data or schema, grid saves and imports outright.
- **Reconnects by itself** when a server restarts or the network drops. The statement is retried once, except inside an open manual-commit transaction, which the server has already rolled back.
- **ER diagram** for each connection: tables with keys, foreign-key links between them, and boxes you can drag.
- **Query history** (last 500 statements) with duration and outcome. Click an entry to reopen it.
- **CSV import** into a table. The header row names the columns, and the import is one transaction.
- **Connections**: New and Edit Connection dialog with *Test Connection*, and three explicit ways to sign in: a password kept in the OS keychain, a HashiCorp Vault secret, or Vault dynamic credentials (with OpenID Connect sign-in). See [Connecting and signing in](#connecting-and-signing-in).
- **Dark and light themes**, or follow the system appearance (the choice is remembered), native menus, a keyboard shortcut overview (F1).
- **Splash screen** and a **Metal Slug style pixel-art icon** (a beaver soldier hugging a database), on macOS, Windows and Linux.
- **First start**: a demo SQLite database (beavers, lodges, missions) is created, so there is something to click on right away.

## Connecting and signing in

Click **New Connection**, pick the database type, then fill in the dialog's three sections:

1. **Connection**: name, host, port and database (for SQLite: the database file, created if it is missing).
   For PostgreSQL and MySQL, pick the **TLS** mode: *Prefer* (default) uses TLS when the server offers it, *Require* insists on it, *Verify certificate* also checks the certificate against the system's trusted CAs, *Off* never uses it. *Prefer* and *Require* encrypt but don't check the certificate, like libpq. Poolers such as Supavisor find the tenant from the host name sent over TLS; with TLS off they need it in the user name (`user.tenant`).
2. **Authentication**: pick one mode with the buttons. Each mode shows only the fields it needs.
3. **Safety**: *Production* asks before changes, *Read-only* refuses them.

Click **Test Connection** to try the settings, then **Finish** to save and connect.

### Authentication modes

| Mode | Use it when | Fill in | Saved locally |
|------|-------------|---------|---------------|
| **Password** | you have a database user and password | Username, Password | the password in the OS keychain; the rest in `connections.json` |
| **Vault secret** | the password is stored in a Vault KV secret | Username, Secret path (`secret/data/team/db`), Field (default `password`), Vault address | no secret: the password is read from Vault on every connect |
| **Vault dynamic credentials** | Vault's database secrets engine hands out temporary users | Credentials path (`database/creds/<role>`), Vault address | no secret: a fresh user name and password on every connect |

**Password.** The password goes to the macOS Keychain, the Windows Credential Manager, or the Secret Service on Linux (GNOME Keyring, KWallet), in one item (service `rusty-beaver`, account `passwords`) for all connections; `connections.json` never holds it. Passwords from older versions move there on the next start. On macOS, the first start asks once for access to that item: choose **Always Allow**. Every new build of the app is a new app to macOS, so it asks once again after an update or a local rebuild. With `RUSTY_BEAVER_KEYCHAIN=off` (for automation) they stay in the file.

**Vault secret.** For KV v2 the path includes `data/`: a secret you write with `vault kv put secret/team/db password=…` is read from `secret/data/team/db`. KV v1 paths have no `data/`.

**Vault dynamic credentials.** The path is the role's credentials endpoint, e.g. `database/creds/readonly`. There is no username field: Vault creates the user.

**Signing in to Vault** (both Vault modes). The Vault address is the one in the dialog, else `VAULT_ADDR`. The app then uses the first token that works:

1. the token from an earlier sign-in in this session,
2. `VAULT_TOKEN`,
3. `~/.vault-token`, which `vault login` writes,
4. otherwise an **OpenID Connect** sign-in in your browser, exactly like `vault login -method=oidc`. *OIDC role* is optional (empty: the mount's default role) and *Auth mount* defaults to `oidc`. The callback is `http://localhost:8250/oidc/callback`, so roles set up for the Vault CLI work as they are.

Vault's certificate is checked against the system's trusted CAs, so company CAs installed on the machine work. Tokens are kept in memory only. If Vault answers "permission denied", the app signs in once more and retries.

Agents set the same fields over MCP: `add_connection` and `edit_connection` take `auth` (`password`, `vault_kv`, `vault_dynamic`), `ssl` (`prefer`, `require`, `verify_full`, `disable`), `vault_path`, `vault_field`, `vault_addr`, `vault_role` and `vault_mount`.

## Keyboard shortcuts

Mod is Cmd on macOS and Ctrl elsewhere. F1 shows this list in the app.

| Keys | Action |
| --- | --- |
| Mod+Enter | Execute the statement at the cursor (or the selection) |
| Mod+Shift+Enter (Alt+X on Linux/Windows) | Execute the whole script |
| Mod+. | Cancel the running query |
| Mod+Shift+E / Mod+Shift+F | Explain plan / Format SQL |
| Mod+T / Mod+N / Mod+W | New SQL editor / New connection / Close tab |
| Mod+O / Mod+S / Mod+Shift+S | Open / Save / Save script as. In a table tab, Mod+S saves the grid changes |
| F2, double-click | Edit a cell (table tabs) |
| Cmd+Backspace, Ctrl+Delete | Delete a row (table tabs) |
| Mod+C / Mod+Shift+C | Copy value / row |
| Mod+/ | Comment / uncomment lines |
| Mod+1 … Mod+9 | Switch tabs (Alt+1 … 9 on Linux/Windows) |
| F1, Mod+Shift+/ | Keyboard shortcuts |

## Run

```sh
make run          # optimized build + run (MCP on http://127.0.0.1:7979/mcp)
make dev          # debug build + run
make start/stop   # run in the background (pid + log in target/)
make test         # unit tests
make smoke        # end-to-end: drives every feature over MCP against a throwaway database
make screenshots  # docs/screenshots/*.png + docs/tour.gif, driven over MCP
make clean        # stop the app, remove build output (target/)
make distclean    # also this project's Docker images and build caches
make help         # all targets
```

Linux build dependencies (Debian/Ubuntu): `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libxcb-randr0-dev libfontconfig-dev libfreetype-dev libvulkan-dev libasound2-dev libzstd-dev libpipewire-0.3-dev libclang-dev libdbus-1-dev libegl-dev libgbm-dev pkg-config`. See `scripts/Dockerfile`, or run `make linux-docker`.

Windows: `cargo build --release` produces `target\release\rusty-beaver.exe` with the icon embedded. `make windows-docker` cross-builds it with mingw.
macOS: `make bundle` builds `RustyBeaver.app`, and `make install` puts it into `~/Applications`.

To test against real servers, `make db-up` starts PostgreSQL on port 55432 and MySQL on port 53306 (user `postgres`/`root`, password `beaver`). `make db-down` stops them.

## Releases

`make release` runs the tests, then tags the version in `Cargo.toml` (`v0.1.0`, …) and pushes the tag. `.github/workflows/release.yml` then builds the installers with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) and attaches them to a GitHub release:

| Platform | Installer |
| --- | --- |
| macOS (Apple Silicon and Intel) | `.dmg` |
| Windows | NSIS `.exe` (C runtime linked statically, so no VC++ redistributable is needed) |
| Linux | `.deb` and `.AppImage` |

`cargo packager --release --formats dmg` (or `nsis`, `deb,appimage`) builds the same installers locally into `target/dist/`.

The installers are **unsigned**:
- **macOS:** Gatekeeper refuses the app on first launch. On macOS 15+ it says "…is damaged and can't be opened", and right-click → Open doesn't help. After dragging it into /Applications, run this once: `xattr -dr com.apple.quarantine "/Applications/Rusty Beaver.app"`.
- **Windows:** SmartScreen shows "unknown publisher". Click **More info → Run anyway**.

### Publishing updates

`.github/workflows/winget.yml` submits a manifest update to [winget-pkgs](https://github.com/microsoft/winget-pkgs) for every published release (identifier `Benny93.RustyBeaver`). It needs:
1. A fork of `microsoft/winget-pkgs` under the same GitHub account.
2. A classic personal access token with the `public_repo` and `workflow` scopes, stored as the repository secret `WINGET_TOKEN`.
3. A first version submitted by hand (for example with `wingetcreate new <installer url>`). The action builds each update on the existing package, so it only works once that first version is merged.

## Website

`site/` is the project's GitHub Pages site, including *Beaver Bomber*, a small Bomberman-style game in plain JavaScript (`site/game.js`; `make test` checks its rules). `make site` copies the screenshots, the tour GIF and the icons into `site/assets/`, and then you can open `site/index.html`. `.github/workflows/pages.yml` publishes it to GitHub Pages on every push to `main` that changes it, or by hand from the Actions tab. To enable it once, go to Settings → Pages → Source and choose "GitHub Actions". On a free GitHub plan the repository has to be public.

CI (`.github/workflows/ci.yml`) runs the tests and a release build on macOS, Linux and Windows for every push and pull request, and uploads the binaries as artifacts.

## MCP: let an agent drive it

The running app serves MCP (streamable HTTP) on `127.0.0.1` only. Requests whose Host or Origin header is not local are rejected. Set `RUSTY_BEAVER_MCP_PORT` (or `make run PORT=…`) to use a different port.

```sh
claude mcp add --transport http rusty-beaver http://127.0.0.1:7979/mcp   # or use the bundled .mcp.json
```

There are 54 tools. Each one changes the UI the user is looking at and returns structured data.

| Area | Tools |
| --- | --- |
| See | `get_state`, `screenshot` (warns when the screen is locked and the frame may be stale) |
| Connections | `test_connection`, `add_connection`, `edit_connection`, `delete_connection`, `connect`, `disconnect`, `refresh_connection`, `list_tables`, `connection_dialog` |
| Schema | `describe_table`, `open_er_diagram`, `generate_sql`, `row_counts` |
| SQL | `new_sql_editor`, `set_sql`, `run_sql` (`connection`, `params` for `:name`, `confirm` for production), `execute_editor` (like Mod+Enter), `cancel_query`, `explain`, `format_sql`, `sql_completions`, `open_script`, `save_script` |
| Results | `get_results` (`index` for a script's result), `sort_results`, `search_results`, `results_as_text`, `chart`, `record_view`, `show_value`, `export_csv` (csv/tsv/json/md/txt/sql by extension) |
| Table data | `open_table`, `set_table_view` (data/columns/ddl), `set_filter`, `load_more`, `edit_cell`, `add_row`, `duplicate_row`, `delete_row`, `open_referenced_row`, `save_changes` (`confirm` for production), `revert_changes`, `import_csv` |
| Transactions | `set_auto_commit`, `commit`, `rollback` |
| UI | `select_tab`, `close_tab`, `close_all_tabs`, `set_sidebar` (navigator/history), `get_history`, `set_theme` (dark/light/system), `shortcuts` (the F1 overview) |

Try it without an agent:

```sh
python3 scripts/mcp.py tools
python3 scripts/mcp.py run_sql '{"connection": "Beaver Demo (SQLite)", "sql": "SELECT * FROM leaderboard"}'
python3 scripts/mcp.py run_sql '{"connection": "Beaver Demo (SQLite)", "sql": "SELECT * FROM beavers WHERE rank = :rank", "params": {"rank": "'"'"'Private'"'"'"}}'
python3 scripts/mcp.py screenshot --png shot.png
```

Set `RUSTY_BEAVER_BACKGROUND=1` to start the app without taking keyboard focus (tests, automation). Use `RUSTY_BEAVER_CONFIG`, `RUSTY_BEAVER_HISTORY` and `RUSTY_BEAVER_SESSION` to point it at throwaway files.

On macOS, screenshots of the app's own window need no Screen Recording permission. On Linux under Wayland, xcap may ask through the desktop portal.

## Layout

```
src/main.rs      entry point
src/ui.rs        app state, window setup, splash, render root
src/ui/          connections, execute (SQL runs), editor_area, grid_ops, views (chart / value / record),
                 navigator (+ history), dialogs, diagram (ER), scripts (files, session), handle (MCP)
src/grid.rs      result grid: sorting, pending edits, chart data
src/db.rs        drivers (rusqlite / postgres / mysql) behind one text-only API, metadata, DML from grid
                 edits, transactions, cancel, statement splitting, parameters, import/export
src/complete.rs  SQL autocompletion (keywords, tables, alias-aware columns)
src/format.rs    SQL formatter
src/er.rs        ER diagram model and layout
src/history.rs   query history (history.jsonl)
src/mcp.rs       MCP server (rmcp + axum), screenshot capture (xcap)
src/demo.sql     first-start demo database
scripts/pixel_icon.py   generates every icon size + splash from a 64×64 sprite (stdlib python)
scripts/mcp.py          tiny MCP client; scripts/smoke.py end-to-end check; scripts/screenshots.py docs tour
```

## Known limits

- Without a system keychain (for example a Linux system without the Secret Service), passwords stay in `connections.json`, and the app says so. `RUSTY_BEAVER_KEYCHAIN=off` does the same on purpose, which is how the tests run.
- Grid editing needs a primary key. Results of SQL editor queries are read-only; open the table to edit it.
- Query parameters are inserted as text (as in DBeaver), not as bound values.
- `make screenshots` needs an unlocked screen. macOS does not repaint windows while the session is locked, so the tour stops and writes nothing then.
