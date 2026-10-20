//! Left sidebar: Database Navigator tree and query history.

use super::*;

impl AppView {
    pub(super) fn navigator(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let row = |id: SharedString, depth: usize, selected: bool| {
            h_flex()
                .id(id)
                .gap_1()
                .pl(px(6. + depth as f32 * 14.))
                .pr_2()
                .py(px(3.))
                .rounded_sm()
                .text_sm()
                .when(selected, |d| {
                    d.bg(t.list_active).text_color(t.accent_foreground)
                })
                .when(!selected, |d| d.hover(|d| d.bg(t.list_hover)))
        };
        let mut items: Vec<AnyElement> = vec![];
        let view = cx.entity().downgrade();
        for c in &self.conns {
            let id = c.id;
            let selected = self.selected.as_ref() == Some(&(id, None));
            let color = match c.status {
                ConnStatus::Connected => t.success,
                ConnStatus::Connecting => t.warning,
                ConnStatus::Failed(_) => t.danger,
                ConnStatus::Disconnected => t.muted_foreground,
            };
            let expanded = c.expanded;
            items.push(
                row(format!("conn-{id}").into(), 0, selected)
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3(),
                    )
                    .child(Icon::new(IconName::Database).size_4().text_color(color))
                    .child(div().truncate().child(c.cfg.name.clone()))
                    .when(c.cfg.read_only, |d| {
                        d.child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(t.warning)
                                .text_color(gpui_kit::black())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child("RO"),
                        )
                    })
                    .when(c.cfg.production, |d| {
                        d.child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(t.danger)
                                .text_color(gpui_kit::white())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child("PROD"),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(c.cfg.driver.label()),
                    )
                    .on_click(cx.listener(move |v, ev: &ClickEvent, w, cx| {
                        v.selected = Some((id, None));
                        let Some(ix) = v.conn_ix(id) else { return };
                        if v.conns[ix].expanded {
                            v.conns[ix].expanded = false;
                        } else {
                            v.connect(id, w, cx, Box::new(|_, _, _, _| {}));
                        }
                        if ev.click_count() > 1 {
                            let _ = v.new_editor(Some(id), w, cx);
                        }
                        cx.notify();
                    }))
                    .context_menu({
                        let view = view.clone();
                        let connected = c.status == ConnStatus::Connected;
                        let failed = match &c.status {
                            ConnStatus::Failed(e) => Some(e.clone()),
                            _ => None,
                        };
                        move |menu: PopupMenu, _, _| {
                            let menu = match failed.clone() {
                                Some(e) => menu
                                    .item(menu_item(&view, "Copy Error", move |v, _, cx| {
                                        v.copy_text("copy-conn-error", e.clone(), cx)
                                    }))
                                    .separator(),
                                None => menu,
                            };
                            menu.item(menu_item(
                                &view,
                                if connected { "Disconnect" } else { "Connect" },
                                move |v, w, cx| {
                                    if connected {
                                        v.disconnect(id, cx)
                                    } else {
                                        v.connect(id, w, cx, Box::new(|_, _, _, _| {}))
                                    }
                                },
                            ))
                            .item(menu_item(&view, "New SQL Editor", move |v, w, cx| {
                                let _ = v.new_editor(Some(id), w, cx);
                            }))
                            .item(menu_item(&view, "ER Diagram", move |v, w, cx| {
                                v.open_diagram(id, w, cx, Box::new(|_, _, _, _| {}))
                            }))
                            .item(menu_item(&view, "Table Row Counts", move |v, w, cx| {
                                v.row_counts(id, w, cx, Box::new(|_, _, _, _| {}))
                            }))
                            .item(menu_item(&view, "Refresh", move |v, w, cx| {
                                v.refresh(id, w, cx, Box::new(|_, _, _, _| {}))
                            }))
                            .separator()
                            .item(menu_item(&view, "Edit Connection…", move |v, w, cx| {
                                v.open_dialog(Some(id), w, cx)
                            }))
                            .item(menu_item(
                                &view,
                                "Delete Connection",
                                move |v, w, cx| v.confirm_delete(id, w, cx),
                            ))
                        }
                    })
                    .into_any_element(),
            );
            if !expanded {
                continue;
            }
            if c.status == ConnStatus::Connecting {
                items.push(
                    row(format!("loading-{id}").into(), 1, false)
                        .text_color(t.muted_foreground)
                        .child("Loading…")
                        .into_any_element(),
                );
            }
            let q = self.side_filter.read(cx).value().to_lowercase();
            for (name, kind) in c
                .tables
                .iter()
                .filter(|(n, _)| q.is_empty() || n.to_lowercase().contains(&q))
            {
                let cols = c.columns.iter().find(|(n, _)| n == name).map(|(_, r)| r);
                let selected = self
                    .selected
                    .as_ref()
                    .is_some_and(|(i, tb)| *i == id && tb.as_deref() == Some(name));
                let n = name.clone();
                items.push(
                    row(format!("tbl-{id}-{name}").into(), 1, selected)
                        .child(
                            Icon::new(if cols.is_some() {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_3(),
                        )
                        .child(
                            Icon::new(if kind == "VIEW" {
                                IconName::Eye
                            } else {
                                IconName::Table
                            })
                            .size_4()
                            .text_color(t.primary),
                        )
                        .child(div().truncate().child(name.clone()))
                        .on_click(cx.listener(move |v, ev: &ClickEvent, w, cx| {
                            v.selected = Some((id, Some(n.clone())));
                            if ev.click_count() > 1 {
                                v.open_table(id, n.clone(), w, cx, Box::new(|_, _, _, _| {}));
                                return;
                            }
                            let ix = v.conn_ix(id).unwrap_or(0);
                            if v.conns[ix].columns.iter().any(|(t, _)| *t == n) {
                                v.conns[ix].columns.retain(|(t, _)| *t != n);
                                cx.notify();
                            } else {
                                v.describe(id, n.clone(), w, cx, Box::new(|_, _, _, _| {}));
                            }
                        }))
                        .context_menu({
                            let (view, n) = (view.clone(), name.clone());
                            move |menu: PopupMenu, _, _| {
                                let gen_ = |label: &str, kind: &'static str| {
                                    let n = n.clone();
                                    menu_item(&view, label, move |v, w, cx| {
                                        v.generate_sql(
                                            id,
                                            n.clone(),
                                            kind,
                                            w,
                                            cx,
                                            Box::new(|_, _, _, _| {}),
                                        )
                                    })
                                };
                                let (n1, n2, n3) = (n.clone(), n.clone(), n.clone());
                                menu.item(menu_item(&view, "Open Data", move |v, w, cx| {
                                    v.open_table(id, n1.clone(), w, cx, Box::new(|_, _, _, _| {}))
                                }))
                                .item(menu_item(&view, "View DDL", move |v, w, cx| {
                                    v.open_table(id, n2.clone(), w, cx, Box::new(|_, _, _, _| {}));
                                    if let Some(t) = v.tabs.get_mut(v.active) {
                                        t.view = TableView::Ddl;
                                    }
                                }))
                                .separator()
                                .item(gen_("Generate SELECT", "select"))
                                .item(gen_("Generate INSERT", "insert"))
                                .item(gen_("Generate UPDATE", "update"))
                                .item(gen_("Generate DELETE", "delete"))
                                .item(gen_("Count Rows", "count"))
                                .item({
                                    let n = n.clone();
                                    menu_item(&view, "Import CSV…", move |_, w, cx| {
                                        let n = n.clone();
                                        let rx = cx.prompt_for_paths(PathPromptOptions {
                                            files: true,
                                            directories: false,
                                            multiple: false,
                                            prompt: Some("Import".into()),
                                        });
                                        cx.spawn_in(w, async move |this, cx| {
                                            if let Ok(Ok(Some(mut paths))) = rx.await
                                                && let Some(p) = paths.pop()
                                            {
                                                let _ = this.update_in(cx, |v, w, cx| {
                                                    v.import_csv(
                                                        id,
                                                        n.clone(),
                                                        p,
                                                        w,
                                                        cx,
                                                        Box::new(|_, _, _, _| {}),
                                                    )
                                                });
                                            }
                                        })
                                        .detach();
                                    })
                                })
                                .separator()
                                .item(menu_item(
                                    &view,
                                    "Copy Name",
                                    move |v, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            n3.clone(),
                                        ));
                                        v.status = format!("Copied {n3}");
                                    },
                                ))
                            }
                        })
                        .into_any_element(),
                );
                for col in cols.map(|r| r.rows.as_slice()).unwrap_or_default() {
                    let (icon, color) = match col.get(3).map(String::as_str) {
                        Some("PK") => (IconName::KeyRound, t.warning),
                        Some("FK") => (IconName::Link, BRAND.into()),
                        _ => (IconName::Columns2, t.muted_foreground),
                    };
                    let refs = col
                        .get(4)
                        .filter(|r| !r.is_empty())
                        .map(|r| format!("→ {r}"));
                    items.push(
                        row(format!("col-{id}-{name}-{}", col[0]).into(), 2, false)
                            .child(Icon::new(icon).size_3().text_color(color))
                            .child(div().truncate().child(col[0].clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(col.get(1).cloned().unwrap_or_default()),
                            )
                            .children(refs.map(|r| {
                                div()
                                    .text_xs()
                                    .text_color(Hsla::from(BRAND))
                                    .truncate()
                                    .child(r)
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        let list = if self.sidebar == Sidebar::History {
            self.history_list(cx)
        } else {
            items
        };
        let side_button =
            |id: &'static str, panel: Sidebar, icon: IconName, label: &'static str| {
                Button::new(id)
                    .xsmall()
                    .icon(icon)
                    .label(label)
                    .map(|b| {
                        if self.sidebar == panel {
                            b.primary()
                        } else {
                            b.ghost()
                        }
                    })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.set_sidebar(panel, String::new(), w, cx);
                    }))
            };
        v_flex()
            .size_full()
            .bg(t.sidebar)
            .child(
                h_flex()
                    .px_2()
                    .py_1p5()
                    .gap_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(side_button(
                        "side-nav",
                        Sidebar::Navigator,
                        IconName::Server,
                        "Navigator",
                    ))
                    .child(side_button(
                        "side-hist",
                        Sidebar::History,
                        IconName::Rows3,
                        "History",
                    )),
            )
            .child(
                div().px_2().pt_2().pb_1().child(
                    Input::new(&self.side_filter)
                        .xsmall()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size_3()),
                ),
            )
            .child(
                div()
                    .id("nav-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .children(list),
            )
    }

    pub(super) fn history_list(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        let q = self.side_filter.read(cx).value().to_lowercase();
        let now_s = history::now();
        self.history
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, e)| {
                q.is_empty()
                    || e.sql.to_lowercase().contains(&q)
                    || e.connection.to_lowercase().contains(&q)
            })
            .take(200)
            .map(|(i, e)| {
                let first = e
                    .sql
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with("--"))
                    .unwrap_or_default()
                    .to_string();
                v_flex()
                    .id(("hist", i))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .hover(|d| d.bg(t.list_hover))
                    .child(
                        div()
                            .text_xs()
                            .font_family(t.mono_font_family.clone())
                            .truncate()
                            .child(first),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(history::age(e.at, now_s))
                            .child(div().truncate().child(e.connection.clone()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_color(if e.ok { t.muted_foreground } else { t.danger })
                                    .truncate()
                                    .max_w(px(140.))
                                    .child(e.outcome.clone()),
                            ),
                    )
                    .tooltip({
                        let sql = e.sql.clone();
                        move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(sql.clone()).build(w, cx)
                        }
                    })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        let Some(e) = v.history.get(i).cloned() else {
                            return;
                        };
                        let conn = v.conn_by_name(&e.connection).ok();
                        match v.new_editor(conn, w, cx) {
                            Ok(tab) => v.tabs[tab]
                                .editor
                                .update(cx, |ed, cx| ed.set_value(e.sql.clone(), w, cx)),
                            Err(err) => v.status = err,
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }
}
