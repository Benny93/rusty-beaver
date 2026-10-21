//! Query history: every executed statement, newest last, kept in
//! `<config dir>/rusty-beaver/history.jsonl` (one JSON object per line, append-only).

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::PathBuf;

/// Entries kept in memory and shown; the file is trimmed to this on start.
pub const KEEP: usize = 500;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub sql: String,
    pub connection: String,
    /// Unix seconds.
    pub at: u64,
    pub ms: f64,
    /// "8 rows", "3 updated" or the error.
    pub outcome: String,
    pub ok: bool,
}

fn path() -> PathBuf {
    std::env::var_os("RUSTY_BEAVER_HISTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::db::config_dir().join("history.jsonl"))
}

pub fn load() -> Vec<Entry> {
    load_from(&path())
}

fn load_from(p: &std::path::Path) -> Vec<Entry> {
    // lossy: a line cut inside a character must not cost the whole history
    let text = std::fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let mut v: Vec<Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if v.len() > KEEP {
        v.drain(..v.len() - KEEP);
        save_to(p, &v); // trimmed, so the file does not grow forever
    }
    v
}

/// Replace the file with these entries (after a trim or a connection rename).
pub fn save_all(entries: &[Entry]) {
    save_to(&path(), entries)
}

fn save_to(p: &std::path::Path, entries: &[Entry]) {
    let body: String = entries
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    let _ = crate::db::write_atomic(p, body);
}

pub fn append(e: &Entry) {
    append_to(&path(), e)
}

fn append_to(p: &std::path::Path, e: &Entry) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(e).unwrap());
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "14:03" today, else "Oct 19 14:03" (UTC-free: local offset is not in std, so relative age).
pub fn age(at: u64, now: u64) -> String {
    let s = now.saturating_sub(at);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_load_and_trim() {
        let p = std::env::temp_dir().join(format!("rb-history-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let e = |i: usize| Entry {
            sql: format!("SELECT {i}"),
            connection: "c".into(),
            at: i as u64,
            ms: 1.,
            outcome: "1 rows".into(),
            ok: true,
        };
        for i in 0..KEEP + 5 {
            append_to(&p, &e(i));
        }
        let v = load_from(&p);
        assert_eq!((v.len(), v[0].sql.as_str()), (KEEP, "SELECT 5"));
        assert_eq!(load_from(&p).len(), KEEP); // file was trimmed
        let _ = std::fs::remove_file(&p);
        assert_eq!(age(100, 130), "just now");
        assert_eq!(age(0, 7300), "2 h ago");
    }
}
