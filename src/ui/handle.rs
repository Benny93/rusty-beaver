//! MCP commands: each agent tool call lands in `handle` and changes what the user sees.

use super::*;

impl AppView {
    pub(super) fn handle(&mut self, call: mcp::Call, window: &mut Window, cx: &mut Context<Self>) {
        let reply = call.reply;
        let a = call.args;
        let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
        let max_rows = a["max_rows"].as_u64().map(|n| n as usize).unwrap_or(50);
        let done: Done = Box::new(move |_, r, _, _| {
            let _ = reply.send(r);
        });
        if call.name == "reveal" {
            crate::platform::order_front_quietly();
            window.refresh();
            return done(self, Ok(json!({})), window, cx);
        }
        // Any agent command skips the splash, so screenshots show the app.
        self.started = Instant::now() - SPLASH_HOLD - SPLASH_FADE;
        let now = |v: &mut Self,
                   r: Result<Value, String>,
                   window: &mut Window,
                   cx: &mut Context<Self>,
                   done: Done| {
            cx.notify();
            done(v, r, window, cx)
        };
        match call.name {
            "get_state" => {
                let st = self.state_json(cx);
                now(self, Ok(st), window, cx, done)
            }
            "add_connection" => {
                let r = serde_json::from_value::<ConnConfig>(a)
                    .map_err(|e| e.to_string())
                    .and_then(|mut c| {
                        if c.port == 0 {
                            c.port = c.driver.default_port();
                        }
                        self.add_connection(c, cx)
                    })
                    .map(|id| json!({ "added": id }));
                now(self, r, window, cx, done)
            }
            "delete_connection" => match self.conn_by_name(&s("connection")) {
                Ok(id) => {
                    self.delete_connection(id, window, cx);
                    now(
                        self,
                        Ok(json!({ "deleted": s("connection") })),
                        window,
                        cx,
                        done,
                    )
                }
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "connect" => match self.conn_by_name(&s("connection")) {
                Ok(id) => {
                    self.selected = Some((id, None));
                    self.connect(id, window, cx, done)
                }
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "disconnect" => match self.conn_by_name(&s("connection")) {
                Ok(id) => {
                    self.disconnect(id, cx);
                    now(
                        self,
                        Ok(json!({ "disconnected": s("connection") })),
                        window,
                        cx,
                        done,
                    )
                }
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "describe_table" => match self.conn_by_name(&s("connection")) {
                Ok(id) => {
                    self.selected = Some((id, Some(s("table"))));
                    if let Some(ix) = self.conn_ix(id) {
                        self.conns[ix].expanded = true;
                    }
                    self.describe(id, s("table"), window, cx, done)
                }
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "open_table" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.open_table(id, s("table"), window, cx, done),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "new_sql_editor" => {
                let conn = a["connection"].as_str().map(|n| self.conn_by_name(n));
                let r = match conn.transpose() {
                    Ok(c) => self
                        .new_editor(c, window, cx)
                        .map(|i| json!({ "tab": i, "title": self.tabs[i].title })),
                    Err(e) => Err(e),
                };
                now(self, r, window, cx, done)
            }
            "set_sql" => {
                let r = match self.tabs.get(self.active) {
                    Some(t) => {
                        let text = s("text");
                        t.editor.update(cx, |e, cx| e.set_value(text, window, cx));
                        self.save_session_soon(cx);
                        Ok(json!({ "tab": self.active }))
                    }
                    None => Err("no tab open; call new_sql_editor first".to_string()),
                };
                now(self, r, window, cx, done)
            }
            "run_sql" => {
                let id = match a["connection"].as_str().map(|n| self.conn_by_name(n)) {
                    Some(Err(e)) => return now(self, Err(e), window, cx, done),
                    Some(Ok(id)) => Some(id),
                    None => self.tabs.get(self.active).map(|t| t.conn),
                };
                if let Some(id) = id
                    && let Err(e) = self.activate_script_tab(id, window, cx)
                {
                    return now(self, Err(e), window, cx, done);
                }
                if self.tabs.is_empty()
                    && let Err(e) = self.new_editor(None, window, cx)
                {
                    return now(self, Err(e), window, cx, done);
                }
                let sql = a["sql"].as_str().map(String::from);
                if let (Some(params), Some(t)) =
                    (a["params"].as_object(), self.tabs.get_mut(self.active))
                {
                    for (k, v) in params {
                        let v = v
                            .as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string());
                        t.param_values
                            .insert(k.trim_start_matches(':').to_string(), v);
                    }
                }
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.params_given = true; // agents never get a dialog: missing values are an error
                    (t.agent, t.confirmed) = (true, a["confirm"].as_bool().unwrap_or(false));
                }
                self.run(
                    self.active,
                    sql.map(Source::Given).unwrap_or(Source::Script),
                    max_rows,
                    window,
                    cx,
                    done,
                )
            }
            "get_results" if a["index"].is_u64() => {
                let r = self.show_result(
                    self.active,
                    a["index"].as_u64().unwrap_or(0) as usize,
                    max_rows,
                    cx,
                );
                now(self, r, window, cx, done)
            }
            "get_results" => {
                let r = self
                    .tabs
                    .get(self.active)
                    .map(|t| result_json(&t.grid.read(cx).delegate().shown(), max_rows))
                    .ok_or("no tab open".to_string());
                now(self, r, window, cx, done)
            }
            "select_tab" | "close_tab" => {
                let i = a["index"].as_u64().unwrap_or(u64::MAX) as usize;
                let r = if i >= self.tabs.len() {
                    Err(format!("no tab {i}; {} open", self.tabs.len()))
                } else {
                    if call.name == "select_tab" {
                        self.select_tab_ix(i, window, cx);
                        Ok(())
                    } else {
                        self.close_tab(i, window, cx)
                    }
                    .map(|_| json!({ "active": self.active, "tabs": self.tabs.len() }))
                };
                now(self, r, window, cx, done)
            }
            "set_table_view" => {
                let r = match self.tabs.get_mut(self.active) {
                    Some(t) if matches!(t.kind, TabKind::Table(_)) => {
                        t.view = match s("view").as_str() {
                            "columns" => TableView::Columns,
                            "ddl" => TableView::Ddl,
                            "data" => TableView::Data,
                            v => {
                                let e = format!("view {v:?}: data, columns or ddl");
                                return now(self, Err(e), window, cx, done);
                            }
                        };
                        let ddl =
                            (t.view == TableView::Ddl).then(|| t.ddl.read(cx).value().to_string());
                        Ok(json!({ "view": format!("{:?}", t.view).to_lowercase(), "ddl": ddl }))
                    }
                    _ => Err("the active tab is not a table tab".to_string()),
                };
                now(self, r, window, cx, done)
            }
            "set_filter" => {
                let Some(t) = self
                    .tabs
                    .get(self.active)
                    .filter(|t| matches!(t.kind, TabKind::Table(_)))
                else {
                    return now(
                        self,
                        Err("the active tab is not a table tab; call open_table first".into()),
                        window,
                        cx,
                        done,
                    );
                };
                if !t.grid.read(cx).delegate().changes.is_empty() {
                    // before the filter box changes: it would show a filter not applied
                    let e = "save or revert the pending changes first".to_string();
                    return now(self, Err(e), window, cx, done);
                }
                let f = s("filter");
                t.filter.update(cx, |i, cx| i.set_value(f, window, cx));
                self.tabs[self.active].limit = PAGE;
                self.reload_table(self.active, window, cx, done)
            }
            "edit_cell" | "delete_row" => {
                let r = self.cell_arg(&a, cx).and_then(|(row, col)| {
                    self.tabs[self.active].cell = Some((row, col));
                    if call.name == "delete_row" {
                        self.grid_op(GridOp::DeleteRow, window, cx, Box::new(|_, _, _, _| {}))?;
                    } else {
                        if !matches!(self.tabs[self.active].kind, TabKind::Table(_)) {
                            return Err("query results are read-only; open_table to edit".into());
                        }
                        let id = self.tabs[self.active]
                            .grid
                            .read(cx)
                            .delegate()
                            .row_id(row)
                            .unwrap_or_default();
                        self.set_cell(
                            self.active,
                            id,
                            col,
                            a["value"].as_str().map(String::from),
                            cx,
                        );
                    }
                    Ok(self.changes_json(cx))
                });
                now(self, r, window, cx, done)
            }
            "add_row" => {
                // every column checked before the row exists: a bad name adds nothing
                let cols = self
                    .tabs
                    .get(self.active)
                    .map(|t| t.grid.read(cx).delegate().r.columns.clone())
                    .unwrap_or_default();
                let values: Result<Vec<(usize, Option<String>)>, String> = a["values"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        let col = cols
                            .iter()
                            .position(|c| c.eq_ignore_ascii_case(k))
                            .ok_or(format!("no column {k:?}; have {}", cols.join(", ")))?;
                        let v = match v {
                            Value::Null => None,
                            Value::String(s) => Some(s.clone()),
                            v => Some(v.to_string()), // 42, true
                        };
                        Ok((col, v))
                    })
                    .collect();
                let r = values.and_then(|values| {
                    self.grid_op(GridOp::AddRow, window, cx, Box::new(|_, _, _, _| {}))?;
                    let ix = self.active;
                    self.finish_edit(ix, None, window, cx);
                    let id = self.tabs[ix].grid.read(cx).delegate().r.rows.len() - 1;
                    for (col, v) in values {
                        self.set_cell(ix, id, col, v, cx);
                    }
                    Ok(self.changes_json(cx))
                });
                now(self, r, window, cx, done)
            }
            "save_changes" => {
                let d: Done = Box::new(move |v, r, w, cx| {
                    let r = r.map(|_| json!({ "saved": true, "status": v.status, "rows": v.tabs.get(v.active).map(|t| t.grid.read(cx).delegate().order.len()) }));
                    done(v, r, w, cx)
                });
                match self.tabs.get_mut(self.active) {
                    Some(t) => {
                        (t.confirmed, t.agent) = (a["confirm"].as_bool().unwrap_or(false), true);
                        let _ = self.grid_op(GridOp::Save, window, cx, d); // calls `d`, also on errors
                    }
                    None => d(self, Err("no tab open".into()), window, cx),
                }
            }
            "revert_changes" => {
                let r = self
                    .grid_op(GridOp::Revert, window, cx, Box::new(|_, _, _, _| {}))
                    .map(|m| json!({ "ok": m }));
                now(self, r, window, cx, done)
            }
            "sort_results" => {
                let r = match self.tabs.get(self.active) {
                    None => Err("no tab open".to_string()),
                    Some(t) => {
                        let grid = t.grid.clone();
                        let col = grid
                            .read(cx)
                            .delegate()
                            .r
                            .columns
                            .iter()
                            .position(|c| c.eq_ignore_ascii_case(&s("column")));
                        match col {
                            None => Err(format!("no column {:?}", s("column"))),
                            Some(col) => {
                                let sort = if a["descending"].as_bool().unwrap_or(false) {
                                    ColumnSort::Descending
                                } else {
                                    ColumnSort::Ascending
                                };
                                grid.update(cx, |g, cx| {
                                    g.delegate_mut().perform_sort(col, sort, window, cx);
                                    cx.notify();
                                });
                                Ok(result_json(&grid.read(cx).delegate().shown(), max_rows))
                            }
                        }
                    }
                };
                now(self, r, window, cx, done)
            }
            "cancel_query" => {
                let i = a["index"]
                    .as_u64()
                    .map(|i| i as usize)
                    // the longest running holds the connection; later ones wait for it
                    .or_else(|| {
                        (0..self.tabs.len())
                            .filter(|&i| self.tabs[i].running)
                            .min_by_key(|&i| self.tabs[i].started)
                    })
                    .unwrap_or(self.active);
                let r = self.cancel(i, cx).map(|_| json!({ "cancelling": i }));
                now(self, r, window, cx, done)
            }
            "explain" => {
                let sql = a["sql"].as_str().map(String::from);
                if self.tabs.is_empty()
                    && let Err(e) = self.new_editor(None, window, cx)
                {
                    return now(self, Err(e), window, cx, done);
                }
                self.explain(self.active, sql, window, cx, done)
            }
            "format_sql" => {
                if let Some(text) = a["sql"].as_str()
                    && let Some(t) = self.tabs.get(self.active)
                {
                    let text = text.to_string();
                    t.editor.update(cx, |e, cx| e.set_value(text, window, cx));
                }
                let r = self
                    .format(self.active, window, cx)
                    .map(|sql| json!({ "sql": sql }));
                now(self, r, window, cx, done)
            }
            "record_view" => {
                let r = match self.tabs.get_mut(self.active) {
                    Some(t) => {
                        let rows = t.grid.read(cx).delegate().order.len();
                        if let Some(row) = a["row"].as_u64().filter(|&r| r as usize >= rows) {
                            let e = format!("no row {row}; the grid has {rows}");
                            return now(self, Err(e), window, cx, done);
                        }
                        t.record = a["on"].as_bool().unwrap_or(!t.record);
                        if let Some(row) = a["row"].as_u64() {
                            t.cell = Some((row as usize, t.cell.map(|c| c.1).unwrap_or(0)));
                        }
                        let g = t.grid.read(cx).delegate();
                        let row = t.cell.map(|c| c.0).unwrap_or(0);
                        let record: serde_json::Map<String, Value> = g
                            .row_id(row)
                            .map(|id| {
                                g.r.columns
                                    .iter()
                                    .enumerate()
                                    .map(|(c, n)| (n.clone(), json!(g.value(id, c))))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Ok(json!({ "record_view": t.record, "row": row, "record": record }))
                    }
                    None => Err("no tab open".to_string()),
                };
                now(self, r, window, cx, done)
            }
            "open_er_diagram" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.open_diagram(id, window, cx, done),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "chart" => {
                let r = match self.tabs.get_mut(self.active) {
                    None => Err("no tab open".to_string()),
                    Some(t) => {
                        t.chart = a["on"].as_bool().unwrap_or(true);
                        let g = t.grid.read(cx).delegate();
                        let (label, numeric) = crate::grid::chart_columns(&g.r);
                        if let Some(name) = a["column"].as_str() {
                            let Some(c) =
                                g.r.columns
                                    .iter()
                                    .position(|c| c.eq_ignore_ascii_case(name))
                                    .filter(|c| numeric.contains(c))
                            else {
                                let e = format!("{name:?} is no numeric column of the result");
                                return now(self, Err(e), window, cx, done);
                            };
                            t.chart_col = Some(c);
                        }
                        Ok(json!({
                            "chart": t.chart,
                            "label_column": label.map(|c| g.r.columns[c].clone()),
                            "value_columns": numeric.iter().map(|&c| g.r.columns[c].clone()).collect::<Vec<_>>(),
                        }))
                    }
                };
                now(self, r, window, cx, done)
            }
            "results_as_text" => {
                let on = a["on"].as_bool().unwrap_or(true);
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.text_view = on;
                }
                let r = self
                    .export_text("txt", cx)
                    .map(|(text, _)| json!({ "text_view": on, "text": text }));
                now(self, r, window, cx, done)
            }
            "duplicate_row" | "open_referenced_row" => {
                let op = if call.name == "duplicate_row" {
                    GridOp::DuplicateRow
                } else {
                    GridOp::FollowFk
                };
                let r = self.cell_arg(&a, cx).and_then(|(row, col)| {
                    self.tabs[self.active].cell = Some((row, col));
                    self.grid_op(op, window, cx, Box::new(|_, _, _, _| {}))
                });
                let r = r.map(|m| json!({ "ok": m }));
                now(self, r, window, cx, done)
            }
            "test_connection" => {
                let cfg = serde_json::from_value::<ConnConfig>(a);
                match cfg {
                    Err(e) => now(self, Err(e.to_string()), window, cx, done),
                    Ok(mut cfg) => {
                        if cfg.port == 0 {
                            cfg.port = cfg.driver.default_port();
                        }
                        cx.spawn_in(window, async move |this, cx| {
                            let r = cx
                                .background_spawn(async move {
                                    let mut s = db::connect(&cfg)?;
                                    let tables = s.tables()?.len();
                                    Ok::<_, String>(json!({ "ok": true, "server": s.server_version(), "tables": tables }))
                                })
                                .await;
                            let _ = this.update_in(cx, |v, w, cx| done(v, r, w, cx));
                        })
                        .detach();
                    }
                }
            }
            "row_counts" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.row_counts(id, window, cx, done),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "close_all_tabs" => {
                let r = self
                    .close_tabs(a["keep_active"].as_bool().unwrap_or(false), cx)
                    .map(|_| json!({ "tabs": self.tabs.len() }));
                now(self, r, window, cx, done)
            }
            "search_results" => {
                let r = match self.tabs.get(self.active) {
                    Some(t) => {
                        let text = s("text");
                        t.search
                            .update(cx, |i, cx| i.set_value(text.clone(), window, cx));
                        t.grid.update(cx, |g, cx| {
                            g.delegate_mut().set_search(&text);
                            cx.notify();
                        });
                        Ok(result_json(&t.grid.read(cx).delegate().shown(), max_rows))
                    }
                    None => Err("no tab open".to_string()),
                };
                now(self, r, window, cx, done)
            }
            "show_value" if a["on"].as_bool() == Some(false) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.value_panel = false;
                }
                now(self, Ok(json!({ "value_panel": false })), window, cx, done)
            }
            "show_value" => {
                // no row: the selected cell
                let mut a = a.clone();
                if a["row"].is_null()
                    && let Some((row, col)) = self.tabs.get(self.active).and_then(|t| t.cell)
                {
                    a["row"] = json!(row);
                    if a["column"].is_null()
                        && let Some(t) = self.tabs.get(self.active)
                    {
                        a["column"] = json!(t.grid.read(cx).delegate().r.columns.get(col));
                    }
                }
                let r = self.cell_arg(&a, cx).map(|(row, col)| {
                    let t = &mut self.tabs[self.active];
                    t.cell = Some((row, col));
                    t.value_panel = true;
                    t.grid.update(cx, |g, cx| g.set_selected_cell(row, col, cx));
                    let g = t.grid.read(cx).delegate();
                    let stats: serde_json::Map<String, Value> = crate::grid::column_stats(g, col).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
                    json!({ "row": row, "column": g.r.columns[col], "value": g.row_id(row).map(|id| g.value(id, col)), "column_stats": stats })
                });
                now(self, r, window, cx, done)
            }
            "set_auto_commit" => {
                let r = self
                    .conn_by_name(&s("connection"))
                    .and_then(|id| {
                        self.set_manual_commit(id, !a["on"].as_bool().unwrap_or(true), cx)
                    })
                    .map(|_| json!({ "auto_commit": a["on"] }));
                now(self, r, window, cx, done)
            }
            "commit" | "rollback" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.end_tx(id, call.name == "commit", window, cx, done),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "import_csv" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.import_csv(
                    id,
                    s("table"),
                    s("path").into(),
                    Some(a["confirm"].as_bool().unwrap_or(false)),
                    window,
                    cx,
                    done,
                ),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "open_script" => {
                let r = self
                    .open_script(std::path::Path::new(&s("path")), window, cx)
                    .map(|i| json!({ "tab": i, "title": self.tabs[i].title }));
                now(self, r, window, cx, done)
            }
            "save_script" => {
                let path = a["path"].as_str().map(std::path::PathBuf::from);
                let r = self
                    .save_script(self.active, path, cx)
                    .map(|m| json!({ "ok": m }));
                now(self, r, window, cx, done)
            }
            "load_more" => {
                let i = self.active;
                self.load_more(i, window, cx, done)
            }
            "edit_connection" => {
                let r = self.conn_by_name(&s("connection")).and_then(|id| {
                    let mut c = self.conns[self.conn_ix(id).unwrap_or(0)].cfg.clone();
                    for (k, field) in [
                        ("name", &mut c.name),
                        ("vault_addr", &mut c.vault_addr),
                        ("vault_role", &mut c.vault_role),
                        ("vault_mount", &mut c.vault_mount),
                        ("vault_path", &mut c.vault_path),
                        ("vault_field", &mut c.vault_field),
                        ("host", &mut c.host),
                        ("database", &mut c.database),
                        ("user", &mut c.user),
                        ("password", &mut c.password),
                    ] {
                        if let Some(v) = a[k].as_str() {
                            *field = v.to_string();
                        }
                    }
                    if let Some(p) = a["port"].as_u64() {
                        c.port = p as u16;
                    }
                    if let Some(m) = a["auth"].as_str() {
                        c.auth = Auth::parse(m).ok_or(format!(
                            "auth: {m:?} is not password, vault_kv or vault_dynamic"
                        ))?;
                    }
                    if let Some(m) = a.get("ssl").filter(|m| !m.is_null()) {
                        c.ssl = serde_json::from_value(m.clone()).map_err(|_| {
                            format!("ssl: {m} is not prefer, disable, require or verify_full")
                        })?;
                    }
                    crate::vault::upgrade_legacy(&mut c);
                    if let Some(p) = a["production"].as_bool() {
                        c.production = p;
                    }
                    if let Some(p) = a["read_only"].as_bool() {
                        c.read_only = p;
                    }
                    self.update_connection(id, c, cx)
                        .map(|_| json!({ "updated": true }))
                });
                now(self, r, window, cx, done)
            }
            "refresh_connection" => match self.conn_by_name(&s("connection")) {
                Ok(id) => self.refresh(id, window, cx, done),
                Err(e) => now(self, Err(e), window, cx, done),
            },
            "generate_sql" => {
                let kind: &'static str = match s("kind").as_str() {
                    "select" => "select",
                    "insert" => "insert",
                    "update" => "update",
                    "delete" => "delete",
                    "count" => "count",
                    _ => {
                        return now(
                            self,
                            Err("kind: select, insert, update, delete or count".into()),
                            window,
                            cx,
                            done,
                        );
                    }
                };
                match self.conn_by_name(&s("connection")) {
                    Ok(id) => self.generate_sql(id, s("table"), kind, window, cx, done),
                    Err(e) => now(self, Err(e), window, cx, done),
                }
            }
            "execute_editor" => {
                // Like pressing Cmd/Ctrl+Enter (or Shift for the script): parameters open the
                // dialog for the user. Answers at once; get_state / get_results show the outcome.
                let src = if a["script"].as_bool().unwrap_or(false) {
                    Source::Script
                } else {
                    Source::Statement
                };
                self.run(
                    self.active,
                    src,
                    max_rows,
                    window,
                    cx,
                    Box::new(|_, _, _, _| {}),
                );
                let r = match &self.params {
                    Some(p) => {
                        json!({ "waiting_for_parameters": p.inputs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>() })
                    }
                    None => json!({ "started": true }),
                };
                now(self, Ok(r), window, cx, done)
            }
            "get_history" => {
                let q = s("search").to_lowercase();
                let limit = a["limit"].as_u64().unwrap_or(20) as usize;
                let now_s = history::now();
                let items: Vec<Value> = self
                    .history
                    .iter()
                    .rev()
                    .filter(|e| q.is_empty() || e.sql.to_lowercase().contains(&q) || e.connection.to_lowercase().contains(&q))
                    .take(limit)
                    .map(|e| json!({ "sql": e.sql, "connection": e.connection, "when": history::age(e.at, now_s), "ms": e.ms, "outcome": e.outcome, "ok": e.ok }))
                    .collect();
                now(self, Ok(json!(items)), window, cx, done)
            }
            "set_sidebar" => {
                let panel = match s("panel").as_str() {
                    "history" => Sidebar::History,
                    "navigator" | "" => Sidebar::Navigator,
                    p => {
                        let e = format!("panel {p:?}: navigator or history");
                        return now(self, Err(e), window, cx, done);
                    }
                };
                self.set_sidebar(panel, s("filter"), window, cx);
                now(
                    self,
                    Ok(json!({ "panel": format!("{:?}", self.sidebar).to_lowercase() })),
                    window,
                    cx,
                    done,
                )
            }
            "sql_completions" => {
                let r = self.conn_by_name(&s("connection")).map(|id| {
                    let sql = s("sql");
                    let offset = a["offset"].as_u64().map(|o| o as usize).unwrap_or(sql.len());
                    let cat = self.conns[self.conn_ix(id).unwrap_or(0)].catalog.clone();
                    let (_, items) = crate::complete::suggest(&sql, offset, &cat.lock().unwrap());
                    json!(items.iter().take(30).map(|i| json!({ "label": i.label, "kind": format!("{:?}", i.kind).to_lowercase(), "detail": i.detail })).collect::<Vec<_>>())
                });
                now(self, r, window, cx, done)
            }
            "export_csv" => {
                let r = self
                    .export_csv(std::path::Path::new(&s("path")), None, cx)
                    .map(|m| json!({ "ok": m }));
                now(self, r, window, cx, done)
            }
            "set_theme" => {
                match s("mode").as_str() {
                    "system" => self.follow_system_theme(window, cx),
                    m @ ("dark" | "light") => self.set_theme(m == "dark", window, cx),
                    m => {
                        let e = format!("mode {m:?}: dark, light or system");
                        return now(self, Err(e), window, cx, done);
                    }
                }
                now(
                    self,
                    Ok(json!({ "dark": self.dark, "follow_system": self.follow_system })),
                    window,
                    cx,
                    done,
                )
            }
            "shortcuts" => {
                self.shortcuts = a["open"].as_bool().unwrap_or(true);
                cx.notify();
                now(
                    self,
                    Ok(json!({ "open": self.shortcuts })),
                    window,
                    cx,
                    done,
                )
            }
            "connection_dialog" => {
                if a["open"].as_bool().unwrap_or(true) {
                    let edit = a["connection"]
                        .as_str()
                        .map(|n| self.conn_by_name(n))
                        .transpose();
                    match edit {
                        Ok(edit) => self.open_dialog(edit, window, cx),
                        Err(e) => return now(self, Err(e), window, cx, done),
                    }
                } else {
                    self.dialog = None
                }
                now(
                    self,
                    Ok(json!({ "open": self.dialog.is_some() })),
                    window,
                    cx,
                    done,
                )
            }
            other => now(
                self,
                Err(format!("unknown command {other}")),
                window,
                cx,
                done,
            ),
        }
    }

    /// (display row, column index) from MCP `row` / `column` arguments.
    pub(super) fn cell_arg(&self, a: &Value, cx: &App) -> Result<(usize, usize), String> {
        let t = self.tabs.get(self.active).ok_or("no tab open")?;
        let g = t.grid.read(cx).delegate();
        let row = a["row"].as_u64().ok_or("row is required")? as usize;
        if row >= g.order.len() {
            return Err(format!("no row {row}; the grid has {}", g.order.len()));
        }
        let col = match a["column"].as_str() {
            Some(name) => {
                g.r.columns
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(name))
                    .ok_or(format!(
                        "no column {name:?}; have {}",
                        g.r.columns.join(", ")
                    ))?
            }
            None => 0,
        };
        Ok((row, col))
    }

    pub(super) fn changes_json(&self, cx: &App) -> Value {
        let g = self.tabs[self.active].grid.read(cx).delegate();
        let c = &g.changes;
        json!({
            "pending_changes": c.count(),
            "edited_cells": c.cells.iter().map(|((r, col), v)| json!({ "row": g.order.iter().position(|x| x == r), "column": g.r.columns[*col], "value": v })).collect::<Vec<_>>(),
            "deleted_rows": c.deleted.iter().map(|r| g.order.iter().position(|x| x == r)).collect::<Vec<_>>(),
            "added_rows": c.inserted.len(),
        })
    }

    pub(super) fn state_json(&self, cx: &App) -> Value {
        let name = |id: u64| self.conn_ix(id).map(|i| self.conns[i].cfg.name.clone());
        json!({
            "connections": self.conns.iter().map(|c| json!({
                "name": c.cfg.name, "driver": c.cfg.driver, "host": c.cfg.host, "port": c.cfg.port, "database": c.cfg.database,
                "status": match &c.status { ConnStatus::Failed(e) => format!("failed: {e}"), s => format!("{s:?}").to_lowercase() },
                "expanded": c.expanded, "tables": c.tables.len(),
                "auth": c.cfg.auth, "password": if c.cfg.auth.is_vault() { "vault" } else if c.cfg.password.is_empty() { "none" } else if crate::secrets::enabled() { "keychain" } else { "file" },
                "production": c.cfg.production, "read_only": c.cfg.read_only, "server": c.server, "auto_commit": !c.cfg.manual_commit, "in_transaction": c.in_tx, "statements_in_transaction": c.tx_statements,
                "expanded_tables": c.columns.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "selected": self.selected.as_ref().map(|(id, t)| json!({ "connection": name(*id), "table": t })),
            "tabs": self.tabs.iter().enumerate().map(|(i, t)| {
                let r = &t.grid.read(cx).delegate().r;
                json!({
                    "index": i, "title": t.title, "active": i == self.active, "connection": name(t.conn),
                    "kind": match &t.kind { TabKind::Sql => "sql".to_string(), TabKind::Table(n) => format!("table:{n}"), TabKind::Diagram => "er_diagram".to_string() },
                    "view": format!("{:?}", t.view).to_lowercase(),
                    "sql": t.editor.read(cx).value().to_string(), "running": t.running,
                    "result": { "columns": r.columns, "rows": r.rows.len() },
                    "filter": t.filter.read(cx).value().to_string(),
                    "total_rows": t.total,
                    "file": t.path.as_ref().map(|p| p.display().to_string()),
                    "pending_changes": t.grid.read(cx).delegate().changes.count(),
                    "selected_cell": t.cell.map(|(row, col)| json!({ "row": row, "column": r.columns.get(col) })),
                    "editing": t.editing.is_some(),
                    "message": t.message.as_ref().map(|m| match m { Ok(s) => json!({ "ok": s }), Err(e) => json!({ "error": e }) }),
                })
            }).collect::<Vec<_>>(),
            "theme": if self.dark { "dark" } else { "light" },
            "sidebar": format!("{:?}", self.sidebar).to_lowercase(),
            "sidebar_filter": self.side_filter.read(cx).value().to_string(),
            "history_entries": self.history.len(),
            "connection_dialog_open": self.dialog.is_some(),
            "status": self.status,
            "mcp": self.mcp,
        })
    }
}
