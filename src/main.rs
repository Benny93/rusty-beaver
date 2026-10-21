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
    let port = match mcp_port(std::env::var("RUSTY_BEAVER_MCP_PORT").ok().as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    ui::run(port);
}

/// A port that does not parse is refused: falling back to the default would have scripts
/// talk to whichever app has that port (the user's own, with real connections).
fn mcp_port(v: Option<&str>) -> Result<u16, String> {
    match v.filter(|p| !p.trim().is_empty()) {
        None => Ok(mcp::DEFAULT_PORT),
        Some(p) => p
            .trim()
            .parse()
            .map_err(|_| format!("RUSTY_BEAVER_MCP_PORT={p:?} is no port (0-65535)")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mcp_port_is_the_one_asked_for_or_refused() {
        assert_eq!(super::mcp_port(None), Ok(super::mcp::DEFAULT_PORT));
        assert_eq!(super::mcp_port(Some("")), Ok(super::mcp::DEFAULT_PORT));
        assert_eq!(super::mcp_port(Some(" 7880")), Ok(7880));
        // not the default port, where the user's own app may answer
        assert!(super::mcp_port(Some("70000")).is_err());
        assert!(super::mcp_port(Some("abc")).is_err());
    }
}
