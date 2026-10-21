//! Toolbar and the tab area: SQL editor, result bars and the result / table views.

use super::execute::export_menu;
use super::*;
use gpui_kit::component::menu::DropdownMenu as _;

impl AppView {
    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let sel_conn = self.selected.as_ref().map(|s| s.0);
        let connected = sel_conn
            .and_then(|id| self.conn_ix(id))
            .is_some_and(|i| self.conns[i].status == ConnStatus::Connected);
        let sel_table = self.selected.as_ref().and_then(|s| s.1.clone());
        // Transaction controls follow the active tab's connection, else the selected one.
        let tx_conn = self
            .tabs
            .get(self.active)
            .map(|t| t.conn)
            .or(sel_conn)
            .and_then(|id| self.conn_ix(id))
            .map(|i| {
                (
                    self.conns[i].id,
                    self.conns[i].cfg.manual_commit,
                    self.conns[i].in_tx,
                    self.conns[i].tx_statements,
                )
            });
        h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(t.border)
            .bg(t.title_bar)
            .child(img(Arc::new(Image::from_bytes(ImageFormat::Png, ICON_PNG.to_vec()))).size(px(22.)).mr_1())
            .child(div().font_weight(FontWeight::BOLD).mr_3().child("Rusty Beaver"))
            .child(Button::new("tb-new-conn").ghost().small().icon(IconName::Plus).label("New Connection").on_click(cx.listener(|v, _, w, cx| v.open_dialog(None, w, cx))))
            .child(Button::new("tb-sql").ghost().small().icon(IconName::SquareTerminal).label("SQL Editor").on_click(cx.listener(|v, _, w, cx| {
                if let Err(e) = v.new_editor(None, w, cx) {
                    v.status = e;
                }
            })))
            .child(
                Button::new("tb-connect")
                    .ghost()
                    .small()
                    .disabled(sel_conn.is_none())
                    .icon(if connected { IconName::Unplug } else { IconName::Plug })
                    .label(if connected { "Disconnect" } else { "Connect" })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        let Some(id) = sel_conn else { return };
                        if connected { v.disconnect(id, cx) } else { v.connect(id, w, cx, Box::new(|_, _, _, _| {})) }
                    })),
            )
            .child(Button::new("tb-open").ghost().small().disabled(sel_table.is_none()).icon(IconName::Table).tooltip("Open Table").on_click(cx.listener(move |v, _, w, cx| {
                if let (Some(id), Some(t)) = (sel_conn, sel_table.clone()) {
                    v.open_table(id, t, w, cx, Box::new(|_, _, _, _| {}));
                }
            })))
            .child(Button::new("tb-er").ghost().small().disabled(sel_conn.is_none()).icon(IconName::Link).tooltip("ER Diagram").on_click(cx.listener(move |v, _, w, cx| {
                if let Some(id) = sel_conn {
                    v.open_diagram(id, w, cx, Box::new(|_, _, _, _| {}));
                }
            })))
            .child(Button::new("tb-run").ghost().small().disabled(self.tabs.is_empty()).icon(IconName::Play).tooltip("Execute the statement at the cursor (Cmd/Ctrl+Enter); the whole script: Cmd/Ctrl+Shift+Enter").on_click(cx.listener(|v, _, w, cx| v.run(v.active, Source::Statement, 0, w, cx, Box::new(|_, _, _, _| {})))))
            .child({
                let view = cx.entity().downgrade();
                Button::new("tb-csv")
                    .ghost()
                    .small()
                    .disabled(self.tabs.is_empty())
                    .icon(IconName::Download)
                    .tooltip("Export the result: copy or save as CSV, TSV, JSON, Markdown, SQL or text")
                    .dropdown_menu(move |m, _, _| export_menu(m, &view, true, true))
            })
            .child(
                Button::new("tb-delete")
                    .ghost()
                    .small()
                    .disabled(sel_conn.is_none())
                    .icon(IconName::CircleX)
                    .tooltip("Delete the selected connection")
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if let Some(id) = sel_conn {
                            v.confirm_delete(id, w, cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .children(tx_conn.map(|(id, manual, in_tx, n)| {
                h_flex()
                    .gap_1()
                    .mr_2()
                    .child(
                        Button::new("tb-autocommit")
                            .small()
                            .map(|b| if manual { b.warning() } else { b.ghost() })
                            .label(if manual { "Manual commit" } else { "Auto-commit" })
                            .tooltip("Toggle auto-commit for this connection")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let Err(e) = v.set_manual_commit(id, !manual, cx) {
                                    v.status = e;
                                }
                            })),
                    )
                    .when(in_tx, |d| {
                        d.child(Button::new("tb-commit").small().primary().label(format!("Commit ({n})")).on_click(cx.listener(move |v, _, w, cx| v.end_tx(id, true, w, cx, Box::new(|_, _, _, _| {})))))
                            .child(Button::new("tb-rollback").small().outline().label("Rollback").on_click(cx.listener(move |v, _, w, cx| v.end_tx(id, false, w, cx, Box::new(|_, _, _, _| {})))))
                    })
            }))
            .child(Button::new("tb-help").ghost().small().icon(IconName::Info).tooltip("Keyboard shortcuts (F1)").on_click(cx.listener(|v, _, _, cx| {
                v.shortcuts = !v.shortcuts;
                cx.notify();
            })))
            .child(Button::new("tb-theme").ghost().small().icon(if self.dark { IconName::Sun } else { IconName::Moon }).tooltip("Toggle theme").on_click(cx.listener(|v, _, w, cx| v.set_theme(!v.dark, w, cx))))
    }

    pub(super) fn editor_area(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let t = cx.theme().clone();
        if self.tabs.is_empty() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(t.muted_foreground)
                .child(img(self.splash_image.clone()).size(px(128.)))
                .child(div().text_lg().child("Pick a connection, then open a table or a SQL editor"))
                .child(div().text_sm().child("Double-click a table · Cmd/Ctrl+T new SQL editor · Cmd/Ctrl+Enter run statement"))
                .into_any_element();
        }
        // Bring a newly opened or selected tab into view (once, so the user can scroll away).
        if self.revealed_tab.get() != Some(self.active) {
            self.tab_scroll.scroll_to_item(self.active);
            self.revealed_tab.set(Some(self.active));
        }
        let tabbar = TabBar::new("tabs")
            .track_scroll(&self.tab_scroll)
            .menu(true)
            .max_width(px(240.))
            .selected_index(self.active)
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                Tab::new()
                    .label(tab.title.clone())
                    .prefix(
                        div().pl_2().child(
                            Icon::new(match tab.kind {
                                TabKind::Sql => IconName::FileCode,
                                TabKind::Diagram => IconName::Link,
                                _ => IconName::Table,
                            })
                            .size_4(),
                        ),
                    )
                    .suffix(
                        Button::new(("close-tab", i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                let _ = v.close_tab(i, cx);
                            })),
                    )
            }))
            .on_click(cx.listener(|v, i: &usize, w, cx| v.select_tab_ix(*i, w, cx)));
        let tab = &self.tabs[self.active];
        let ix = self.active;
        let is_table = matches!(tab.kind, TabKind::Table(_));
        let message = match &tab.message {
            _ if tab.running => div().text_color(t.warning).child(format!("Executing… {:.1} s", tab.started.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.))),
            Some(Ok(m)) => div().truncate().text_color(t.muted_foreground).child(m.clone()),
            // One line, cut off with "…": the copy button and the status bar have it all.
            Some(Err(e)) => h_flex()
                .gap_1()
                .min_w_0()
                .child(self.copy_button("copy-sql-error", e.clone(), cx))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.danger)
                        .child(format!("SQL Error: {}", e.lines().next().unwrap_or_default())),
                ),
            None => div().truncate().text_color(t.muted_foreground).child("Cmd/Ctrl+Enter runs the statement at the cursor (or the selection) · Cmd/Ctrl+Shift+Enter runs the script"),
        };
        let g = tab.grid.read(cx).delegate();
        let pending = g.changes.count();
        let loaded = g.r.rows.len() - g.changes.inserted.len();
        let more = is_table && loaded >= tab.limit && tab.total.is_none_or(|n| n as usize > loaded);
        let editing = tab.editing.map(|(id, col)| {
            (
                g.r.columns.get(col).cloned().unwrap_or_default(),
                g.order.iter().position(|r| *r == id).unwrap_or(0),
            )
        });
        let view_button =
            |id: &'static str, view: TableView, icon: IconName, label: &'static str| {
                pick(Button::new(id).small(), tab.view == view)
                    .icon(icon)
                    .label(label)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.tabs[ix].view = view;
                        cx.notify();
                    }))
            };
        let bar = h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(t.border)
            .text_sm()
            .when(is_table, |d| {
                d.child(view_button(
                    "view-data",
                    TableView::Data,
                    IconName::Sheet,
                    "Data",
                ))
                .child(view_button(
                    "view-cols",
                    TableView::Columns,
                    IconName::TableProperties,
                    "Columns",
                ))
                .child(view_button(
                    "view-ddl",
                    TableView::Ddl,
                    IconName::FileCode,
                    "DDL",
                ))
                .child(div().w(px(1.)).h(px(18.)).bg(t.border))
            })
            .when(is_table && tab.view == TableView::Data, |d| {
                d.child(
                    div().flex_1().max_w(px(420.)).child(
                        Input::new(&tab.filter)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size_3()),
                    ),
                )
                .child(
                    Button::new("reload")
                        .small()
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .tooltip("Reload")
                        .loading(tab.running)
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.reload_table(ix, w, cx, Box::new(|_, _, _, _| {}))
                        })),
                )
                .child(
                    Button::new("add-row")
                        .small()
                        .ghost()
                        .icon(IconName::Plus)
                        .tooltip("Add row")
                        .on_click(cx.listener(|v, _, w, cx| v.report(GridOp::AddRow, w, cx))),
                )
                .child(
                    Button::new("del-row")
                        .small()
                        .ghost()
                        .icon(IconName::CircleX)
                        .tooltip("Delete row (Cmd/Ctrl+Backspace)")
                        .on_click(cx.listener(|v, _, w, cx| v.report(GridOp::DeleteRow, w, cx))),
                )
                .when(pending > 0, |d| {
                    d.child(
                        Button::new("save")
                            .small()
                            .primary()
                            .icon(IconName::Save)
                            .label(format!("Save ({pending})"))
                            .loading(tab.running)
                            .on_click(cx.listener(|v, _, w, cx| v.report(GridOp::Save, w, cx))),
                    )
                    .child(
                        Button::new("revert")
                            .small()
                            .outline()
                            .label("Revert")
                            .on_click(cx.listener(|v, _, w, cx| v.report(GridOp::Revert, w, cx))),
                    )
                })
            })
            .when(!is_table && tab.view == TableView::Data, |d| {
                d.child(
                    div().w(px(200.)).child(
                        Input::new(&tab.search)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size_3()),
                    ),
                )
            })
            .when(!is_table && !tab.running, |d| {
                d.child(
                    Button::new("run")
                        .small()
                        .primary()
                        .icon(IconName::Play)
                        .label("Execute")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.run(ix, Source::Statement, 0, w, cx, Box::new(|_, _, _, _| {}))
                        })),
                )
            })
            .when(!is_table, |d| {
                d.child(
                    Button::new("explain")
                        .small()
                        .ghost()
                        .icon(IconName::Zap)
                        .label("Explain")
                        .tooltip("Query plan of the statement at the cursor (Cmd/Ctrl+Shift+E)")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.explain(ix, None, w, cx, Box::new(|_, _, _, _| {}))
                        })),
                )
                .child(
                    Button::new("format")
                        .small()
                        .ghost()
                        .icon(IconName::Pencil)
                        .label("Format")
                        .tooltip("Format the SQL (Cmd/Ctrl+Shift+F)")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if let Err(e) = v.format(ix, w, cx) {
                                v.status = e;
                            }
                        })),
                )
            })
            .when(tab.running, |d| {
                d.child(
                    Button::new("cancel")
                        .small()
                        .danger()
                        .icon(IconName::CircleX)
                        .label("Cancel")
                        .tooltip("Cancel the query (Cmd/Ctrl+.)")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if let Err(e) = v.cancel(ix, cx) {
                                v.status = e;
                            }
                        })),
                )
            })
            .when(tab.view == TableView::Data, |d| {
                let export_view = view.clone();
                d.child(
                    Button::new("results-export")
                        .small()
                        .ghost()
                        .icon(IconName::Download)
                        .label("Export")
                        .tooltip("Copy or save the result as CSV, TSV, JSON, Markdown, SQL or text")
                        .dropdown_menu(move |m, _, _| export_menu(m, &export_view, true, true)),
                )
                .child(
                    Button::new("text-view")
                        .small()
                        .map(|b| {
                            if tab.text_view {
                                b.primary()
                            } else {
                                b.ghost()
                            }
                        })
                        .icon(IconName::Terminal)
                        .tooltip("Text: the result as an aligned plain-text table")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.tabs[ix].text_view = !v.tabs[ix].text_view;
                            cx.notify();
                        })),
                )
                .when(tab.text_view, |d| {
                    let text = self
                        .export_text("txt", cx)
                        .map(|(t, _)| t)
                        .unwrap_or_default();
                    d.child(self.copy_button("copy-text-view", text, cx))
                })
                .child(
                    Button::new("chart")
                        .small()
                        .map(|b| if tab.chart { b.primary() } else { b.ghost() })
                        .icon(IconName::ChartColumn)
                        .tooltip("Chart: the result as bars")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.tabs[ix].chart = !v.tabs[ix].chart;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("value-panel")
                        .small()
                        .map(|b| {
                            if tab.value_panel {
                                b.primary()
                            } else {
                                b.ghost()
                            }
                        })
                        .icon(IconName::Eye)
                        .tooltip("Value panel: the selected cell in full (JSON pretty-printed)")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.tabs[ix].value_panel = !v.tabs[ix].value_panel;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("record")
                        .small()
                        .map(|b| if tab.record { b.primary() } else { b.ghost() })
                        .icon(IconName::Rows3)
                        .tooltip("Record view: the selected row as a form")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.tabs[ix].record = !v.tabs[ix].record;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1().min_w_0().pr_2().child(message))
            .when(is_table && tab.view == TableView::Data, |d| {
                d.children(tab.total.map(|n| {
                    div()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child(format!("{loaded} of {n} rows"))
                }))
                .when(more, |d| {
                    d.child(
                        Button::new("more")
                            .small()
                            .outline()
                            .label(format!("Load {PAGE} more"))
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.load_more(ix, w, cx, Box::new(|_, _, _, _| {}))
                            })),
                    )
                })
            });
        let edit_bar = editing.map(|(col, row)| {
            h_flex()
                .key_context("CellEdit")
                .on_action(
                    cx.listener(move |v, _: &CancelEdit, w, cx| v.finish_edit(ix, None, w, cx)),
                )
                .gap_2()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(t.border)
                .bg(t.warning.opacity(0.12))
                .text_sm()
                .child(Icon::new(IconName::Pencil).size_4())
                .child(div().font_weight(FontWeight::BOLD).child(col))
                .child(
                    div()
                        .text_color(t.muted_foreground)
                        .child(format!("row {}", row + 1)),
                )
                .child(div().flex_1().child(Input::new(&tab.edit_input).small()))
                .child(
                    Button::new("edit-ok")
                        .small()
                        .primary()
                        .label("Apply")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            let value = v.tabs[ix].edit_input.read(cx).value().to_string();
                            v.finish_edit(ix, Some(value), w, cx)
                        })),
                )
                .child(
                    Button::new("edit-null")
                        .small()
                        .outline()
                        .label("Set NULL")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if let Some((id, col)) = v.tabs[ix].editing {
                                v.finish_edit(ix, None, w, cx);
                                v.set_cell(ix, id, col, None, cx);
                            }
                        })),
                )
                .child(
                    Button::new("edit-cancel")
                        .small()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(move |v, _, w, cx| v.finish_edit(ix, None, w, cx))),
                )
        });
        let body = match tab.view {
            TableView::Columns => DataTable::new(&tab.columns).stripe(true).into_any_element(),
            TableView::Ddl => Editor::new(&tab.ddl)
                .bordered(false)
                .readonly(true)
                .h(relative(1.))
                .into_any_element(),
            TableView::Data if tab.text_view => {
                let text = self
                    .export_text("txt", cx)
                    .map(|(t, _)| t)
                    .unwrap_or_default();
                if tab.text.read(cx).value() != text.as_str() {
                    tab.text.update(cx, |e, cx| e.set_value(text, window, cx));
                }
                Editor::new(&tab.text)
                    .bordered(false)
                    .readonly(true)
                    .h(relative(1.))
                    .into_any_element()
            }
            TableView::Data if tab.chart => self.chart_view(ix, cx),
            TableView::Data if tab.record => self.record_view(ix, cx),
            TableView::Data => div()
                .id("grid-wrap")
                .size_full()
                .child(DataTable::new(&tab.grid).stripe(true))
                .context_menu(move |menu: PopupMenu, w, cx| {
                    let (copy_view, save_view) = (view.clone(), view.clone());
                    let menu = menu
                        .menu("Copy Value", Box::new(CopyValue))
                        .menu("Copy Row", Box::new(CopyRow))
                        .menu("Copy as INSERT", Box::new(CopyInsert))
                        .menu("Copy Column as IN List", Box::new(CopyColumnInList))
                        .separator()
                        .submenu("Copy All As", w, cx, move |m, _, _| {
                            export_menu(m, &copy_view, true, false)
                        })
                        .submenu("Export to File", w, cx, move |m, _, _| {
                            export_menu(m, &save_view, false, true)
                        });
                    if is_table {
                        menu.separator()
                            .menu("Filter by This Value", Box::new(FilterByValue))
                            .menu("Exclude This Value", Box::new(FilterExcludeValue))
                            .separator()
                            .menu("Edit Cell", Box::new(EditCell))
                            .menu("Set NULL", Box::new(SetNull))
                            .menu("Add Row", Box::new(AddRow))
                            .menu("Duplicate Row", Box::new(DuplicateRowAction))
                            .menu("Open Referenced Row", Box::new(OpenReferencedRow))
                            .menu("Delete Row", Box::new(DeleteRow))
                            .separator()
                            .menu("Save Changes", Box::new(SaveChanges))
                            .menu("Revert Changes", Box::new(RevertChanges))
                    } else {
                        menu
                    }
                })
                .into_any_element(),
        };
        let switcher = (tab.results.len() > 1 && tab.view == TableView::Data).then(|| {
            h_flex()
                .gap_1()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(t.border)
                .text_xs()
                .children(tab.results.iter().enumerate().map(|(i, r)| {
                    let label = match r.affected {
                        Some(n) if r.columns.is_empty() => format!("{}: {n} updated", i + 1),
                        _ => format!("{}: {}", i + 1, plural(r.rows.len(), "row")),
                    };
                    Button::new(("result", i))
                        .xsmall()
                        .map(|b| {
                            if i == tab.result_ix {
                                b.primary()
                            } else {
                                b.ghost()
                            }
                        })
                        .label(label)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            let _ = v.show_result(ix, i, 50, cx);
                        }))
                }))
        });
        let body = if tab.value_panel && tab.view == TableView::Data {
            h_resizable(("value-split", ix))
                .child(resizable_panel().child(body))
                .child(
                    resizable_panel()
                        .size(px(320.))
                        .child(self.value_panel(ix, cx)),
                )
                .into_any_element()
        } else {
            body
        };
        let prod = self
            .conn_ix(tab.conn)
            .is_some_and(|i| self.conns[i].cfg.production);
        let results = v_flex()
            .when(prod, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .py(px(2.))
                        .bg(t.danger.opacity(0.85))
                        .text_color(gpui_kit::white())
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child("PRODUCTION")
                        .child(
                            div()
                                .font_weight(FontWeight::NORMAL)
                                .child("changes to data and schema ask for confirmation"),
                        ),
                )
            })
            .size_full()
            .child(bar)
            .children(switcher)
            .children(edit_bar)
            .child(div().flex_1().min_h_0().child(body));
        v_flex()
            .size_full()
            .child(tabbar)
            .child(if tab.kind == TabKind::Diagram {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.diagram_view(ix, cx))
                    .into_any_element()
            } else if is_table {
                div().flex_1().min_h_0().child(results).into_any_element()
            } else {
                v_resizable(("editor-split", ix))
                    .child(
                        resizable_panel().size(px(260.)).child(
                            div()
                                .size_full()
                                .child(Editor::new(&tab.editor).bordered(false).h(relative(1.))),
                        ),
                    )
                    .child(resizable_panel().child(results))
                    .into_any_element()
            })
            .into_any_element()
    }
}
