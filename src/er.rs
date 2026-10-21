//! ER diagram model and layout: tables as boxes, foreign keys as links. Referenced (parent)
//! tables go left, the tables pointing at them in later columns; boxes can then be dragged.

use crate::db::ForeignKey;
use std::collections::HashMap;

pub const BOX_W: f32 = 230.;
pub const HEADER_H: f32 = 30.;
pub const ROW_H: f32 = 20.;
const GAP_X: f32 = 90.;
const GAP_Y: f32 = 40.;
/// Boxes stacked per layout column before a level wraps into another column.
const PER_COLUMN: usize = 5;

#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    /// (name, type, key: "PK" / "FK" / "")
    pub columns: Vec<(String, String, String)>,
}

impl Table {
    pub fn height(&self) -> f32 {
        HEADER_H + ROW_H * self.columns.len() as f32 + 6.
    }
}

#[derive(Clone, Debug, Default)]
pub struct Diagram {
    pub tables: Vec<Table>,
    pub links: Vec<ForeignKey>,
    /// Top-left corner of each table's box.
    pub pos: HashMap<String, (f32, f32)>,
}

impl Diagram {
    pub fn new(tables: Vec<Table>, links: Vec<ForeignKey>) -> Self {
        let mut d = Diagram {
            tables,
            links,
            pos: HashMap::new(),
        };
        d.layout();
        d
    }

