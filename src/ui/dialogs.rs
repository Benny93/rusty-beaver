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
        let Some(tab) = self.tab_ix(p.tab) else {
            cx.notify();
            return done(self, Err("the tab was closed".into()), window, cx);
        };
        self.tabs[tab].param_values.extend(values.clone());
        match db::bind(&p.sql, &values) {
            Ok(bound) => self.run(tab, Source::Raw(bound), p.max_rows, window, cx, done),
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
                        .child(
                            v_flex()
                                .id("params-list")
                                .h_auto()
                                .gap_3()
                                .max_h(px(360.))
                                .overflow_y_scrollbar()
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
                                })),
                        )
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
                host: "localhost".into(),
                ..Default::default()
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
            auth: c.auth,
            ssl: c.ssl,
            vault_path: input(
                if c.auth == Auth::VaultDynamic {
                    "database/creds/readonly"
                } else {
                    "secret/data/team/db"
                },
                &c.vault_path,
                window,
                cx,
            ),
            vault_field: input("password", &c.vault_field, window, cx),
            vault_addr: input(
                &std::env::var("VAULT_ADDR").unwrap_or_else(|_| "https://vault.example.com".into()),
                &c.vault_addr,
                window,
                cx,
            ),
            vault_role: input("default", &c.vault_role, window, cx),
            vault_mount: input("oidc", &c.vault_mount, window, cx),
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
        cfg.id = self.conns[ix].cfg.id.clone();
        let old = std::mem::replace(&mut self.conns[ix].cfg, cfg.clone());
        // what the connection itself does not use: changing it keeps the session (and an open
        // transaction, which reconnecting would roll back)
        let renamed_only = ConnConfig {
            name: cfg.name.clone(),
            keychain: cfg.keychain,     // the dialog does not carry it
            production: cfg.production, // asked about by the app, not the server
            ..old.clone()
        };
        if serde_json::to_string(&renamed_only).ok() != serde_json::to_string(&cfg).ok() {
            self.disconnect(id, cx);
        }
        for t in self.tabs.iter_mut().filter(|t| t.conn == id) {
            match t.kind {
                TabKind::Sql => {
                    t.title = t
                        .title
                        .replace(&format!("<{}>", old.name), &format!("<{}>", cfg.name))
                }
                TabKind::Diagram => t.title = format!("ER: {}", cfg.name),
                TabKind::Table(_) => {}
            }
        }
        if old.name != cfg.name {
            // history entries name their connection too
            for e in self.history.iter_mut().filter(|e| e.connection == old.name) {
                e.connection = cfg.name.clone();
            }
            crate::history::save_all(&self.history);
        }
        self.save();
        self.write_session(cx); // tabs are saved by connection name: now, not in a second
        self.status = format!("Updated connection {}", cfg.name);
        cx.notify();
        Ok(())
    }

    /// The dialog's port: empty is the driver's default, anything else must be 1-65535 (it
    /// used to become the default silently, connecting somewhere else than typed).
    fn dialog_port(&self, cx: &App) -> Result<u16, String> {
        let d = self.dialog.as_ref().ok_or("no dialog")?;
        let s = d.port.read(cx).value().trim().to_string();
        if s.is_empty() || d.driver == Driver::Sqlite {
            return Ok(d.driver.default_port());
        }
        s.parse::<u16>()
            .ok()
            .filter(|&p| p != 0)
            .ok_or(format!("Port: {s:?} is not a number from 1 to 65535"))
    }

    /// `dialog_config`, or why not: shown in the dialog.
    fn checked_dialog_config(&mut self, cx: &mut Context<Self>) -> Option<ConnConfig> {
        if let Err(e) = self.dialog_port(cx) {
            if let Some(d) = self.dialog.as_mut() {
                d.test = Some(Err(e));
            }
            cx.notify();
            return None;
        }
        self.dialog_config(cx)
    }

    pub(super) fn dialog_config(&self, cx: &App) -> Option<ConnConfig> {
        let d = self.dialog.as_ref()?;
        let v = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let sqlite = d.driver == Driver::Sqlite;
        let mut database = v(&d.database);
        if database.is_empty() && sqlite {
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
            port: self.dialog_port(cx).unwrap_or(d.driver.default_port()),
            database,
            // Each mode keeps only its own fields.
            user: if d.auth == Auth::VaultDynamic || sqlite {
                String::new()
            } else {
                v(&d.user)
            },
            password: if d.auth == Auth::Password && !sqlite {
                d.password.read(cx).value().to_string()
            } else {
                String::new()
            },
            auth: if sqlite { Auth::Password } else { d.auth },
            ssl: d.ssl,
            vault_path: if d.auth.is_vault() && !sqlite {
                v(&d.vault_path)
            } else {
                String::new()
            },
            vault_field: if d.auth == Auth::VaultKv && !sqlite {
                v(&d.vault_field)
            } else {
                String::new()
            },
            production: d.production,
            read_only: d.read_only,
            vault_addr: if d.auth.is_vault() {
                v(&d.vault_addr)
            } else {
                String::new()
            },
            vault_role: if d.auth.is_vault() {
                v(&d.vault_role)
            } else {
                String::new()
            },
            vault_mount: if d.auth.is_vault() {
                v(&d.vault_mount)
            } else {
                String::new()
            },
            ..Default::default()
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
        let mut cfg = cfg;
        crate::vault::upgrade_legacy(&mut cfg);
        if cfg.id.is_empty() {
            cfg.id = crate::secrets::new_id();
        }
        cfg.keychain = false;
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
            waiting: vec![],
            generation: 0,
        });
        self.selected = Some((id, None));
        self.save();
        cx.notify();
        Ok(id)
    }

    pub(super) fn delete_connection(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut waiting = vec![];
        if let Some(c) = self.conns.iter_mut().find(|c| c.id == id) {
            db::forget_password(&c.cfg);
            waiting = std::mem::take(&mut c.waiting);
        }
        self.conns.retain(|c| c.id != id);
        let active = self.tab_key(self.active);
        self.tabs.retain(|t| t.conn != id);
        self.active = active
            .and_then(|k| self.tab_ix(k))
            .unwrap_or(self.active)
            .min(self.tabs.len().saturating_sub(1));
        if self.selected.as_ref().is_some_and(|s| s.0 == id) {
            self.selected = None;
        }
        self.save();
        self.save_session_soon(cx);
        self.ensure_loaded(self.active, window, cx); // a restored tab not shown yet
        // Callers waiting for its connect get an answer, once it is gone: earlier, one could
        // queue itself again on the connection.
        for w in waiting {
            w(self, Err("connection is gone".into()), window, cx);
        }
        cx.notify();
    }

    pub(super) fn dialog(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        let section = |title: &str| {
            div()
                .pt_1()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(t.muted_foreground)
                .child(title.to_uppercase())
        };
        let note = |text: &str| {
            div()
                .text_xs()
                .text_color(t.muted_foreground)
                .child(text.to_string())
        };
        let auth_modes = h_flex().gap_1().children(Auth::ALL.map(|mode| {
            pick(
                Button::new(SharedString::from(format!("auth-{mode:?}"))).small(),
                d.auth == mode,
            )
            .label(mode.label())
            .on_click(cx.listener(move |v, _, w, cx| {
                if let Some(d) = v.dialog.as_mut() {
                    d.auth = mode;
                    d.test = None;
                    let hint = if mode == Auth::VaultDynamic {
                        "database/creds/readonly"
                    } else {
                        "secret/data/team/db"
                    };
                    d.vault_path
                        .update(cx, |i, cx| i.set_placeholder(hint, w, cx));
                }
                cx.notify();
            }))
        }));
        let tls_modes = h_flex().gap_1().children(db::Ssl::ALL.map(|mode| {
            pick(
                Button::new(SharedString::from(format!("ssl-{mode:?}"))).small(),
                d.ssl == mode,
            )
            .label(mode.label())
            .on_click(cx.listener(move |v, _, _, cx| {
                if let Some(d) = v.dialog.as_mut() {
                    d.ssl = mode;
                    d.test = None;
                }
                cx.notify();
            }))
        }));
        let tls_help = match d.ssl {
            db::Ssl::Prefer => {
                "TLS when the server offers it, else plain. The certificate is not checked."
            }
            db::Ssl::Require => "TLS or fail. The certificate is not checked.",
            db::Ssl::VerifyFull => {
                "TLS, and the certificate must be trusted by the system and match the host."
            }
            db::Ssl::Disable => {
                "Plain TCP, no encryption. Poolers like Supavisor then need the tenant in the user name."
            }
        };
        let auth_help = match d.auth {
            Auth::Password if crate::secrets::enabled() => format!(
                "Type the password once: it is saved in the {}, not in the connections file.",
                db::keychain_name()
            ),
            Auth::Password => {
                "Keychain is off (RUSTY_BEAVER_KEYCHAIN=off): the password is saved in the connections file.".into()
            }
            Auth::VaultKv => "The password is read from a field of a Vault KV secret on every connect; nothing is saved locally.".into(),
            Auth::VaultDynamic => "Vault's database secrets engine creates a temporary user and password on every connect.".into(),
        };
        let auth_help = auth_help.as_str();
        let test = d.test.clone().map(|r| match r {
            Ok(m) => div().text_sm().text_color(t.success).child(m),
            Err(e) => h_flex()
                .gap_1()
                .items_start()
                .child(self.copy_button("copy-dialog-error", e.clone(), cx))
                .child(div().text_sm().text_color(t.danger).child(e)),
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
                        .w(px(560.))
                        .max_h(relative(0.92))
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
                        // Only the form scrolls: title and buttons stay in view.
                        .child(v_flex()
                            .id("dialog-form")
                            // Room left for the form under the title and above the buttons.
                            .max_h((window.viewport_size().height * 0.92 - px(170.)).max(px(120.))).overflow_y_scrollbar().gap_3()
                        .child(section("Connection"))
                        .child(drivers)
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
                                v_flex()
                                    .gap_1()
                                    .child(div().text_sm().text_color(t.muted_foreground).child("TLS"))
                                    .child(tls_modes),
                            )
                            .child(note(tls_help))
                        })
                        .when(!sqlite, |c| {
                            c.child(section("Authentication"))
                                .child(auth_modes)
                                .child(note(auth_help))
                                .map(|c| match d.auth {
                                    Auth::Password => c.child(
                                        h_flex()
                                            .gap_2()
                                            .child(div().flex_1().child(field("Username", &d.user)))
                                            .child(div().flex_1().child(field("Password", &d.password))),
                                    ),
                                    Auth::VaultKv => c
                                        .child(field("Username", &d.user))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(div().flex_1().child(field("Secret path", &d.vault_path)))
                                                .child(div().w(px(130.)).child(field("Field", &d.vault_field))),
                                        ),
                                    Auth::VaultDynamic => c.child(field("Credentials path", &d.vault_path)),
                                })
                                .when(d.auth.is_vault(), |c| {
                                    c.child(
                                        h_flex()
                                            .gap_2()
                                            .child(div().flex_1().child(field("Vault address", &d.vault_addr)))
                                            .child(div().w(px(120.)).child(field("OIDC role", &d.vault_role)))
                                            .child(div().w(px(90.)).child(field("Auth mount", &d.vault_mount))),
                                    )
                                        .child(note(
                                            "Sign-in: reuses your `vault login` token (VAULT_TOKEN, ~/.vault-token), else opens the browser for OpenID Connect like `vault login -method=oidc`. Role and mount are optional.",
                                        ))
                                })
                        })
                        .child(section("Safety"))
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
                        .child(note("Production asks before changes; read-only refuses them.")))
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
                                            let Some(cfg) = v.checked_dialog_config(cx) else {
                                                return;
                                            };
                                            // this dialog, not one opened after it closed
                                            let this_dialog =
                                                v.dialog.as_ref().map(|d| d.name.entity_id());
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
                                                    if let Some(d) = v.dialog.as_mut().filter(|d| {
                                                        Some(d.name.entity_id()) == this_dialog
                                                    }) {
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
                                        let Some(cfg) = v.checked_dialog_config(cx) else {
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
