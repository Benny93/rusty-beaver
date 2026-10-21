//! ER diagram tab: loading the schema and drawing boxes and foreign-key links.

use super::*;

impl AppView {
    /// Open (or refresh) a connection's ER diagram tab.
    pub(super) fn open_diagram(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: Done,
    ) {
        let tab = self.open_tab(id, TabKind::Diagram, String::new(), window, cx);
        self.tabs[tab].stale = false; // a restored tab: loaded now, not again when selected
        (self.tabs[tab].running, self.tabs[tab].started) = (true, Some(Instant::now()));
        let key = self.tabs[tab].grid.entity_id();
        self.with_session::<(Vec<crate::er::Table>, Vec<db::ForeignKey>)>(
            id,
            |s| s.schema(80),
            window,
            cx,
            Box::new(move |v, r, w, cx| {
                let tab = v.tab_ix(key).unwrap_or(usize::MAX); // closed: no tab gets it
                let out = r.map(|(tables, links)| {
                    let j = json!({
                        "tables": tables.iter().map(|t| json!({ "name": t.name, "columns": t.columns.iter().map(|c| json!({ "name": c.0, "type": c.1, "key": c.2 })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                        "relations": links,
                    });
                    if let Some(t) = v.tabs.get_mut(tab) {
                        v.status = format!("ER diagram: {} tables, {} relations", tables.len(), links.len());
                        t.diagram = Some(crate::er::Diagram::new(tables, links));
                    }
                    j
                });
                if let Some(t) = v.tabs.get_mut(tab) {
                    t.running = false;
                    if let Err(e) = &out {
                        t.message = Some(Err(e.clone()));
                        // a refresh keeps the old diagram on screen: say why it is not new
                        v.status = format!("ER diagram: {}", e.lines().next().unwrap_or_default());
                    }
                }
                cx.notify();
                done(v, out, w, cx)
            }),
        );
    }

    pub(super) fn diagram_view(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        use crate::er::{BOX_W, HEADER_H, ROW_H};
        let t = cx.theme().clone();
        let tab = &self.tabs[ix];
        let Some(d) = &tab.diagram else {
            let msg = match &tab.message {
                Some(Err(e)) => e.clone(),
                _ => "Reading the schema…".into(),
            };
            return div()
                .p_4()
                .text_color(t.muted_foreground)
                .child(msg)
                .into_any_element();
        };
        let (w, h) = d.extent();
        let lines = d.link_points();
        let line: Hsla = BRAND.into();
        let conn = tab.conn;
        let boxes: Vec<AnyElement> = d
            .tables
            .iter()
            .map(|tb| {
                let (x, y) = d.pos.get(&tb.name).copied().unwrap_or_default();
                let (n1, n2) = (tb.name.clone(), tb.name.clone());
                v_flex()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(BOX_W))
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.background)
                    .shadow_md()
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("er-{}", tb.name)))
                            .h(px(HEADER_H))
                            .px_2()
                            .gap_1()
                            .rounded_t_md()
                            .bg(t.primary.opacity(0.2))
                            .cursor_move()
                            .child(Icon::new(IconName::Table).size_4())
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .truncate()
                                    .child(tb.name.clone()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, ev: &MouseDownEvent, _, _| {
                                    v.tabs[ix].drag = Some((n1.clone(), ev.position, (x, y)));
                                }),
                            )
                            .on_click(cx.listener(move |v, ev: &ClickEvent, w, cx| {
                                if ev.click_count() > 1 {
                                    v.open_table(
                                        conn,
                                        n2.clone(),
                                        w,
                                        cx,
                                        Box::new(|_, _, _, _| {}),
                                    );
                                }
                            })),
                    )
                    .children(tb.columns.iter().map(|(name, ty, key)| {
                        h_flex()
                            .h(px(ROW_H))
                            .px_2()
                            .gap_1()
                            .text_xs()
                            .child(match key.as_str() {
                                "PK" => Icon::new(IconName::KeyRound)
                                    .size_3()
                                    .text_color(t.warning)
                                    .into_any_element(),
                                "FK" => Icon::new(IconName::Link)
                                    .size_3()
                                    .text_color(Hsla::from(BRAND))
                                    .into_any_element(),
                                _ => div().size_3().into_any_element(),
                            })
                            .child(div().truncate().child(name.clone()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .max_w(px(90.))
                                    .child(ty.clone()),
                            )
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id(("er", ix))
            .size_full()
            .overflow_scrollbar()
            .on_mouse_move(cx.listener(move |v, ev: &MouseMoveEvent, _, cx| {
                let Some(t) = v.tabs.get_mut(ix) else { return };
                let Some((name, start, (x0, y0))) = t.drag.clone() else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    t.drag = None;
                    return;
                }
                let (dx, dy) = (
                    (ev.position.x - start.x).as_f32(),
                    (ev.position.y - start.y).as_f32(),
                );
                if let Some(d) = t.diagram.as_mut() {
                    d.pos.insert(name, ((x0 + dx).max(0.), (y0 + dy).max(0.)));
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |v, _, _, _| {
                    if let Some(t) = v.tabs.get_mut(ix) {
                        t.drag = None;
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let o = bounds.origin;
                                for &((ax, ay), (bx, by)) in &lines {
                                    let a = point(o.x + px(ax), o.y + px(ay));
                                    let b = point(o.x + px(bx), o.y + px(by));
                                    let mid = point((a.x + b.x) / 2., (a.y + b.y) / 2.);
                                    let mut p = PathBuilder::stroke(px(1.5));
                                    p.move_to(a);
                                    p.curve_to(mid, point(mid.x, a.y));
                                    p.curve_to(b, point(mid.x, b.y));
                                    if let Ok(path) = p.build() {
                                        window.paint_path(path, line);
                                    }
                                    // the referenced end gets a dot
                                    let mut dot = PathBuilder::fill();
                                    let r = px(3.5);
                                    dot.move_to(point(b.x - r, b.y));
                                    dot.line_to(point(b.x, b.y - r));
                                    dot.line_to(point(b.x + r, b.y));
                                    dot.line_to(point(b.x, b.y + r));
                                    dot.close();
                                    if let Ok(path) = dot.build() {
                                        window.paint_path(path, line);
                                    }
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .children(boxes),
            )
            .into_any_element()
    }
}
