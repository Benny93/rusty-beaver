//! Grid commands (copy, edit, filter, save, …) shared by the context menu, shortcuts and MCP.

use super::*;

impl AppView {
    /// Run a grid command on the active tab and show its outcome in the status bar.
    pub(super) fn report(&mut self, op: GridOp, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.grid_op(
            op,
            window,
            cx,
            Box::new(|v, r, _, cx| {
                if let Err(e) = r {
                    v.status = e;
                    cx.notify();
                }
            }),
        );
        match r {
            Ok(Some(msg)) => self.status = msg,
            Ok(None) => {}
            Err(e) => self.status = e,
        }
        cx.notify();
    }

    /// Grid commands on the active tab's selected cell. `done` only runs for Save.
    pub(super) fn grid_op(
        &mut self,
        op: GridOp,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) -> Result<Option<String>, String> {
        let ix = self.active;
        let t = self.tabs.get(ix).ok_or("no tab open")?;
        // While it loads or saves, rows are about to be replaced: a change now would land on
        // the wrong row, and a second Save would run the same statements again.
        let changes = matches!(
            op,
            GridOp::EditCell
                | GridOp::SetNull
                | GridOp::DeleteRow
                | GridOp::AddRow
                | GridOp::DuplicateRow
                | GridOp::Save
                | GridOp::Revert
        );
        if changes && t.running {
            let e = "wait until it has loaded or saved".to_string();
            if op == GridOp::Save {
                // an agent's confirm is used up here too: the user's next save asks again
                (self.tabs[ix].confirmed, self.tabs[ix].agent) = (false, false);
                done(self, Err(e.clone()), window, cx);
            }
            return Err(e);
        }
        let table = match &t.kind {
            TabKind::Table(n) => Some(n.clone()),
            _ => None,
        };
        let editable = || {
            table.clone().ok_or("query results are read-only: open the table (double-click it in the navigator) to edit rows")
        };
        let grid = t.grid.clone();
        let g = grid.read(cx).delegate();
        let cell = t
            .cell
            .filter(|(r, c)| *r < g.order.len() && *c < g.r.columns.len());
        let need_cell = || cell.ok_or("select a cell first");
        let driver = self
            .conn_ix(t.conn)
            .map(|i| self.conns[i].cfg.driver)
            .unwrap_or(Driver::Sqlite);
        match op {
            GridOp::FilterBy | GridOp::FilterExclude => {
                editable()?;
                if !g.changes.is_empty() {
                    return Err("save or revert the pending changes first".into());
                }
                let (row, col) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                let (name, v) = (driver.quote(&g.r.columns[col]), g.value(id, col));
                let cond = match (op == GridOp::FilterBy, v == "NULL") {
                    (true, true) => format!("{name} IS NULL"),
                    (false, true) => format!("{name} IS NOT NULL"),
                    (true, false) => format!("{name} = {}", driver.literal(&v)),
                    (false, false) => format!("{name} <> {}", driver.literal(&v)),
                };
                let tab = &mut self.tabs[ix];
                let old = tab.filter.read(cx).value().trim().to_string();
                // a sort stays at the end: (x = 1 ORDER BY y) AND … is no SQL
                let (old_cond, order) = db::split_order(&old);
                let filter = if old_cond.trim().is_empty() {
                    format!("{cond} {order}")
                } else {
                    format!("({}) AND {cond} {order}", old_cond.trim())
                }
                .trim()
                .to_string();
                tab.filter
                    .update(cx, |i, cx| i.set_value(filter.clone(), window, cx));
                tab.limit = PAGE;
                self.reload_table(ix, window, cx, done);
                Ok(Some(format!("Filter: {filter}")))
            }
            GridOp::CopyColumnIn => {
                let (_, col) = need_cell()?;
                let mut seen = std::collections::HashSet::new();
                let vals: Vec<String> = g
                    .order
                    .iter()
                    .filter(|id| !g.changes.deleted.contains(id)) // as shown()
                    .map(|&id| g.value(id, col))
                    .filter(|v| v != "NULL" && seen.insert(v.clone()))
                    .map(|v| {
                        if crate::grid::num(&v).is_some() {
                            v
                        } else {
                            driver.literal(&v)
                        }
                    })
                    .collect();
                let text = format!("({})", vals.join(", "));
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                Ok(Some(format!(
                    "Copied {} as an IN list",
                    plural(vals.len(), "value")
                )))
            }
            GridOp::CopyAllMarkdown => {
                let (text, rows) = self.export_text("md", cx)?;
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                Ok(Some(format!("Copied {} as Markdown", plural(rows, "row"))))
            }
            GridOp::CopyValue | GridOp::CopyRow | GridOp::CopyInsert => {
                let (row, col) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                let values: Vec<String> = (0..g.r.columns.len()).map(|c| g.value(id, c)).collect();
                let text = match op {
                    GridOp::CopyValue => values[col].clone(),
                    GridOp::CopyRow => values.join("\t"),
                    _ => format!(
                        "INSERT INTO {} ({}) VALUES ({});",
                        driver.table_ref(table.as_deref().unwrap_or("table_name")),
                        g.r.columns
                            .iter()
                            .map(|c| driver.quote(c))
                            .collect::<Vec<_>>()
                            .join(", "),
                        values
                            .iter()
                            .map(|v| if v == "NULL" {
                                v.clone()
                            } else {
                                driver.literal(v)
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                Ok(Some(format!(
                    "Copied {}",
                    if text.len() > 60 {
                        format!("{}…", &text[..text.floor_char_boundary(60)])
                    } else {
                        text
                    }
                )))
            }
            GridOp::EditCell => {
                editable()?;
                let (row, col) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                let v = g.value(id, col);
                let tab = &mut self.tabs[ix];
                tab.editing = Some((id, col));
                tab.edit_input.update(cx, |i, cx| {
                    i.set_value(if v == "NULL" { String::new() } else { v }, window, cx);
                    i.focus(window, cx);
                });
                Ok(None)
            }
            GridOp::SetNull => {
                editable()?;
                let (row, col) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                self.set_cell(ix, id, col, None, cx);
                Ok(Some("Set to NULL (not saved yet)".into()))
            }
            GridOp::DeleteRow => {
                editable()?;
                let (row, _) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                grid.update(cx, |g, cx| {
                    let d = &mut g.delegate_mut().changes.deleted;
                    if !d.remove(&id) {
                        d.insert(id);
                    }
                    cx.notify();
                });
                Ok(Some("Row marked for deletion (Save to apply)".into()))
            }
            GridOp::AddRow => {
                editable()?;
                let row = grid.update(cx, |g, cx| {
                    let r = g.delegate_mut().add_row();
                    g.refresh(cx);
                    g.scroll_to_row(r, cx);
                    r
                });
                self.tabs[ix].cell = Some((row, 0));
                self.grid_op(GridOp::EditCell, window, cx, done)?;
                Ok(Some("New row: type values, then Save".into()))
            }
            GridOp::FollowFk => {
                editable()?;
                let (row, col) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                let name = g.r.columns[col].clone();
                let value = g.value(id, col);
                // The Columns view knows what each column references ("table.column").
                let refs = t
                    .columns
                    .read(cx)
                    .delegate()
                    .r
                    .rows
                    .iter()
                    .find(|c| c[0] == name)
                    .and_then(|c| c.get(4).cloned())
                    .unwrap_or_default();
                let target = refs
                    .split(", ")
                    .next()
                    .filter(|r| !r.is_empty())
                    .ok_or(format!("{name} is not a foreign key"))?
                    .to_string();
                if value == "NULL" {
                    return Err(format!("{name} is NULL: it references nothing"));
                }
                // "schema.table.column" or "table.column"; a bare table means its primary key.
                let (ref_table, ref_col) = match target.rsplit_once('.') {
                    Some((t, c)) if !(driver == Driver::Postgres && !t.contains('.')) => {
                        (t.to_string(), Some(c.to_string()))
                    }
                    _ => (target.clone(), None),
                };
                let conn = t.conn;
                // an open tab of that table with unsaved edits keeps its rows and filter
                let target_tab = self
                    .tabs
                    .iter()
                    .position(|o| o.conn == conn && o.kind == TabKind::Table(ref_table.clone()));
                if let Some(i) = target_tab {
                    self.unsaved_edits([i], cx)?;
                }
                self.open_table(conn, ref_table.clone(), window, cx, done);
                let filter = ref_col
                    .map(|c| format!("{} = {}", driver.quote(&c), driver.literal(&value)))
                    .unwrap_or_default();
                if let Some(nt) = self.tabs.get(self.active) {
                    nt.filter
                        .update(cx, |i, cx| i.set_value(filter.clone(), window, cx));
                }
                Ok(Some(format!("{ref_table}: {filter}")))
            }
            GridOp::DuplicateRow => {
                editable()?;
                let (row, _) = need_cell()?;
                let id = g.row_id(row).unwrap_or_default();
                let values: Vec<String> = (0..g.r.columns.len()).map(|c| g.value(id, c)).collect();
                let key: Vec<usize> = t
                    .key
                    .iter()
                    .filter_map(|k| g.r.columns.iter().position(|c| c == k))
                    .collect();
                let new = grid.update(cx, |g, cx| {
                    let r = g.delegate_mut().add_row();
                    g.refresh(cx);
                    g.scroll_to_row(r, cx);
                    r
                });
                let new_id = self.tabs[ix]
                    .grid
                    .read(cx)
                    .delegate()
                    .row_id(new)
                    .unwrap_or_default();
                for (c, v) in values.into_iter().enumerate() {
                    // a blob's "[BLOB n bytes]" is no value to insert
                    if !key.contains(&c) && v != "NULL" && !db::is_blob_text(&v) {
                        self.set_cell(ix, new_id, c, Some(v), cx);
                    }
                }
                self.tabs[ix].cell = Some((new, 0));
                Ok(Some(
                    "Row duplicated (key left empty); Save to insert it".into(),
                ))
            }
            GridOp::Revert => {
                grid.update(cx, |g, cx| {
                    g.delegate_mut().revert();
                    g.refresh(cx);
                });
                self.tabs[ix].editing = None;
                Ok(Some("Changes reverted".into()))
            }
            GridOp::Save => {
                // used up by this save, also when it stops early (see `run`)
                let (confirmed, agent) = (t.confirmed, t.agent);
                (self.tabs[ix].confirmed, self.tabs[ix].agent) = (false, false);
                let t = &self.tabs[ix];
                let g = grid.read(cx).delegate();
                let key: Vec<usize> = t
                    .key
                    .iter()
                    .filter_map(|k| g.r.columns.iter().position(|c| c == k))
                    .collect();
                let sql = editable().map_err(String::from).and_then(|table| {
                    if g.changes.is_empty() {
                        return Err("nothing to save".to_string());
                    }
                    g.changes
                        .to_sql(driver, &table, &g.r.columns, &g.r.rows, &key)
                });
                let sql = match sql {
                    Ok(sql) => sql,
                    Err(e) => {
                        done(self, Err(e.clone()), window, cx);
                        return Err(e);
                    }
                };
                let (n, conn) = (sql.len(), t.conn);
                if let Some(name) = self
                    .conn_ix(t.conn)
                    .filter(|&i| self.conns[i].cfg.read_only)
                    .map(|i| self.conns[i].cfg.name.clone())
                {
                    let e = format!("{name} is read-only: the changes cannot be saved");
                    done(self, Err(e.clone()), window, cx);
                    return Err(e);
                }
                if !confirmed
                    && let Some(name) = self
                        .conn_ix(conn)
                        .filter(|&i| self.conns[i].cfg.production)
                        .map(|i| self.conns[i].cfg.name.clone())
                {
                    if agent {
                        let e = format!(
                            "{name} is a production connection: save again with confirm: true"
                        );
                        done(self, Err(e.clone()), window, cx);
                        return Err(e);
                    }
                    let answer = window.prompt(
                        PromptLevel::Warning,
                        &format!(
                            "Save {} change(s) to production ({name})?",
                            g.changes.count()
                        ),
                        Some(&sql.join("\n").chars().take(600).collect::<String>()),
                        &["Save", "Cancel"],
                        cx,
                    );
                    let key = self.tabs[ix].grid.entity_id();
                    cx.spawn_in(window, async move |this, cx| {
                        let yes = answer.await == Ok(0);
                        let _ = this.update_in(cx, |v, w, cx| {
                            // The tab may have closed, moved, or another become active, meanwhile.
                            match v.tab_ix(key) {
                                Some(ix) if yes => {
                                    v.tabs[ix].confirmed = true;
                                    v.active = ix;
                                    let _ = v.grid_op(GridOp::Save, w, cx, done);
                                }
                                None if yes => done(v, Err("the tab was closed".into()), w, cx),
                                _ => done(v, Err("cancelled".into()), w, cx),
                            }
                        });
                    })
                    .detach();
                    return Ok(Some("Waiting for confirmation…".into()));
                }
                (self.tabs[ix].running, self.tabs[ix].started) = (true, Some(Instant::now()));
                self.tabs[ix].editing = None; // an open cell editor would change it meanwhile
                let key = self.tabs[ix].grid.entity_id();
                // Manual commit: the changes join the open transaction instead of committing.
                let (manual, in_tx) = self
                    .conn_ix(conn)
                    .map(|i| (self.conns[i].cfg.manual_commit, self.conns[i].in_tx))
                    .unwrap_or_default();
                if manual && let Some(i) = self.conn_ix(conn) {
                    self.conns[i].in_tx = true;
                    self.conns[i].tx_statements += n;
                }
                self.with_session_write::<QueryResult>(
                    conn,
                    true,
                    move |s| {
                        if manual {
                            if !in_tx {
                                s.begin()?;
                            }
                            s.apply_in(&sql, false)
                        } else {
                            s.apply(&sql)
                        }
                    },
                    window,
                    cx,
                    Box::new(move |v, r, window, cx| {
                        let ix = v.tab_ix(key);
                        if let Some(t) = ix.and_then(|ix| v.tabs.get_mut(ix)) {
                            t.running = false;
                        }
                        match r {
                            Ok(_) => {
                                v.status = if manual { format!("Saved: {n} statement(s) in the open transaction; Commit to keep them") } else { format!("Saved: {n} statement(s) committed") };
                                match ix {
                                    Some(ix) => {
                                        v.tabs[ix].grid.update(cx, |g, _| {
                                            g.delegate_mut().changes = Default::default() // saved
                                        });
                                        v.reload_table(ix, window, cx, done)
                                    }
                                    None => done(v, Ok(json!({ "saved": n })), window, cx),
                                }
                            }
                            Err(e) => {
                                if let Some(t) = ix.and_then(|ix| v.tabs.get_mut(ix)) {
                                    t.message = Some(Err(e.clone()));
                                }
                                // taken back to the savepoint: not in the transaction
                                if manual && let Some(c) = v.conn_ix(conn).map(|i| &mut v.conns[i]) {
                                    c.tx_statements = c.tx_statements.saturating_sub(n);
                                }
                                cx.notify();
                                done(v, Err(e), window, cx)
                            }
                        }
                    }),
                );
                Ok(Some("Saving…".into()))
            }
        }
    }

    /// Record a pending cell change (dropped again when it equals the loaded value).
    pub(super) fn set_cell(
        &mut self,
        tab: usize,
        id: usize,
        col: usize,
        value: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.tabs[tab].running {
            // the rows are about to be replaced (load) or the changes cleared (save)
            self.status = "Not changed: wait until it has loaded or saved".into();
            return;
        }
        self.tabs[tab].grid.update(cx, |g, cx| {
            let d = g.delegate_mut();
            let loaded = d.r.rows.get(id).and_then(|r| r.get(col)).cloned();
            let same = !d.changes.inserted.contains(&id)
                && loaded.as_deref() == Some(value.as_deref().unwrap_or("NULL"));
            if same {
                d.changes.cells.remove(&(id, col));
            } else {
                d.changes.cells.insert((id, col), value);
            }
            cx.notify();
        });
    }

    /// Close the cell editor, keeping `value` (None: cancel).
    pub(super) fn finish_edit(
        &mut self,
        tab: usize,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        let Some((id, col)) = t.editing.take() else {
            return;
        };
        // Enter on a NULL cell without typing keeps it NULL (not '')
        let was_null = t.grid.read(cx).delegate().value(id, col) == "NULL";
        if let Some(v) = value.filter(|v| !(was_null && v.is_empty())) {
            let running = self.tabs[tab].running; // set_cell says why it did not change it
            self.set_cell(tab, id, col, Some(v), cx);
            if !running {
                self.status = "Cell changed (Cmd/Ctrl+S saves)".into();
            }
        }
        self.tabs[tab]
            .grid
            .update(cx, |g, cx| g.focus_handle(cx).focus(window, cx));
        cx.notify();
    }
}
