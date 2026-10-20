//! Alternative result views: bar chart, value panel and record form.

use super::*;

impl AppView {
    /// The result as a bar chart: a text column labels the bars, a numeric column sizes them.
    pub(super) fn chart_view(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::chart::BarChart;
        let t = cx.theme().clone();
        let tab = &self.tabs[ix];
        let g = tab.grid.read(cx).delegate();
        let (label, numeric) = crate::grid::chart_columns(&g.r);
        let Some(value) = tab
            .chart_col
            .filter(|c| numeric.contains(c))
            .or(numeric.first().copied())
        else {
            return div()
                .p_4()
                .text_color(t.muted_foreground)
                .child("Nothing to chart: the result has no numeric column")
                .into_any_element();
        };
        let data = crate::grid::chart_data(g, label, value, 60);
        let shown = data.len();
        let total = g.order.len();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .child(format!(
                        "{} by",
                        label
                            .map(|c| g.r.columns[c].clone())
                            .unwrap_or("row".into())
                    ))
                    .children(numeric.iter().map(|&c| {
                        Button::new(("chart-col", c))
                            .xsmall()
                            .map(|b| if c == value { b.primary() } else { b.ghost() })
                            .label(g.r.columns[c].clone())
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.tabs[ix].chart_col = Some(c);
                                cx.notify();
                            }))
                    }))
                    .when(total > shown, |d| {
                        d.child(format!("· first {shown} of {total} rows"))
                    }),
            )
            .child(
                div().flex_1().min_h_0().p_4().child(
                    BarChart::new(data)
                        .id(("bar-chart", ix))
                        .band(|d: &(String, f64)| d.0.clone())
                        .value(|d: &(String, f64)| d.1)
                        .label(|d: &(String, f64)| {
                            if d.1.fract() == 0. && d.1.abs() < 1e15 {
                                format!("{}", d.1 as i64)
                            } else {
                                format!("{:.2}", d.1)
                            }
                        })
                        .fill(|_: &(String, f64), _, _, _| Hsla::from(BRAND))
                        .corner_radii(Corners::all(px(3.))),
                ),
            )
            .into_any_element()
    }

    /// The selected cell's full value: JSON pretty-printed, everything else as is.
    pub(super) fn value_panel(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let tab = &self.tabs[ix];
        let g = tab.grid.read(cx).delegate();
        let cell = tab
            .cell
            .and_then(|(r, c)| Some((g.row_id(r)?, c, g.r.columns.get(c)?.clone())));
        let Some((id, c, col)) = cell else {
            return div()
                .p_3()
                .text_sm()
                .text_color(t.muted_foreground)
                .child("Select a cell")
                .into_any_element();
        };
        let raw = g.value(id, c);
        let pretty = serde_json::from_str::<Value>(&raw)
            .ok()
            .filter(|v| v.is_object() || v.is_array())
            .map(|v| serde_json::to_string_pretty(&v).unwrap());
        let is_json = pretty.is_some();
        let shown = pretty.unwrap_or_else(|| raw.clone());
        v_flex()
            .size_full()
            .border_l_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(t.border)
                    .text_sm()
                    .child(div().font_weight(FontWeight::BOLD).truncate().child(col))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(if is_json {
                                "JSON".to_string()
                            } else {
                                format!("{} chars", raw.chars().count())
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("value-copy")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip("Copy value")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(raw.clone()));
                                v.status = "Copied value".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("value-text")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .text_sm()
                    .font_family(t.mono_font_family.clone())
                    .when(shown == "NULL", |d| {
                        d.italic().text_color(t.muted_foreground)
                    })
                    .child(shown),
            )
            .child(
                // Column statistics over the shown rows (DBeaver's Calc panel).
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(t.border)
                    .text_xs()
                    .children(crate::grid::column_stats(g, c).into_iter().map(|(k, v)| {
                        h_flex()
                            .gap_1()
                            .child(div().text_color(t.muted_foreground).child(k))
                            .child(div().font_family(t.mono_font_family.clone()).child(v))
                    })),
            )
            .into_any_element()
    }

    /// The selected row of a tab as field / value pairs, with previous / next.
    pub(super) fn record_view(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let tab = &self.tabs[ix];
        let g = tab.grid.read(cx).delegate();
        let rows = g.order.len();
        if rows == 0 {
            return div()
                .p_4()
                .text_color(t.muted_foreground)
                .child("No rows")
                .into_any_element();
        }
        let row = tab.cell.map(|c| c.0).unwrap_or(0).min(rows - 1);
        let id = g.row_id(row).unwrap_or(0);
        let col_types: Vec<String> = {
            let cols = &tab.columns.read(cx).delegate().r;
            g.r.columns
                .iter()
                .map(|c| {
                    cols.rows
                        .iter()
                        .find(|r| r[0] == *c)
                        .map(|r| r[1].clone())
                        .unwrap_or_default()
                })
                .collect()
        };
        let step = |id: &'static str, icon: IconName, delta: isize| {
            Button::new(id)
                .small()
                .ghost()
                .icon(icon)
                .on_click(cx.listener(move |v, _, _, cx| {
                    let t = &mut v.tabs[ix];
                    let n = t.grid.read(cx).delegate().order.len();
                    let (r, c) = t.cell.unwrap_or((0, 0));
                    t.cell = Some(((r as isize + delta).clamp(0, n as isize - 1) as usize, c));
                    cx.notify();
                }))
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child(step("rec-prev", IconName::ChevronLeft, -1))
                    .child(format!("Row {} of {rows}", row + 1))
                    .child(step("rec-next", IconName::ChevronRight, 1)),
            )
            .child(
                div()
                    .id("record")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .children(g.r.columns.iter().enumerate().map(|(c, name)| {
                        let v = g.value(id, c);
                        let edited = g.changes.cells.contains_key(&(id, c));
                        h_flex()
                            .id(("rec", c))
                            .gap_3()
                            .py_1()
                            .border_b_1()
                            .border_color(t.border)
                            .child(
                                v_flex()
                                    .w(px(200.))
                                    .flex_shrink_0()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.muted_foreground)
                                            .child(col_types[c].clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_1()
                                    .rounded_sm()
                                    .when(v == "NULL", |d| {
                                        d.text_color(t.muted_foreground).italic()
                                    })
                                    .when(edited, |d| d.bg(t.warning.opacity(0.25)))
                                    .child(v),
                            )
                            .on_click(cx.listener(move |v, ev: &ClickEvent, w, cx| {
                                v.tabs[ix].cell = Some((row, c));
                                if ev.click_count() > 1 {
                                    v.report(GridOp::EditCell, w, cx);
                                }
                                cx.notify();
                            }))
                    })),
            )
            .into_any_element()
    }
}
