// No console window behind the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod complete;
mod db;
mod er;
mod format;
mod grid;
mod history;
mod mcp;
mod platform;
mod secrets;
mod ui;
mod vault;

fn main() {
    // RUSTY_BEAVER_MCP_PORT=0 still binds an OS-chosen port; the toolbar shows where.
    let port = std::env::var("RUSTY_BEAVER_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(mcp::DEFAULT_PORT);
    ui::run(port);
}