    /// Level 0: tables that reference nothing; else one past the deepest table they reference.
    fn levels(&self) -> HashMap<String, usize> {
        let mut level: HashMap<String, usize> =
            self.tables.iter().map(|t| (t.name.clone(), 0)).collect();
        // (child, parent) of every link between two shown tables
        let edges: Vec<(&str, &str)> = self
            .links
            .iter()
            .filter(|l| {
                l.table != l.ref_table
                    && level.contains_key(&l.table)
                    && level.contains_key(&l.ref_table)
            })
            .map(|l| (l.table.as_str(), l.ref_table.as_str()))
            .collect();
        // A cycle (a→b→a) loses one edge, found by a depth-first walk: the rest has no cycle,
        // so a table that references the cycle still lands right of it.
        fn walk<'a>(
            n: &'a str,
            edges: &[(&'a str, &'a str)],
            state: &mut HashMap<&'a str, u8>, // 1 on the walk's path, 2 done
            back: &mut Vec<usize>,
        ) {
            state.insert(n, 1);
            for (i, &(c, p)) in edges.iter().enumerate() {
                if c == n {
                    match state.get(p) {
                        Some(1) => back.push(i),
                        None => walk(p, edges, state, back),
                        _ => {}
                    }
                }
            }
            state.insert(n, 2);
        }
        let (mut state, mut back) = (HashMap::new(), vec![]);
        for t in &self.tables {
            if !state.contains_key(t.name.as_str()) {
                walk(&t.name, &edges, &mut state, &mut back);
            }
        }
        // Longest path by relaxation over what is left (bounded by the table count anyway).
        for _ in 0..self.tables.len() {
            let mut changed = false;
            for (i, &(c, p)) in edges.iter().enumerate() {
                if back.contains(&i) {
                    continue;
                }
                let (parent, child) = (level[p], level[c]);
                if child < parent + 1 {
                    level.insert(c.to_string(), parent + 1);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        level
    }

    pub fn layout(&mut self) {
        let levels = self.levels();
        let max = levels.values().copied().max().unwrap_or(0);
        let mut x = 20.;
        for lvl in 0..=max {
            let mut in_level: Vec<&Table> = self
                .tables
                .iter()
                .filter(|t| levels.get(&t.name) == Some(&lvl))
                .collect();
            in_level.sort_by(|a, b| a.name.cmp(&b.name));
            for chunk in in_level.chunks(PER_COLUMN) {
                let mut y = 20.;
                for t in chunk {
                    self.pos.insert(t.name.clone(), (x, y));
                    y += t.height() + GAP_Y;
                }
                x += BOX_W + GAP_X;
            }
        }
    }

    /// Size of the area the boxes cover.
    pub fn extent(&self) -> (f32, f32) {
        self.tables.iter().fold((0f32, 0f32), |(w, h), t| {
            let (x, y) = self.pos.get(&t.name).copied().unwrap_or_default();
            (w.max(x + BOX_W + 40.), h.max(y + t.height() + 40.))
        })
    }

    /// Link endpoints in diagram coordinates: from the referencing column's row to the
    /// referenced column's row (or header), on the facing sides of the two boxes.
    pub fn link_points(&self) -> Vec<((f32, f32), (f32, f32))> {
        let find = |name: &str| self.tables.iter().find(|t| t.name == name);
        self.links
            .iter()
            .filter_map(|l| {
                let (from, to) = (find(&l.table)?, find(&l.ref_table)?);
                let (fx, fy) = *self.pos.get(&from.name)?;
                let (tx, ty) = *self.pos.get(&to.name)?;
                let row_y = |t: &Table, top: f32, col: &str| match t
                    .columns
                    .iter()
                    .position(|c| c.0 == col)
                {
                    Some(i) => top + HEADER_H + ROW_H * i as f32 + ROW_H / 2.,
                    None => top + HEADER_H / 2.,
                };
                let ref_col = if l.ref_column.is_empty() {
                    to.columns
                        .iter()
                        .find(|c| c.2 == "PK")
                        .map(|c| c.0.as_str())
                        .unwrap_or("")
                } else {
                    l.ref_column.as_str()
                };
                let a_y = row_y(from, fy, &l.column);
                let b_y = row_y(to, ty, ref_col);
                Some(if (tx - fx).abs() < BOX_W {
                    // one above the other (dragged, or a table referencing itself): both
                    // ends on the right side, not a line across the boxes
                    ((fx + BOX_W, a_y), (tx + BOX_W, b_y))
                } else if tx > fx {
                    ((fx + BOX_W, a_y), (tx, b_y))
                } else {
                    ((fx, a_y), (tx + BOX_W, b_y))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, cols: &[&str]) -> Table {
        Table {
            name: name.into(),
            columns: cols
                .iter()
                .map(|c| {
                    (
                        c.to_string(),
                        "INT".into(),
                        if *c == "id" {
                            "PK".into()
                        } else {
                            String::new()
                        },
                    )
                })
                .collect(),
        }
    }

    fn fk(table: &str, column: &str, ref_table: &str) -> ForeignKey {
        ForeignKey {
            table: table.into(),
            column: column.into(),
            ref_table: ref_table.into(),
            ref_column: String::new(),
        }
    }

    #[test]
    fn a_table_referencing_a_cycle_is_right_of_it() {
        let d = Diagram::new(
            vec![
                t("a", &["id", "b_id"]),
                t("b", &["id", "a_id"]),
                t("c", &["id", "b_id"]),
            ],
            vec![
                fk("a", "b_id", "b"),
                fk("b", "a_id", "a"),
                fk("c", "b_id", "b"),
            ],
        );
        let x = |n: &str| d.pos[n].0;
        assert!(x("c") > x("b"), "c references b");
    }

    #[test]
    fn parents_left_children_right() {
        let d = Diagram::new(
            vec![
                t("missions", &["id", "leader_id"]),
                t("beavers", &["id", "lodge_id"]),
                t("lodges", &["id"]),
                t("loose", &["id"]),
            ],
            vec![
                fk("missions", "leader_id", "beavers"),
                fk("beavers", "lodge_id", "lodges"),
            ],
        );
        let x = |n: &str| d.pos[n].0;
        assert!(x("lodges") < x("beavers") && x("beavers") < x("missions"));
        assert_eq!(x("loose"), x("lodges"));
        // boxes one above the other (dragged there): both ends on the same side, not across
        let mut stacked = d.clone();
        stacked.pos.insert("missions".into(), (20., 20.));
        stacked.pos.insert("beavers".into(), (20., 300.));
        let (a, b) = stacked.link_points()[0];
        assert_eq!(a.0, b.0, "{a:?} {b:?}");
        // missions.leader_id (row 1) -> beavers' PK row 0, from missions' left side to beavers' right side
        let pts = d.link_points();
        assert_eq!(
            pts[0].0,
            (x("missions"), d.pos["missions"].1 + HEADER_H + ROW_H * 1.5)
        );
        assert_eq!(
            pts[0].1,
            (
                x("beavers") + BOX_W,
                d.pos["beavers"].1 + HEADER_H + ROW_H * 0.5
            )
        );
    }

    #[test]
    fn cycles_and_self_links_terminate() {
        let d = Diagram::new(
            vec![
                t("a", &["id", "b"]),
                t("b", &["id", "a"]),
                t("c", &["id", "p"]),
            ],
            vec![fk("a", "b", "b"), fk("b", "a", "a"), fk("c", "p", "c")],
        );
        assert_eq!(d.pos.len(), 3);
        assert!(d.extent().0 > BOX_W);
    }
}
