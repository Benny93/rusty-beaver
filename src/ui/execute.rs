//! Running SQL: statements and scripts, parameters, production checks, explain, format, cancel, paging, export.

use super::*;

impl AppView {
    /// Execute SQL in a tab (its editor text when `sql` is None; the selection if any).
    pub(super) fn run(
        &mut self,
        tab: usize,
        src: Source,
        max_rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        // A new run replaces a parameter dialog still waiting on this tab.
        if let Some(key) = self.tab_key(tab)
            && self.params.as_ref().is_some_and(|p| p.tab == key)
        {
            self.submit_params(false, window, cx);
        }
        if matches!(src, Source::Given(_))
            && self.tabs.get(tab).is_some_and(|t| t.kind == TabKind::Sql)
        {
            self.save_session_soon(cx); // reads the editor a second later, after the text is in
        }
        let mysql = self.mysql_tab(tab);
        let Some(t) = self.tabs.get_mut(tab) else {
            return done(self, Err("no such tab".into()), window, cx);
        };
        let key = t.grid.entity_id();
        // One run uses up an agent's confirm and params, also when it stops early: a leftover
        // confirm would let the user's next write skip the production prompt.
        let (confirmed, agent, given) = (
            std::mem::take(&mut t.confirmed),
            std::mem::take(&mut t.agent),
            std::mem::take(&mut t.params_given),
        );
        if matches!(t.kind, TabKind::Table(_)) && !t.grid.read(cx).delegate().changes.is_empty() {
            // its result replaces the rows, and with them the unsaved edits
            let e = "save or revert the pending changes first".to_string();
            self.status = e.clone();
            cx.notify();
            return done(self, Err(e), window, cx);
        }
        if t.running {
            // it would wait for the lock, and the first one's end would clear `running`
            let e = "a query is already running in this tab: wait or cancel it".to_string();
            self.status = e.clone(); // the toolbar and shortcuts pass no `done` that shows it
            cx.notify();
            return done(self, Err(e), window, cx);
        }
        let raw = matches!(src, Source::Raw(_));
        let sql = match src {
            Source::Given(sql) => {
                t.editor
                    .update(cx, |e, cx| e.set_value(sql.clone(), window, cx));
                sql
            }
            Source::Script => t.editor.read(cx).value().to_string(),
            Source::Raw(sql) => sql,
            Source::Statement => {
                let e = t.editor.read(cx);
                let sel = e.selected_text().to_string();
                if sel.trim().is_empty() {
                    let text = e.value().to_string();
                    text[db::statement_at(&text, e.cursor(), mysql)].to_string()
                } else {
                    sel
                }
            }
        };
        if sql.trim().is_empty() {
            return done(self, Err("the editor is empty".into()), window, cx);
        }
        // :name parameters: values from the agent (params_given), else ask in a dialog.
        let names = if raw { vec![] } else { db::params(&sql) };
        if !names.is_empty() {
            if given {
                return match db::bind(&sql, &t.param_values) {
                    Ok(bound) => {
                        (t.confirmed, t.agent) = (confirmed, agent); // for the bound run
                        self.run(tab, Source::Raw(bound), max_rows, window, cx, done)
                    }
                    Err(e) => done(
                        self,
                        Err(format!("{e} (pass params: {{{}}})", names.join(", "))),
                        window,
                        cx,
                    ),
                };
            }
            let inputs = names
                .iter()
                .map(|n| {
                    let value = t.param_values.get(n).cloned().unwrap_or_default();
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("value as SQL, e.g. 42 or 'text'")
                            .default_value(value)
                    });
                    cx.subscribe_in(&input, window, |v, _, ev: &InputEvent, w, cx| {
                        if matches!(ev, InputEvent::PressEnter { .. }) {
                            v.submit_params(true, w, cx);
                        }
                    })
                    .detach();
                    (n.clone(), input)
                })
                .collect::<Vec<_>>();
            if let Some((_, first)) = inputs.first() {
                first.update(cx, |i, cx| i.focus(window, cx));
            }
            self.params = Some(ParamPrompt {
                tab: key,
                sql,
                inputs,
                max_rows,
                done: Some(done),
            });
            cx.notify();
            return;
        }
        // Production: changing data needs a yes (the dialog, or `confirm` from an agent).
        let conn = t.conn;
        if db::is_write(&sql, mysql)
            && let Some(name) = self
                .conn_ix(conn)
                .filter(|&i| self.conns[i].cfg.read_only)
                .map(|i| self.conns[i].cfg.name.clone())
        {
            return done(
                self,
                Err(format!(
                    "{name} is read-only: statements that change data or schema are refused"
                )),
                window,
                cx,
            );
        }
        if !confirmed
            && db::is_write(&sql, mysql)
            && let Some(name) = self
                .conn_ix(conn)
                .filter(|&i| self.conns[i].cfg.production)
                .map(|i| self.conns[i].cfg.name.clone())
        {
            if agent {
                return done(
                    self,
                    Err(format!(
                        "{name} is a production connection and this SQL changes data: run it again with confirm: true"
                    )),
                    window,
                    cx,
                );
            }
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Run on production ({name})?"),
                Some(&format!(
                    "This changes data or schema:\n\n{}",
                    sql.chars().take(400).collect::<String>()
                )),
                &["Run", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let yes = answer.await == Ok(0);
                let _ = this.update_in(cx, |v, w, cx| {
                    // the tab may have moved (or closed) meanwhile
                    match v.tab_ix(key) {
                        Some(tab) if yes => {
                            v.tabs[tab].confirmed = true;
                            v.run(tab, Source::Raw(sql), max_rows, w, cx, done);
                        }
                        None if yes => done(v, Err("the tab was closed".into()), w, cx),
                        _ => done(v, Err("cancelled".into()), w, cx),
                    }
                });
            })
            .detach();
            return;
        }
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.running = true;
        t.started = Some(Instant::now());
        t.editing = None;
        let conn = t.conn;
        let logged = sql.clone();
        self.status = "Executing query…".into();
        // Live "Executing… 2.4 s" while it runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let running = this.update(cx, |v, cx| {
                    cx.notify();
                    v.tabs.iter().any(|t| t.running)
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
        // Manual commit: the first statement opens the transaction it then stays in.
        let manual = self
            .conn_ix(conn)
            .is_some_and(|i| self.conns[i].cfg.manual_commit);
        let in_tx = self.conn_ix(conn).is_some_and(|i| self.conns[i].in_tx);
        let mysql = self.mysql_tab(tab);
        // first two words of each statement, past comments
        let heads = db::statement_heads(&sql, mysql);
        // MySQL commits the open transaction when it runs DDL (temporary tables aside), also
        // when the DDL then fails; only one that does not parse leaves it open. Settled once
        // the statements ran.
        let ddl = |(w1, w2): &(String, String)| {
            (matches!(
                w1.as_str(),
                "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" | "GRANT" | "REVOKE"
                    // table maintenance commits implicitly too, and so does starting another
                    // transaction
                    | "ANALYZE" | "OPTIMIZE" | "REPAIR" | "FLUSH" | "CHECK" | "CACHE"
                    | "BEGIN" | "START"
            ) || w1 == "LOCK" && w2 == "TABLES")
                && w2 != "TEMPORARY"
        };
        let mysql_ddl = mysql && heads.first().is_some_and(ddl);
        let script_ddl = mysql && heads.iter().skip(1).any(ddl);
        let one_statement = heads.len() <= 1;
        // COMMIT / ROLLBACK end the transaction (ROLLBACK TO SAVEPOINT does not); END only as
        // the first statement, as it also closes trigger and BEGIN ATOMIC bodies
        // (ABORT: Postgres' ROLLBACK)
        let ends = |(w1, w2): &(String, String)| {
            w1 == "COMMIT" || (w1 == "ROLLBACK" || w1 == "ABORT") && w2 != "TO"
        };
        let first_ends = heads.first().is_some_and(|h| ends(h) || h.0 == "END");
        // "UPDATE …; COMMIT" ends it too, once the script ran through
        // ponytail: statements after the COMMIT run outside a transaction, as typed
        let later_ends = heads.iter().skip(1).any(ends);
        // BEGIN / START TRANSACTION typed by the user: the transaction later writes run in
        let opens = heads
            .first()
            .is_some_and(|(w1, w2)| w1 == "BEGIN" || w1 == "START" && w2 == "TRANSACTION");
        // Smart commit: only a change opens the transaction; reads run inside an open one.
        let write = db::is_write(&sql, mysql);
        // session settings are writes for the prompts, but no data to put in a transaction
        // (SQLite ignores PRAGMA foreign_keys inside one, DISCARD ALL refuses to run there)
        let setting = heads
            .first()
            .is_some_and(|(w, _)| matches!(w.as_str(), "PRAGMA" | "SET" | "RESET" | "DISCARD"));
        // a script that opens its own transaction gets no BEGIN before it
        let need_begin =
            manual && !in_tx && write && !first_ends && !mysql_ddl && !setting && !opens;
        if manual && let Some(i) = self.conn_ix(conn) {
            let c = &mut self.conns[i];
            if first_ends {
                (c.in_tx, c.tx_statements) = (false, 0);
            } else if write && !mysql_ddl && !setting {
                (c.in_tx, c.tx_statements) = (true, c.tx_statements + 1);
            }
        }
        self.with_session_write::<Vec<QueryResult>>(
            conn,
            write,
            move |s| {
                if need_begin {
                    s.begin()?;
                }
                s.query_all(&sql)
            },
            window,
            cx,
            Box::new(move |v, r, window, cx| {
                // disconnected while it ran: that session and its transaction are gone, so
                // nothing of it is tracked any more
                let manual = manual
                    && v.conn_ix(conn)
                        .is_some_and(|i| v.conns[i].status == ConnStatus::Connected);
                // a write that failed is no statement of the transaction (it stays open)
                if manual
                    && write
                    && !mysql_ddl
                    && !setting
                    && !first_ends
                    && r.is_err()
                    && let Some(i) = v.conn_ix(conn)
                {
                    let c = &mut v.conns[i];
                    c.tx_statements = c.tx_statements.saturating_sub(1);
                }
                // a script that failed before its COMMIT leaves the transaction open
                if manual
                    && later_ends
                    && r.is_ok()
                    && let Some(i) = v.conn_ix(conn)
                {
                    (v.conns[i].in_tx, v.conns[i].tx_statements) = (false, 0);
                }
                if manual
                    && (mysql_ddl || script_ddl)
                    && let Some(i) = v.conn_ix(conn)
                {
                    match &r {
                        // DDL ran: MySQL committed (and later writes autocommitted)
                        Ok(_) => (v.conns[i].in_tx, v.conns[i].tx_statements) = (false, 0),
                        Err(e) if one_statement && !e.contains("ERROR 1064") => {
                            (v.conns[i].in_tx, v.conns[i].tx_statements) = (false, 0)
                        }
                        Err(_) => {}
                    }
                }
                // (BEGIN; …; COMMIT ended it again)
                if manual
                    && opens
                    && !later_ends
                    && r.is_ok()
                    && let Some(i) = v.conn_ix(conn)
                    && !v.conns[i].in_tx
                {
                    (v.conns[i].in_tx, v.conns[i].tx_statements) = (true, 0);
                }
                // which statement failed is unknown: say so rather than guess
                let r = match r {
                    Err(e) if manual && (mysql_ddl || script_ddl) && !one_statement => Err(format!(
                        "{e}\n(the script has DDL: MySQL may have committed the open transaction; Commit or Roll back to be sure)"
                    )),
                    r => r,
                };
                // The last result is shown; a script's others are one click away.
                let r = r.map(|mut all| {
                    let last = all.pop().unwrap_or_default();
                    (last, all)
                });
                let conn_name = v
                    .conn_ix(conn)
                    .map(|i| v.conns[i].cfg.name.clone())
                    .unwrap_or_default();
                let entry = match &r {
                    Ok((res, _)) => history::Entry {
                        sql: logged,
                        connection: conn_name,
                        at: history::now(),
                        ms: res.elapsed.as_secs_f64() * 1000.,
                        outcome: match res.affected {
                            Some(n) if res.columns.is_empty() => format!("{n} updated"),
                            _ => plural(res.rows.len(), "row"),
                        },
                        ok: true,
                    },
                    Err(e) => history::Entry {
                        sql: logged,
                        connection: conn_name,
                        at: history::now(),
                        // from the tab's start: a failed or cancelled run took time too
                        ms: v
                            .tab_ix(key)
                            .and_then(|i| v.tabs[i].started)
                            .map_or(0., |s| s.elapsed().as_secs_f64() * 1000.),
                        outcome: e.lines().next().unwrap_or_default().to_string(),
                        ok: false,
                    },
                };
                history::append(&entry);
                v.history.push(entry);
                if v.history.len() > history::KEEP {
                    v.history.remove(0);
                }
                let Some(t) = v.tab_ix(key).and_then(|i| v.tabs.get_mut(i)) else {
                    cx.notify();
                    return done(v, Err("the tab was closed".into()), window, cx);
                };
                t.running = false;
                let out = match r {
                    Ok((res, mut earlier)) => {
                        let ms = res.elapsed.as_secs_f64() * 1000.;
                        let n_results = earlier.len() + 1;
                        let mut msg = match res.affected {
                            Some(n) if res.columns.is_empty() => {
                                format!("Updated rows: {n} · {ms:.1} ms")
                            }
                            _ => format!(
                                "{}{} fetched · {ms:.1} ms",
                                plural(res.rows.len(), "row"),
                                if res.truncated { "+" } else { "" }
                            ),
                        };
                        if n_results > 1 {
                            msg = format!("{n_results} statements · last: {msg}");
                        }
                        let mut j = result_json(&res, max_rows);
                        if n_results > 1 {
                            j["results"] = json!(earlier.iter().chain([&res]).enumerate().map(|(i, r)| json!({ "index": i, "columns": r.columns, "row_count": r.rows.len(), "affected": r.affected })).collect::<Vec<_>>());
                        }
                        earlier.push(res.clone());
                        t.editing = None; // an edit begun on the old rows
                        t.results = earlier;
                        t.result_ix = n_results - 1;
                        t.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(res);
                            g.refresh(cx);
                        });
                        // rows as the grid shows them: an active search applies to them too
                        // (copied only then: every run would copy up to 10,000 rows again)
                        let g = t.grid.read(cx).delegate();
                        if !g.search.is_empty() {
                            let s = result_json(&g.rows(true), max_rows);
                            (j["rows"], j["row_count"], j["truncated"]) =
                                (s["rows"].clone(), s["row_count"].clone(), s["truncated"].clone());
                        }
                        v.status = msg.clone();
                        t.message = Some(Ok(msg));
                        Ok(j)
                    }
                    Err(e) => {
                        v.status = format!("Query failed: {}", e.lines().next().unwrap_or_default());
                        t.message = Some(Err(e.clone()));
                        Err(e)
                    }
                };
                // DDL may have changed the table list: re-read it in the background (the
                // session may be busy with another tab's query; the UI must not wait for it).
                if write
                    && let Some(ix) = v.conn_ix(conn)
                    && let Some(s) = v.conns[ix].session.clone()
                {
                    let tables = cx.background_spawn(async move {
                        s.lock().map_err(|e| e.to_string()).and_then(|mut s| s.tables())
                    });
                    cx.spawn(async move |this, cx| {
                        if let Ok(tables) = tables.await {
                            let _ = this.update(cx, |v, cx| {
                                if let Some(ix) = v.conn_ix(conn)
                                    && tables != v.conns[ix].tables
                                {
                                    v.conns[ix].tables = tables;
                                    v.load_catalog(conn, cx);
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .detach();
                }
                cx.notify();
                done(v, out, window, cx)
            }),
        );
    }

    /// The selection, else the statement at the cursor, of a tab's editor.
    pub(super) fn current_statement(&self, tab: usize, cx: &App) -> Option<String> {
        let e = self.tabs.get(tab)?.editor.read(cx);
        let sel = e.selected_text().to_string();
        if !sel.trim().is_empty() {
            return Some(sel);
        }
        let text = e.value().to_string();
        let mysql = self.mysql_tab(tab);
        Some(text[db::statement_at(&text, e.cursor(), mysql)].to_string())
            .filter(|s| !s.trim().is_empty())
    }

    /// Show the query plan of the current statement in the result grid.
    pub(super) fn explain(
        &mut self,
        tab: usize,
        sql: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(sql) = sql.or_else(|| self.current_statement(tab, cx)) else {
            return done(self, Err("nothing to explain".into()), window, cx);
        };
        let Some(t) = self.tabs.get(tab) else {
            return done(self, Err("no such tab".into()), window, cx);
        };
        // the plan goes to a script tab, never into a table tab's editable grid
        let tab = match t.kind {
            TabKind::Sql => tab,
            _ => match self.activate_script_tab(t.conn, window, cx) {
                Ok(i) => i,
                Err(e) => return done(self, Err(e), window, cx),
            },
        };
        let Some(t) = self.tabs.get(tab) else {
            return done(self, Err("no such tab".into()), window, cx);
        };
        let driver = self
            .conn_ix(t.conn)
            .map(|i| self.conns[i].cfg.driver)
            .unwrap_or(Driver::Sqlite);
        let prefix = if driver == Driver::Sqlite {
            "EXPLAIN QUERY PLAN "
        } else {
            "EXPLAIN "
        };
        // EXPLAIN a; b would run b: one statement only
        let one = sql.trim().trim_end_matches(';');
        if db::statement_at(one, 0, driver == Driver::Mysql).end < one.trim_end().len() {
            self.status = "Explain one statement at a time: select it".into(); // Mod+Shift+E
            cx.notify();
            return done(
                self,
                Err("explain one statement at a time".into()),
                window,
                cx,
            );
        }
        let sql = format!("{prefix}{one}");
        self.run(tab, Source::Raw(sql), 200, window, cx, done);
    }

    /// Format the selection, else the whole editor, of a tab (`format::format_sql`).
    pub(super) fn format(
        &mut self,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let t = self.tabs.get(tab).ok_or("no tab open")?;
        let (text, range) = {
            let e = t.editor.read(cx);
            (e.value().to_string(), e.selected_range())
        };
        let range = if range.is_empty() {
            0..text.len()
        } else {
            range
        };
        let formatted = crate::format::format_sql_as(&text[range.clone()], self.mysql_tab(tab));
        let new = format!(
            "{}{}{}",
            &text[..range.start],
            formatted.trim_end(),
            &text[range.end..]
        );
        t.editor
            .update(cx, |e, cx| e.set_value(new.clone(), window, cx));
        self.save_session_soon(cx);
        self.status = "Formatted".into();
        cx.notify();
        Ok(new)
    }

    /// Stop the statement running in a tab.
    pub(super) fn cancel(&mut self, tab: usize, cx: &mut Context<Self>) -> Result<(), String> {
        let t = self.tabs.get(tab).ok_or("no such tab")?;
        if !t.running {
            return Err("nothing is running".into());
        }
        // The canceller stops whatever runs on the connection: a tab that started earlier
        // there holds it, and this one is still waiting.
        // ponytail: start order stands in for lock order
        if let Some(first) = self
            .tabs
            .iter()
            .find(|o| o.running && o.conn == t.conn && o.started < t.started && o.grid != t.grid)
        {
            return Err(format!(
                "this query waits for the one in {:?} on the same connection: cancel that one",
                first.title
            ));
        }
        let c = self
            .conn_ix(t.conn)
            .and_then(|i| self.conns[i].canceller.clone())
            .ok_or("not connected")?;
        self.status = "Cancelling…".into();
        cx.background_spawn(async move {
            if let Err(e) = c.cancel() {
                eprintln!("cancel: {e}");
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    /// Re-run a table tab's SELECT with its current filter.
    pub(super) fn reload_table(
        &mut self,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(t) = self.tabs.get(tab) else {
            return done(self, Err("no such tab".into()), window, cx);
        };
        let TabKind::Table(table) = &t.kind else {
            return done(self, Err("not a table tab".into()), window, cx);
        };
        // loading replaces the rows, and with them the unsaved edits
        if !t.grid.read(cx).delegate().changes.is_empty() {
            let e = format!("{table}: save or revert the pending changes first");
            self.status = e.clone();
            cx.notify();
            return done(self, Err(e), window, cx);
        }
        let Some(ix) = self.conn_ix(t.conn) else {
            return done(self, Err("connection is gone".into()), window, cx);
        };
        let driver = self.conns[ix].cfg.driver;
        let filter = t.filter.read(cx).value().to_string();
        let sql = driver.select_all(table, &filter, t.limit);
        // "x = 1 ORDER BY y" filters and sorts the page; count(*) with that ORDER BY fails on
        // Postgres, so the total drops it
        let (cond, _) = db::split_order(&filter);
        let count = format!(
            "SELECT count(*) FROM {}{};",
            driver.table_ref(table),
            if cond.trim().is_empty() {
                String::new()
            } else {
                format!(" WHERE {cond}")
            }
        );
        let (conn, key) = (t.conn, t.grid.entity_id());
        self.run(
            tab,
            Source::Given(sql),
            50,
            window,
            cx,
            Box::new(move |v, r, w, cx| {
                // Then the total, quietly (not in the history): "200 of 1,234 rows".
                if r.is_ok() {
                    v.with_session::<QueryResult>(
                        conn,
                        move |s| s.query(&count),
                        w,
                        cx,
                        Box::new(move |v, c, _, cx| {
                            if let Some(t) = v.tab_ix(key).and_then(|i| v.tabs.get_mut(i)) {
                                t.total = c.ok().and_then(|c| {
                                    c.rows.first()?.first()?.as_deref()?.parse().ok()
                                });
                            }
                            cx.notify();
                        }),
                    );
                }
                done(v, r, w, cx)
            }),
        );
    }

    /// Show another result of the last script execution in the grid.
    pub(super) fn show_result(
        &mut self,
        tab: usize,
        i: usize,
        max_rows: usize,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let t = self.tabs.get_mut(tab).ok_or("no such tab")?;
        let r = t.results.get(i).cloned().ok_or(format!(
            "no result {i}; the last run produced {}",
            t.results.len()
        ))?;
        if !t.grid.read(cx).delegate().changes.is_empty() {
            return Err("save or revert the pending changes first".into());
        }
        t.result_ix = i;
        t.grid.update(cx, |g, cx| {
            g.delegate_mut().set(r);
            g.refresh(cx);
        });
        // the rows as the grid shows them (an active search applies to the new result)
        let j = result_json(&t.grid.read(cx).delegate().rows(true), max_rows);
        cx.notify();
        Ok(j)
    }

    /// Fetch the next page of a table tab (keeps the filter; pending edits must be saved first).
    pub(super) fn load_more(
        &mut self,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return done(self, Err("no such tab".into()), window, cx);
        };
        if !t.grid.read(cx).delegate().changes.is_empty() {
            return done(
                self,
                Err("save or revert the pending changes first".into()),
                window,
                cx,
            );
        }
        t.limit += PAGE;
        // a load that fails (or is refused) keeps the old page count: else the next one
        // fetched two pages, and "Load more" hid while rows were left
        let key = t.grid.entity_id();
        let done: Done = Box::new(move |v, r, w, cx| {
            if r.is_err()
                && let Some(t) = v.tab_ix(key).and_then(|i| v.tabs.get_mut(i))
            {
                t.limit = t.limit.saturating_sub(PAGE).max(PAGE);
            }
            done(v, r, w, cx)
        });
        self.reload_table(tab, window, cx, done);
    }

    /// The active grid as shown, in the format of `ext` (see `db::export`).
    pub(super) fn export_text(&self, ext: &str, cx: &App) -> Result<(String, usize), String> {
        let t = self.tabs.get(self.active).ok_or("no tab open")?;
        let mut r = t.grid.read(cx).delegate().shown();
        if r.columns.is_empty() {
            return Err("no result to export".into());
        }
        // INSERTs of a table take no value for its generated columns: running them would fail
        if ext.eq_ignore_ascii_case("sql") && matches!(t.kind, TabKind::Table(_)) {
            let cols = &t.columns.read(cx).delegate().r.rows;
            let keep: Vec<bool> = r
                .columns
                .iter()
                .map(|c| {
                    !cols
                        .iter()
                        .any(|k| db::text(&k[0]) == c && db::text(&k[5]) == db::GENERATED)
                })
                .collect();
            let pick = |row: Vec<Option<String>>| {
                row.into_iter()
                    .zip(&keep)
                    .filter(|(_, k)| **k)
                    .map(|(v, _)| v)
                    .collect::<Vec<_>>()
            };
            r.columns = pick(r.columns.into_iter().map(Some).collect())
                .into_iter()
                .flatten()
                .collect();
            r.rows = r.rows.into_iter().map(pick).collect();
        }
        let r = &r;

        let driver = self
            .conn_ix(t.conn)
            .map(|i| self.conns[i].cfg.driver)
            .unwrap_or(Driver::Sqlite);
        let table = match &t.kind {
            TabKind::Table(n) => n.as_str(),
            _ => "table_name",
        };
        // Postgres takes values for GENERATED ALWAYS identity columns only when told so
        let identity = ext.eq_ignore_ascii_case("sql")
            && driver == Driver::Postgres
            && matches!(t.kind, TabKind::Table(_))
            && t.columns.read(cx).delegate().r.rows.iter().any(|k| {
                db::text(&k[5]) == "identity" && r.columns.iter().any(|c| c == db::text(&k[0]))
            });
        let mut text = db::export(r, ext, driver, table);
        if identity {
            text = text
                .split_inclusive('\n')
                .map(|l| match l.starts_with("INSERT INTO ") {
                    true => l.replacen(") VALUES (", ") OVERRIDING SYSTEM VALUE VALUES (", 1),
                    false => l.to_string(),
                })
                .collect();
        }
        Ok((text, r.rows.len()))
    }

    /// Export the active grid; the format follows the file extension (csv, tsv, json, md, sql).
    /// The format is the file's extension; `chosen` (the menu's) when that is none we know
    /// (report_2024.10.21), else an error.
    pub(super) fn export_csv(
        &mut self,
        path: &std::path::Path,
        chosen: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        if !path.is_absolute() {
            // relative to the app's working directory, which is / when started from Finder
            return Err(format!("{}: give an absolute path", path.display()));
        }
        let known = |e: &str| EXPORT_FORMATS.iter().any(|f| f.0 == e) || e == "markdown";
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) if known(&e.to_ascii_lowercase()) => e.to_ascii_lowercase(),
            e => match (chosen, e) {
                (Some(c), _) => c.to_string(),
                (None, None) => "csv".to_string(),
                (None, Some(e)) => {
                    let list: Vec<&str> = EXPORT_FORMATS.iter().map(|f| f.0).collect();
                    return Err(format!("{e:?} is no export format: {}", list.join(", ")));
                }
            },
        };
        let (text, rows) = self.export_text(&ext, cx)?;
        db::write_atomic(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        // A table tab holds one page: say so when the table has more rows.
        // (rows loaded from the table, not shown: a row marked for deletion was loaded)
        let total = self.tabs.get(self.active).and_then(|t| {
            let g = t.grid.read(cx).delegate();
            let loaded = g.r.rows.len() - g.changes.inserted.len();
            t.total.filter(|&n| n as usize > loaded)
        });
        // a result cut at the row cap has more rows than the file
        let capped = self
            .tabs
            .get(self.active)
            .is_some_and(|t| t.grid.read(cx).delegate().r.truncated);
        let msg = match total {
            Some(n) => format!(
                "Exported {rows} of {n} rows (the loaded pages) to {}; Load more first, or export a SELECT",
                path.display()
            ),
            None if capped => format!(
                "Exported the first {} to {}: the result stops at that many; narrow the SELECT or page through it",
                plural(rows, "row"),
                path.display()
            ),
            None => format!("Exported {} to {}", plural(rows, "row"), path.display()),
        };
        self.status = msg.clone();
        cx.notify();
        Ok(msg)
    }

    /// The whole result (the loaded rows) to the clipboard in one of `EXPORT_FORMATS`.
    pub(super) fn copy_all(&mut self, ext: &str, cx: &mut Context<Self>) {
        let label = EXPORT_FORMATS
            .iter()
            .find(|f| f.0 == ext)
            .map_or(ext, |f| f.1);
        self.status = match self.export_text(ext, cx) {
            Ok((text, rows)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                format!("Copied {} as {label} to the clipboard", plural(rows, "row"))
            }
            Err(e) => e,
        };
        cx.notify();
    }

    /// Ask for a file name (`<table or result>.<ext>`), then write the result there.
    pub(super) fn export_to_file(
        &mut self,
        ext: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Table(n)) => n.replace(['.', '/', '\\'], "_"),
            _ => "result".into(),
        };
        let rx = cx.prompt_for_new_path(
            &dirs::home_dir().unwrap_or_default(),
            Some(&format!("{name}.{ext}")),
        );
        let ext = ext.to_string();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    if let Err(e) = v.export_csv(&path, Some(&ext), cx) {
                        v.status = e;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}

/// Result export formats: (file extension, label).
pub(super) const EXPORT_FORMATS: [(&str, &str); 6] = [
    ("csv", "CSV"),
    ("tsv", "TSV (pastes into spreadsheets)"),
    ("json", "JSON"),
    ("md", "Markdown table"),
    ("sql", "SQL INSERT statements"),
    ("txt", "Plain-text table"),
];

/// "Save to file" and "Copy to clipboard" entries for a menu; with both, each group gets a heading.
pub(super) fn export_menu(
    menu: PopupMenu,
    view: &WeakEntity<AppView>,
    copy: bool,
    save: bool,
) -> PopupMenu {
    let both = copy && save;
    let mut menu = menu;
    if save {
        if both {
            menu = menu.label("Save to file");
        }
        for (ext, label) in EXPORT_FORMATS {
            menu = menu.item(menu_item(
                view,
                &format!("Save as {label}…"),
                move |v, w, cx| v.export_to_file(ext, w, cx),
            ));
        }
    }
    if both {
        menu = menu.separator().label("Copy to clipboard");
    }
    if copy {
        for (ext, label) in EXPORT_FORMATS {
            menu = menu.item(menu_item(
                view,
                &format!("Copy as {label}"),
                move |v, _, cx| v.copy_all(ext, cx),
            ));
        }
    }
    menu
}
