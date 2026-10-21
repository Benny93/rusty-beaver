//! Script files and the session file that brings open scripts back after a restart.

use super::*;

impl AppView {
    /// Mod+/: comment or uncomment the lines of the selection (or the cursor's line).
    pub(super) fn toggle_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self
            .tabs
            .get(self.active)
            .filter(|t| t.kind == TabKind::Sql)
        else {
            return;
        };
        let (text, range, pos) = {
            let e = t.editor.read(cx);
            (
                e.value().to_string(),
                e.selected_range(),
                e.cursor_position(),
            )
        };
        let new = crate::format::toggle_comment(&text, range);
        t.editor.update(cx, |e, cx| {
            e.set_value(new, window, cx);
            e.set_cursor_position(pos, window, cx);
        });
        self.save_session_soon(cx);
    }

    /// Close every tab, or every tab but the active one.
    pub(super) fn close_tabs(&mut self, keep_active: bool, cx: &mut Context<Self>) {
        let keep = keep_active.then_some(self.active);
        let mut i = 0;
        self.tabs.retain(|_| {
            let k = Some(i) == keep;
            i += 1;
            k
        });
        self.active = 0;
        self.save_session_soon(cx);
        cx.notify();
    }

    pub(super) fn select_tab_ix(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i < self.tabs.len() {
            self.active = i;
            self.ensure_loaded(i, window, cx);
            cx.notify();
        }
    }

    /// Load a table / diagram tab restored from the last session, the first time it is shown.
    pub(super) fn ensure_loaded(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.tabs.get_mut(i).filter(|t| t.stale) else {
            return;
        };
        t.stale = false;
        let (conn, kind) = (t.conn, t.kind.clone());
        match kind {
            TabKind::Table(name) => {
                self.open_table(conn, name, window, cx, Box::new(|_, _, _, _| {}))
            }
            TabKind::Diagram => self.open_diagram(conn, window, cx, Box::new(|_, _, _, _| {})),
            TabKind::Sql => {}
        }
    }

    /// Write the open scripts to `session.json` a second after the last change.
    pub(super) fn save_session_soon(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.session_dirty, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |v, cx| {
                v.session_dirty = false;
                let scripts: Vec<Value> = v
                    .tabs
                    .iter()
                    .map(|t| {
                        let conn = v.conn_ix(t.conn).map(|i| v.conns[i].cfg.name.clone());
                        match &t.kind {
                            TabKind::Sql => json!({ "connection": conn, "sql": t.editor.read(cx).value().to_string(), "path": t.path }),
                            TabKind::Table(name) => json!({ "connection": conn, "table": name, "filter": t.filter.read(cx).value().to_string() }),
                            TabKind::Diagram => json!({ "connection": conn, "er_diagram": true }),
                        }
                    })
                    .collect();
                let path = session_path();
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
                if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&json!({ "scripts": scripts })).unwrap()) {
                    v.status = format!("Could not save open scripts: {e}");
                }
            });
        })
        .detach();
    }

    /// Reopen the scripts of the last session.
    pub(super) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(text) = std::fs::read_to_string(session_path()) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        for s in v["scripts"].as_array().into_iter().flatten() {
            // A table or diagram of a deleted connection is gone with it; a script keeps its
            // text, on the first connection.
            let conn = match s["connection"].as_str() {
                Some(n) => self.conn_by_name(n).ok(),
                None => self.conns.first().map(|c| c.id),
            };
            // Table and diagram tabs come back empty and load when first selected.
            if let (Some(conn), Some(table)) = (conn, s["table"].as_str()) {
                let tab = self.open_tab(
                    conn,
                    TabKind::Table(table.to_string()),
                    String::new(),
                    window,
                    cx,
                );
                let filter = s["filter"].as_str().unwrap_or_default().to_string();
                let t = &mut self.tabs[tab];
                t.filter.update(cx, |i, cx| i.set_value(filter, window, cx));
                (t.stale, t.message) = (true, Some(Ok("Select the tab to load it".into())));
                continue;
            }
            if let (Some(conn), true) = (conn, s["er_diagram"].as_bool() == Some(true)) {
                let tab = self.open_tab(conn, TabKind::Diagram, String::new(), window, cx);
                self.tabs[tab].stale = true;
                continue;
            }
            if s["table"].is_null()
                && s["er_diagram"].is_null()
                && let Some(conn) = conn.or(self.conns.first().map(|c| c.id))
            {
                let tab = self.open_tab(
                    conn,
                    TabKind::Sql,
                    s["sql"].as_str().unwrap_or_default().to_string(),
                    window,
                    cx,
                );
                if let Some(p) = s["path"].as_str() {
                    let p = std::path::PathBuf::from(p);
                    self.tabs[tab].title = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.tabs[tab].path = Some(p);
                }
            }
        }
        if !self.tabs.is_empty() {
            // Start on a script: table and diagram tabs connect only when selected.
            self.active = self
                .tabs
                .iter()
                .position(|t| t.kind == TabKind::Sql)
                .unwrap_or(0);
            self.status = format!("Restored {} tab(s) from the last session", self.tabs.len());
        }
    }

    /// Open a .sql file in a new script tab on the selected connection.
    pub(super) fn open_script(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let tab = self.new_editor(None, window, cx)?;
        let t = &mut self.tabs[tab];
        t.editor.update(cx, |e, cx| e.set_value(text, window, cx));
        t.title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        t.path = Some(path.to_path_buf());
        self.status = format!("Opened {}", path.display());
        cx.notify();
        Ok(tab)
    }

    /// Write a script tab to `path` (or the file it came from).
    pub(super) fn save_script(
        &mut self,
        tab: usize,
        path: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let t = self.tabs.get_mut(tab).ok_or("no tab open")?;
        if t.kind != TabKind::Sql {
            return Err("only SQL editor tabs are scripts".into());
        }
        let path = path
            .or(t.path.clone())
            .ok_or("no file yet: use Save Script As")?;
        std::fs::write(&path, t.editor.read(cx).value().as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        t.title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        t.path = Some(path.clone());
        self.save_session_soon(cx); // the path comes back after a restart
        let msg = format!("Saved {}", path.display());
        self.status = msg.clone();
        cx.notify();
        Ok(msg)
    }

    /// Ask where to save the active script.
    pub(super) fn save_script_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.active;
        let name = self
            .tabs
            .get(tab)
            .map(|t| format!("{}.sql", t.title.replace(['<', '>', ' ', '/'], "")))
            .unwrap_or_default();
        let dir = self
            .tabs
            .get(tab)
            .and_then(|t| t.path.as_ref()?.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let key = self.tab_key(tab);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    let Some(tab) = key.and_then(|k| v.tab_ix(k)) else {
                        v.status = "The script tab was closed".into();
                        return;
                    };
                    if let Err(e) = v.save_script(tab, Some(path), cx) {
                        v.status = e;
                    }
                });
            }
        })
        .detach();
    }
}
