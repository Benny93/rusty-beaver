//! Connections: connecting, background work on a session (with reconnect), transactions, metadata.

use super::*;

impl AppView {
    /// A SQL tab counting the rows of every table, run right away (try the chart on it).
    pub(super) fn row_counts(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        self.connect(
            id,
            window,
            cx,
            Box::new(move |v, r, w, cx| {
                if let Err(e) = r {
                    return done(v, Err(e), w, cx);
                }
                let Some(i) = v.conn_ix(id) else {
                    return done(v, Err("connection is gone".into()), w, cx);
                };
                let c = &v.conns[i];
                let tables: Vec<String> = c
                    .tables
                    .iter()
                    .filter(|(_, k)| k != "VIEW")
                    .map(|(n, _)| n.clone())
                    .take(200)
                    .collect();
                if tables.is_empty() {
                    return done(v, Err("no tables".into()), w, cx);
                }
                let sql = db::row_counts_sql(c.cfg.driver, &tables);
                match v.new_editor(Some(id), w, cx) {
                    Ok(tab) => v.run(tab, Source::Given(sql), 200, w, cx, done),
                    Err(e) => done(v, Err(e), w, cx),
                }
            }),
        );
    }

    /// Connect (if needed), load the table list, expand the node; then `done` with the tables.
    pub(super) fn connect(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(ix) = self.conn_ix(id) else {
            return done(self, Err("connection is gone".into()), window, cx);
        };
        let c = &mut self.conns[ix];
        c.expanded = true;
        if c.status == ConnStatus::Connected {
            let tables = c.tables.clone();
            return done(self, Ok(tables_json(&tables)), window, cx);
        }
        // one connect at a time: a second Vault sign-in could not get the callback port
        if c.status == ConnStatus::Connecting {
            c.waiting.push(done);
            return;
        }
        c.status = ConnStatus::Connecting;
        c.generation += 1;
        let generation = c.generation;
        self.status = if c.cfg.auth.is_vault() {
            format!(
                "Connecting to {}: fetching credentials from Vault (sign in in your browser if it opens)…",
                c.cfg.name
            )
        } else {
            format!("Connecting to {}…", c.cfg.name)
        };
        let cfg = c.cfg.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    // The resolved credentials also serve the canceller (MySQL opens a 2nd connection).
                    let cfg = crate::vault::resolve(&cfg, &crate::vault::open_browser)?;
                    let mut s = db::connect(&cfg)?;
                    let tables = s.tables()?;
                    let canceller = s.canceller(&cfg);
                    let version = s.server_version();
                    Ok::<_, String>((s, tables, canceller, version))
                })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                let Some(ix) = v.conn_ix(id) else {
                    return done(v, Err("connection is gone".into()), window, cx);
                };
                let c = &mut v.conns[ix];
                if c.generation != generation {
                    // Disconnected (or edited) meanwhile: this session has the old settings.
                    if c.status == ConnStatus::Connecting {
                        return c.waiting.push(done); // a newer connect answers it
                    }
                    let waiting = std::mem::take(&mut c.waiting);
                    let e = "disconnected while connecting".to_string();
                    for w in waiting {
                        w(v, Err(e.clone()), window, cx);
                    }
                    return done(v, Err(e), window, cx);
                }
                let waiting = std::mem::take(&mut c.waiting);
                let out = match res {
                    Ok((s, tables, canceller, version)) => {
                        c.session = Some(Arc::new(Mutex::new(s)));
                        c.server = Some(version.clone()).filter(|v| !v.is_empty());
                        c.canceller = Some(Arc::new(canceller));
                        c.status = ConnStatus::Connected;
                        v.status = format!(
                            "Connected to {} ({}{} tables)",
                            c.cfg.name,
                            if version.is_empty() {
                                String::new()
                            } else {
                                format!("{version}, ")
                            },
                            tables.len()
                        );
                        c.tables = tables;
                        let j = tables_json(&c.tables);
                        v.load_catalog(id, cx);
                        Ok(j)
                    }
                    Err(e) => {
                        c.status = ConnStatus::Failed(e.clone());
                        c.expanded = false;
                        v.status = format!("{}: {e}", c.cfg.name);
                        Err(e)
                    }
                };
                cx.notify();
                for w in waiting {
                    w(v, out.clone(), window, cx);
                }
                done(v, out, window, cx);
            });
        })
        .detach();
    }

    /// Fill the connection's autocompletion catalog: table names now, columns in the background.
    pub(super) fn load_catalog(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.conn_ix(id) else { return };
        let c = &self.conns[ix];
        let names: Vec<String> = c.tables.iter().map(|(n, _)| n.clone()).collect();
        {
            let mut cat = c.catalog.lock().unwrap();
            // columns of tables that are gone (dropped, another database) go too
            let keep: std::collections::HashSet<String> = names
                .iter()
                .flat_map(|n| {
                    let n = n.to_lowercase();
                    let bare = n.split_once('.').map(|(_, b)| b.to_string());
                    std::iter::once(n).chain(bare)
                })
                .collect();
            cat.columns.retain(|k, _| keep.contains(k));
            cat.tables = names.clone();
        }
        let (Some(session), catalog) = (c.session.clone(), c.catalog.clone()) else {
            return;
        };
        cx.background_spawn(async move {
            // ponytail: first 300 tables only; huge schemas complete table names, not every column
            for t in names.iter().take(300) {
                let Ok(cols) = session.lock().unwrap().columns(t) else {
                    continue;
                };
                catalog.lock().unwrap().set_columns(
                    t,
                    cols.rows
                        .into_iter()
                        .map(|r| (db::text(&r[0]).to_string(), db::text(&r[1]).to_string()))
                        .collect(),
                );
            }
        })
        .detach();
    }

    /// Insert the rows of a CSV file (header = column names) into a table, in one transaction.
    /// Production: `confirm` None (the UI) asks, an agent's Some must be true.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn import_csv(
        &mut self,
        id: u64,
        table: String,
        path: std::path::PathBuf,
        confirm: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(driver) = self.conn_ix(id).map(|i| self.conns[i].cfg.driver) else {
            return done(self, Err("connection is gone".into()), window, cx);
        };
        // Reading and parsing a big file takes a while: not on the UI thread.
        let (t, p) = (table.clone(), path.clone());
        let read = cx.background_spawn(async move {
            std::fs::read(&p)
                .map_err(|e| format!("{}: {e}", p.display()))
                .and_then(|bytes| db::csv_text(bytes).map_err(|e| format!("{}: {e}", p.display())))
                .and_then(|text| db::csv_inserts(driver, &t, &text))
        });
        cx.spawn_in(window, async move |this, cx| {
            let inserts = read.await;
            let _ = this.update_in(cx, |v, w, cx| {
                v.import_rows(id, table, path, inserts, confirm, w, cx, done)
            });
        })
        .detach();
    }

    /// `import_csv` once the file is read: the INSERTs, or why there are none.
    #[allow(clippy::too_many_arguments)]
    fn import_rows(
        &mut self,
        id: u64,
        table: String,
        path: std::path::PathBuf,
        inserts: Result<Vec<String>, String>,
        confirm: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let inserts = match inserts {
            Ok(v) => v,
            Err(e) => {
                self.status = e.clone();
                return done(self, Err(e), window, cx);
            }
        };
        if let Some(name) = self
            .conn_ix(id)
            .filter(|&i| self.conns[i].cfg.read_only)
            .map(|i| self.conns[i].cfg.name.clone())
        {
            return done(self, Err(format!("{name} is read-only")), window, cx);
        }
        let n = inserts.len();
        if confirm != Some(true)
            && let Some(name) = self
                .conn_ix(id)
                .filter(|&i| self.conns[i].cfg.production)
                .map(|i| self.conns[i].cfg.name.clone())
        {
            if confirm.is_some() {
                let e =
                    format!("{name} is a production connection: import again with confirm: true");
                return done(self, Err(e), window, cx);
            }
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Import {} into production ({name})?", plural(n, "row")),
                Some(&format!("{table} ← {}", path.display())),
                &["Import", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let yes = answer.await == Ok(0);
                let _ = this.update_in(cx, |v, w, cx| {
                    if yes {
                        v.import_rows(id, table, path, Ok(inserts), Some(true), w, cx, done)
                    } else {
                        done(v, Err("cancelled".into()), w, cx)
                    }
                });
            })
            .detach();
            return;
        }
        self.status = format!("Importing {} into {table}…", plural(n, "row"));
        let (manual, in_tx) = self
            .conn_ix(id)
            .map(|i| (self.conns[i].cfg.manual_commit, self.conns[i].in_tx))
            .unwrap_or_default();
        if manual && let Some(i) = self.conn_ix(id) {
            self.conns[i].in_tx = true;
            self.conns[i].tx_statements += n;
        }
        self.with_session_write::<QueryResult>(
            id,
            true,
            move |s| {
                if manual {
                    if !in_tx {
                        s.begin()?;
                    }
                    s.apply_in(&inserts, false)
                } else {
                    s.apply(&inserts)
                }
            },
            window,
            cx,
            Box::new(move |v, r, w, cx| {
                let out = r.map(|_| {
                    v.status = format!("Imported {} into {table}", plural(n, "row"));
                    json!({ "imported": n })
                });
                if let Err(e) = &out {
                    v.status = format!("Import failed (nothing written): {e}");
                    if manual && let Some(i) = v.conn_ix(id) {
                        let c = &mut v.conns[i];
                        c.tx_statements = c.tx_statements.saturating_sub(n);
                    }
                }
                // Refresh an open data tab of that table (unless it has unsaved edits).
                if let Some(i) = v.tabs.iter().position(|t| {
                    t.conn == id
                        && t.kind == TabKind::Table(table.clone())
                        && t.grid.read(cx).delegate().changes.is_empty()
                }) {
                    v.reload_table(i, w, cx, Box::new(|_, _, _, _| {}));
                }
                cx.notify();
                done(v, out, w, cx)
            }),
        );
    }

    /// End a manual-commit transaction.
    pub(super) fn end_tx(
        &mut self,
        id: u64,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let open = self.conn_ix(id).is_some_and(|i| self.conns[i].in_tx);
        if !open {
            return done(self, Err("no transaction is open".into()), window, cx);
        }
        let sql = if commit { "COMMIT" } else { "ROLLBACK" };
        // no retry: a COMMIT on a fresh connection would report statements lost with the old one
        self.with_session_write::<QueryResult>(
            id,
            true,
            move |s| s.query(sql),
            window,
            cx,
            Box::new(move |v, r, w, cx| {
                if let Some(i) = v.conn_ix(id) {
                    let c = &mut v.conns[i];
                    v.status = match &r {
                        Ok(_) => format!(
                            "{} {} statement(s) on {}",
                            if commit { "Committed" } else { "Rolled back" },
                            c.tx_statements,
                            c.cfg.name
                        ),
                        Err(e) => e.clone(),
                    };
                    (c.in_tx, c.tx_statements) = (false, 0);
                }
                // Rolled-back rows should disappear from open table tabs.
                if !commit {
                    for i in 0..v.tabs.len() {
                        let t = &v.tabs[i];
                        if t.conn == id
                            && matches!(t.kind, TabKind::Table(_))
                            && t.grid.read(cx).delegate().changes.is_empty()
                        {
                            v.reload_table(i, w, cx, Box::new(|_, _, _, _| {}));
                        }
                    }
                }
                cx.notify();
                done(v, r.map(|_| json!({ "ok": sql })), w, cx)
            }),
        );
    }

    pub(super) fn set_manual_commit(
        &mut self,
        id: u64,
        manual: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let i = self.conn_ix(id).ok_or("connection is gone")?;
        if self.conns[i].in_tx && !manual {
            return Err("commit or roll back the open transaction first".into());
        }
        self.conns[i].cfg.manual_commit = manual;
        self.status = format!(
            "{}: {}",
            self.conns[i].cfg.name,
            if manual {
                "manual commit"
            } else {
                "auto-commit"
            }
        );
        self.save();
        cx.notify();
        Ok(())
    }

    /// Reload a connection's table list (after DDL elsewhere).
    pub(super) fn refresh(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        self.with_session::<Vec<(String, String)>>(
            id,
            |s| s.tables(),
            window,
            cx,
            Box::new(move |v, r, window, cx| {
                let out = r.map(|tables| {
                    let j = tables_json(&tables);
                    if let Some(ix) = v.conn_ix(id) {
                        let c = &mut v.conns[ix];
                        c.tables = tables;
                        c.columns.clear();
                        c.expanded = true;
                        v.status = format!("Refreshed {}", c.cfg.name);
                        v.load_catalog(id, cx);
                    }
                    j
                });
                cx.notify();
                done(v, out, window, cx)
            }),
        );
    }

    pub(super) fn confirm_delete(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.conn_ix(id).map(|i| self.conns[i].cfg.name.clone()) else {
            return;
        };
        // its tabs close with it: not over unsaved grid edits (the status bar says which)
        let tabs: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| self.tabs[i].conn == id)
            .collect();
        if self.unsaved_edits(tabs, cx).is_err() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete connection \"{name}\"?"),
            Some("The database itself is not touched."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |v, w, cx| {
                    // edits may have come in while the prompt was open
                    let tabs: Vec<usize> = (0..v.tabs.len())
                        .filter(|&i| v.tabs[i].conn == id)
                        .collect();
                    if v.unsaved_edits(tabs, cx).is_ok() {
                        v.delete_connection(id, w, cx)
                    }
                });
            }
        })
        .detach();
    }

    /// Open a SQL editor holding a generated statement for a table (`db::generate`).
    pub(super) fn generate_sql(
        &mut self,
        id: u64,
        table: String,
        kind: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let t = table.clone();
        self.with_session::<QueryResult>(
            id,
            move |s| s.columns(&t),
            window,
            cx,
            Box::new(move |v, r, window, cx| {
                let driver = v
                    .conn_ix(id)
                    .map(|i| v.conns[i].cfg.driver)
                    .unwrap_or(Driver::Sqlite);
                let out = r.and_then(|cols| {
                    let cols: Vec<(String, bool)> = cols
                        .rows
                        .iter()
                        .map(|c| (db::text(&c[0]).to_string(), db::text(&c[3]) == "PK"))
                        .collect();
                    db::generate(driver, &table, &cols, kind)
                });
                match &out {
                    Ok(sql) => {
                        if let Ok(tab) = v.new_editor(Some(id), window, cx) {
                            v.tabs[tab]
                                .editor
                                .update(cx, |e, cx| e.set_value(sql.clone(), window, cx));
                            v.save_session_soon(cx);
                        }
                    }
                    Err(e) => v.status = e.clone(),
                }
                cx.notify();
                done(v, out.map(|sql| json!({ "sql": sql })), window, cx)
            }),
        );
    }

    pub(super) fn disconnect(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(ix) = self.conn_ix(id) {
            let c = &mut self.conns[ix];
            (c.session, c.status, c.expanded) = (None, ConnStatus::Disconnected, false);
            c.generation += 1; // a connect still running is stale now
            c.canceller = None;
            (c.in_tx, c.tx_statements) = (false, 0);
            c.tables.clear();
            c.columns.clear();
            self.status = format!("Disconnected {}", c.cfg.name);
            cx.notify();
        }
    }

    /// Run blocking work on a connected session, connecting first if needed.
    pub(super) fn with_session<T: Send + 'static>(
        &mut self,
        id: u64,
        work: impl Fn(&mut Session) -> Result<T, String> + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Callback<T>,
    ) {
        self.with_session_retry(id, Arc::new(work), true, window, cx, done)
    }

    /// `with_session` for work that may change data (`write`): after a lost connection it
    /// does not run again, since the server may have done it already (a duplicated INSERT).
    pub(super) fn with_session_write<T: Send + 'static>(
        &mut self,
        id: u64,
        write: bool,
        work: impl Fn(&mut Session) -> Result<T, String> + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Callback<T>,
    ) {
        self.with_session_retry(id, Arc::new(work), !write, window, cx, done)
    }

    /// Run `work`; when the connection turns out to be gone (server restart, network), drop
    /// it and, if `retry`, reconnect and run once more. An open manual-commit transaction is
    /// lost with the connection, so then the error is reported instead.
    pub(super) fn with_session_retry<T: Send + 'static>(
        &mut self,
        id: u64,
        work: Work<T>,
        retry: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Callback<T>,
    ) {
        let session = self
            .conn_ix(id)
            .and_then(|ix| self.conns[ix].session.clone());
        let Some(session) = session else {
            return self.connect(
                id,
                window,
                cx,
                Box::new(move |v, r, window, cx| match r {
                    Ok(_) => v.with_session_retry(id, work, retry, window, cx, done),
                    Err(e) => {
                        // not connected: no transaction can be open (unless a newer
                        // connect made a session meanwhile)
                        if let Some(ix) = v.conn_ix(id).filter(|&ix| v.conns[ix].session.is_none())
                        {
                            (v.conns[ix].in_tx, v.conns[ix].tx_statements) = (false, 0);
                        }
                        done(v, Err(e), window, cx)
                    }
                }),
            );
        };
        let w = work.clone();
        let used = session.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (res, broken) = cx
                .background_spawn(async move {
                    let mut s = session.lock().unwrap();
                    let r = w(&mut s);
                    let broken = r.is_err() && s.is_broken();
                    (r, broken)
                })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                if !broken {
                    return done(v, res, window, cx);
                }
                let Some(ix) = v.conn_ix(id) else {
                    return done(v, res, window, cx);
                };
                let c = &mut v.conns[ix];
                if !c.session.as_ref().is_some_and(|s| Arc::ptr_eq(s, &used)) {
                    // Another job already noticed and dropped (or replaced) this session:
                    // leave the connection to it, and retry on what is there now.
                    return if retry {
                        v.with_session_retry(id, work, false, window, cx, done)
                    } else {
                        let e = res.err().unwrap_or_default();
                        done(v, Err(format!("{e}\n(connection lost)")), window, cx)
                    };
                }
                let lost_tx = std::mem::take(&mut c.in_tx);
                c.tx_statements = 0;
                (c.session, c.canceller, c.status) = (None, None, ConnStatus::Disconnected);
                if retry && !lost_tx {
                    v.status = format!("Connection to {} was lost; reconnecting…", c.cfg.name);
                    v.with_session_retry(id, work, false, window, cx, done);
                } else {
                    let e = res.err().unwrap_or_default();
                    let why = if lost_tx {
                        "the open transaction was lost with it"
                    } else {
                        "it reconnects on the next query"
                    };
                    v.status = format!("Connection to {} was lost; {why}", c.cfg.name);
                    cx.notify();
                    done(v, Err(format!("{e}\n(connection lost; {why})")), window, cx);
                }
            });
        })
        .detach();
    }

    /// Toggle a table's columns in the navigator (loading them on first expand).
    pub(super) fn describe(
        &mut self,
        id: u64,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let t = table.clone();
        self.with_session(
            id,
            move |s| s.columns(&t),
            window,
            cx,
            Box::new(move |v, r, window, cx| {
                let out = r.map(|cols| {
                    let j = result_json(&cols, usize::MAX);
                    if let Some(ix) = v.conn_ix(id) {
                        let c = &mut v.conns[ix];
                        c.columns.retain(|(n, _)| *n != table);
                        c.columns.push((table, cols));
                    }
                    j
                });
                cx.notify();
                done(v, out, window, cx)
            }),
        );
    }
}
