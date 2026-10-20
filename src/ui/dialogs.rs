//! Connection dialog (new / edit) and the query parameter dialog.

use super::*;

impl AppView {
    /// Close the parameter dialog: run with the typed values, or cancel.
    pub(super) fn submit_params(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.params.take() else {
            return;
        };
        let done = p.done.take().unwrap_or_else(|| Box::new(|_, _, _, _| {}));
        if !ok {
            cx.notify();
            return done(self, Err("cancelled".into()), window, cx);
        }
        let values: std::collections::HashMap<String, String> = p
            .inputs
            .iter()
            .map(|(n, i)| (n.clone(), i.read(cx).value().to_string()))
            .collect();
        if let Some(t) = self.tabs.get_mut(p.tab) {
            t.param_values.extend(values.clone());
        }
        match db::bind(&p.sql, &values) {
            Ok(bound) => self.run(p.tab, Source::Raw(bound), p.max_rows, window, cx, done),
            Err(e) => done(self, Err(e), window, cx),
        }
        cx.notify();
    }

    pub(super) fn params_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.params.as_ref()?;
        let t = cx.theme().clone();
        Some(
            div()
                .id("params-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .bg(gpui_kit::black().opacity(0.5))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .w(px(480.))
                        .gap_3()
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.background)
                        .shadow_lg()
                        .key_context("CellEdit")
                        .on_action(
                            cx.listener(|v, _: &CancelEdit, w, cx| v.submit_params(false, w, cx)),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .child("Query parameters"),
                        )
                        .child(div().text_xs().text_color(t.muted_foreground).child(
                            "Values are inserted as typed: quote text ('Private'), numbers as is.",
                        ))
                        .children(p.inputs.iter().map(|(n, input)| {
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(120.))
                                        .font_family(t.mono_font_family.clone())
                                        .text_color(Hsla::from(BRAND))
                                        .child(format!(":{n}")),
                                )
                                .child(div().flex_1().child(Input::new(input)))
                        }))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1())
                                .child(
                                    Button::new("params-cancel")
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(
                                            cx.listener(|v, _, w, cx| {
                                                v.submit_params(false, w, cx)
                                            }),
                                        ),
                                )
                                .child(
                                    Button::new("params-run")
                                        .primary()
                                        .icon(IconName::Play)
                                        .label("Execute")
                                        .on_click(
                                            cx.listener(|v, _, w, cx| v.submit_params(true, w, cx)),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The New Connection dialog, or with `edit` the Edit Connection dialog for that one.
    pub(super) fn open_dialog(
        &mut self,
        edit: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            |placeholder: &str, value: &str, window: &mut Window, cx: &mut Context<Self>| {
                let (p, v) = (placeholder.to_string(), value.to_string());
                cx.new(|cx| InputState::new(window, cx).placeholder(p).default_value(v))
            };
        let dir = dirs::home_dir().unwrap_or_default().join("database.db");
        let c = edit
            .and_then(|id| self.conn_ix(id))
            .map(|i| self.conns[i].cfg.clone())
            .unwrap_or(ConnConfig {
                name: String::new(),
                driver: Driver::Sqlite,
                host: "localhost".into(),
                port: 0,
                database: String::new(),
                user: String::new(),
                password: String::new(),
                manual_commit: false,
                production: false,
                read_only: false,
            });
        let port = if c.port == 0 {
            String::new()
        } else {
            c.port.to_string()
        };
        self.dialog = Some(ConnDialog {
            driver: c.driver,
            name: input("My database", &c.name, window, cx),
            host: input("localhost", &c.host, window, cx),
            port: input("5432", &port, window, cx),
            database: input(&dir.to_string_lossy(), &c.database, window, cx),
            user: input("user", &c.user, window, cx),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("password")
                    .masked(true)
                    .default_value(c.password.clone())
            }),
            test: None,
            testing: false,
            editing: edit,
            production: c.production,
            read_only: c.read_only,
        });
        cx.notify();
    }

    /// Replace a saved connection's settings; reconnects lazily when they changed.
    pub(super) fn update_connection(
        &mut self,
        id: u64,
        cfg: ConnConfig,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if cfg.name.trim().is_empty() {
            return Err("name is required".into());
        }
        if self
            .conns
            .iter()
            .any(|c| c.id != id && c.cfg.name.eq_ignore_ascii_case(&cfg.name))
        {
            return Err(format!("a connection named {:?} exists", cfg.name));
        }
        let ix = self.conn_ix(id).ok_or("connection is gone")?;
        let mut cfg = cfg;
        cfg.manual_commit = self.conns[ix].cfg.manual_commit;
        let old = std::mem::replace(&mut self.conns[ix].cfg, cfg.clone());
        let renamed_only = ConnConfig {
            name: cfg.name.clone(),
            ..old.clone()
        };
        if serde_json::to_string(&renamed_only).ok() != serde_json::to_string(&cfg).ok() {
            self.disconnect(id, cx);
        }
        for t in self
            .tabs
            .iter_mut()
            .filter(|t| t.conn == id && t.kind == TabKind::Sql)
        {
            t.title = t
                .title
                .replace(&format!("<{}>", old.name), &format!("<{}>", cfg.name));
        }
        self.save();
        self.status = format!("Updated connection {}", cfg.name);
        cx.notify();
        Ok(())
    }

    pub(super) fn dialog_config(&self, cx: &App) -> Option<ConnConfig> {
        let d = self.dialog.as_ref()?;
        let v = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let mut database = v(&d.database);
        if database.is_empty() && d.driver == Driver::Sqlite {
            database = dirs::home_dir()
                .unwrap_or_default()
                .join("database.db")
                .to_string_lossy()
                .into_owned();
        }
        let name = Some(v(&d.name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                format!(
                    "{} {}",
                    d.driver.label(),
                    if database.is_empty() {
                        v(&d.host)
                    } else {
                        database.clone()
                    }
                )
            });
        Some(ConnConfig {
            name,
            driver: d.driver,
            host: v(&d.host),
            port: v(&d.port).parse().unwrap_or(d.driver.default_port()),
            database,
            user: v(&d.user),
            password: d.password.read(cx).value().to_string(),
            manual_commit: false,
            production: d.production,
            read_only: d.read_only,
        })
    }

    pub(super) fn add_connection(
        &mut self,
        cfg: ConnConfig,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if cfg.name.trim().is_empty() {
            return Err("name is required".into());
        }
        if self
            .conns
            .iter()
            .any(|c| c.cfg.name.eq_ignore_ascii_case(&cfg.name))
        {
            return Err(format!("a connection named {:?} exists", cfg.name));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.status = format!("Added connection {}", cfg.name);
        self.conns.push(Conn {
            id,
            cfg,
            session: None,
            status: ConnStatus::Disconnected,
            expanded: false,
            tables: vec![],
            columns: vec![],
            catalog: Default::default(),
            canceller: None,
            server: None,
            in_tx: false,
            tx_statements: 0,
        });
        self.selected = Some((id, None));
        self.save();
        cx.notify();
        Ok(id)
    }

    pub(super) fn delete_connection(&mut self, id: u64, cx: &mut Context<Self>) {
        self.conns.retain(|c| c.id != id);
        self.tabs.retain(|t| t.conn != id);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if self.selected.as_ref().is_some_and(|s| s.0 == id) {
            self.selected = None;
        }
        self.save();
        cx.notify();
    }

    pub(super) fn dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.dialog.as_ref()?;
        let t = cx.theme().clone();
        let field = |label: &str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(t.muted_foreground)
                        .child(label.to_string()),
                )
                .child(Input::new(input))
        };
        let sqlite = d.driver == Driver::Sqlite;
        let drivers = h_flex().gap_1().children(Driver::ALL.map(|drv| {
            pick(
                Button::new(SharedString::from(format!("drv-{drv:?}"))).small(),
                d.driver == drv,
            )
            .icon(IconName::Database)
            .label(drv.label())
            .on_click(cx.listener(move |v, _, w, cx| {
                if let Some(d) = v.dialog.as_mut() {
                    d.driver = drv;
                    d.test = None;
                    let port = if drv == Driver::Sqlite {
                        String::new()
                    } else {
                        drv.default_port().to_string()
                    };
                    d.port.update(cx, |p, cx| p.set_value(port, w, cx));
                }
                cx.notify();
            }))
        }));
        let test = d.test.clone().map(|r| match r {
            Ok(m) => div().text_sm().text_color(t.success).child(m),
            Err(e) => div().text_sm().text_color(t.danger).child(e),
        });
        Some(
            div()
                .id("dialog-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .bg(gpui_kit::black().opacity(0.5))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .id("dialog")
                        .w(px(520.))
                        .gap_3()
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Icon::new(IconName::DatabaseZap).size_5())
                                .child(div().text_lg().font_weight(FontWeight::BOLD).child(
                                    if d.editing.is_some() {
                                        "Edit connection"
                                    } else {
                                        "Connect to a database"
                                    },
                                )),
                        )
                        .child(drivers)
                        .child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    Button::new("dlg-prod")
                                        .small()
                                        .map(|b| {
                                            if d.production {
                                                b.danger()
                                            } else {
                                                b.outline()
                                            }
                                        })
                                        .icon(IconName::CircleX)
                                        .label(if d.production {
                                            "Production"
                                        } else {
                                            "Development"
                                        })
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            if let Some(d) = v.dialog.as_mut() {
                                                d.production = !d.production;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("dlg-ro")
                                        .small()
                                        .map(|b| {
                                            if d.read_only {
                                                b.warning()
                                            } else {
                                                b.outline()
                                            }
                                        })
                                        .label(if d.read_only {
                                            "Read-only"
                                        } else {
                                            "Read-write"
                                        })
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            if let Some(d) = v.dialog.as_mut() {
                                                d.read_only = !d.read_only;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.muted_foreground)
                                .child("Production asks before changes; read-only refuses them."),
                        )
                        .child(field("Connection name", &d.name))
                        .when(!sqlite, |c| {
                            c.child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(field("Host", &d.host)))
                                    .child(div().w(px(110.)).child(field("Port", &d.port))),
                            )
                        })
                        .child(field(
                            if sqlite {
                                "Database file (created if missing)"
                            } else {
                                "Database"
                            },
                            &d.database,
                        ))
                        .when(!sqlite, |c| {
                            c.child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(field("Username", &d.user)))
                                    .child(div().flex_1().child(field("Password", &d.password))),
                            )
                        })
                        .children(test)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("dlg-test")
                                        .outline()
                                        .loading(d.testing)
                                        .label("Test Connection")
                                        .on_click(cx.listener(|v, _, w, cx| {
                                            let Some(cfg) = v.dialog_config(cx) else {
                                                return;
                                            };
                                            if let Some(d) = v.dialog.as_mut() {
                                                d.testing = true;
                                            }
                                            cx.spawn_in(w, async move |this, cx| {
                                                let r = cx
                                                    .background_spawn(async move {
                                                        db::connect(&cfg)
                                                            .and_then(|mut s| s.tables())
                                                            .map(|t| {
                                                                format!(
                                                                    "Connected: {} tables",
                                                                    t.len()
                                                                )
                                                            })
                                                    })
                                                    .await;
                                                let _ = this.update(cx, |v, cx| {
                                                    if let Some(d) = v.dialog.as_mut() {
                                                        (d.test, d.testing) = (Some(r), false);
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .detach();
                                            cx.notify();
                                        })),
                                )
                                .child(div().flex_1())
                                .child(Button::new("dlg-cancel").ghost().label("Cancel").on_click(
                                    cx.listener(|v, _, _, cx| {
                                        v.dialog = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(Button::new("dlg-save").primary().label("Finish").on_click(
                                    cx.listener(|v, _, w, cx| {
                                        let Some(cfg) = v.dialog_config(cx) else {
                                            return;
                                        };
                                        if let Some(id) = v.dialog.as_ref().and_then(|d| d.editing)
                                        {
                                            match v.update_connection(id, cfg, cx) {
                                                Ok(()) => v.dialog = None,
                                                Err(e) => {
                                                    if let Some(d) = v.dialog.as_mut() {
                                                        d.test = Some(Err(e));
                                                    }
                                                }
                                            }
                                            cx.notify();
                                            return;
                                        }
                                        match v.add_connection(cfg, cx) {
                                            Ok(id) => {
                                                v.dialog = None;
                                                v.connect(id, w, cx, Box::new(|_, _, _, _| {}));
                                            }
                                            Err(e) => {
                                                if let Some(d) = v.dialog.as_mut() {
                                                    d.test = Some(Err(e));
                                                }
                                            }
                                        }
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}
