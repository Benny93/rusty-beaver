//! Result grid: every cell is text (see `db::QueryResult`). Sorting only permutes the
//! display order; pending edits (`db::Changes`) are keyed by the row's original index and
//! drawn on top of the loaded values until they are saved or reverted.

use crate::db::{Changes, QueryResult};
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cmp::Ordering;

#[derive(Default)]
pub struct Grid {
    pub r: QueryResult,
    /// Display position -> row index in `r.rows`.
    pub order: Vec<usize>,
    pub changes: Changes,
    /// Quick search: only rows with a cell containing this (lower-case) text are shown.
    pub search: String,
    /// The column sort in effect, so rows a search brings back take their sorted place.
    sort: Option<(usize, ColumnSort)>,
}

impl Grid {
    pub fn set(&mut self, r: QueryResult) {
        self.order = (0..r.rows.len()).collect();
        self.changes = Changes::default();
        self.sort = None; // new rows come unsorted
        self.r = r;
        if !self.search.is_empty() {
            let s = self.search.clone();
            self.set_search(&s);
        }
    }

    /// Show only rows containing `text` (case-insensitive); empty shows all. Keeps the sort.
    pub fn set_search(&mut self, text: &str) {
        self.search = text.trim().to_lowercase();
        // Shown rows keep their place; rows hidden by an earlier search follow in their
        // original order. A position table: 10,000 rows per keystroke stay instant.
        let n = self.r.rows.len();
        let mut pos = vec![usize::MAX; n];
        for (p, &id) in self.order.iter().enumerate() {
            if let Some(slot) = pos.get_mut(id) {
                *slot = p;
            }
        }
        let mut all: Vec<usize> = (0..n).collect();
        all.sort_by_key(|&id| (pos[id], id));
        self.order = all.into_iter().filter(|&id| self.matches(id)).collect();
        if let Some((col, sort)) = self.sort {
            self.sort_order(col, sort);
        }
    }

    fn matches(&self, id: usize) -> bool {
        self.search.is_empty()
            || (0..self.r.columns.len())
                .any(|c| self.value(id, c).to_lowercase().contains(&self.search))
    }

    /// Row index in `r.rows` of a display row.
    pub fn row_id(&self, display_row: usize) -> Option<usize> {
        self.order.get(display_row).copied()
    }

    /// The value shown: a pending edit, else the loaded one ("NULL" for NULL).
    pub fn value(&self, id: usize, col: usize) -> String {
        match self.changes.cells.get(&(id, col)) {
            Some(Some(v)) => v.clone(),
            Some(None) => "NULL".into(),
            None => self
                .r
                .rows
                .get(id)
                .and_then(|r| r.get(col))
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn sort_order(&mut self, col: usize, sort: ColumnSort) {
        let keys: Vec<String> = (0..self.r.rows.len())
            .map(|id| self.value(id, col))
            .collect();
        match sort {
            ColumnSort::Default => self.order.sort_unstable(),
            ColumnSort::Ascending => self.order.sort_by(|&a, &b| compare(&keys[a], &keys[b])),
            ColumnSort::Descending => self.order.sort_by(|&a, &b| compare(&keys[b], &keys[a])),
        }
    }

    /// Rows as shown (sorted, edits applied, deleted rows left out): for copy / export.
    pub fn shown(&self) -> QueryResult {
        let rows = self
            .order
            .iter()
            .filter(|id| !self.changes.deleted.contains(id))
            .map(|&id| {
                (0..self.r.columns.len())
                    .map(|c| self.value(id, c))
                    .collect()
            })
            .collect();
        QueryResult {
            columns: self.r.columns.clone(),
            rows,
            ..self.r.clone()
        }
    }

    /// Append an empty row (all NULL) to be inserted on save; returns its display row.
    pub fn add_row(&mut self) -> usize {
        let id = self.r.rows.len();
        self.r.rows.push(vec!["NULL".into(); self.r.columns.len()]);
        self.order.push(id);
        self.changes.inserted.insert(id);
        self.order.len() - 1
    }

    /// Drop pending edits and rows added in the grid.
    pub fn revert(&mut self) {
        let keep = self.r.rows.len() - self.changes.inserted.len();
        self.r.rows.truncate(keep);
        self.order.retain(|&id| id < keep);
        self.changes = Changes::default();
    }
}

/// Columns a bar chart can use: (label column, numeric value columns). A column is numeric
/// when every non-NULL value parses as a number; the label is the first other column.
pub fn chart_columns(r: &QueryResult) -> (Option<usize>, Vec<usize>) {
    let numeric: Vec<usize> = (0..r.columns.len())
        .filter(|&c| {
            let mut vals = r
                .rows
                .iter()
                .map(|row| row[c].as_str())
                .filter(|v| *v != "NULL")
                .peekable();
            vals.peek().is_some() && vals.all(|v| num(v).is_some())
        })
        .collect();
    let label = (0..r.columns.len()).find(|c| !numeric.contains(c));
    // Only numbers: the first one labels the bars, the rest are values.
    match (label, numeric.as_slice()) {
        (None, [first, rest @ ..]) if !rest.is_empty() => (Some(*first), rest.to_vec()),
        _ => (label, numeric),
    }
}

/// (label, value) bars of the rows as shown, at most `max`; repeated labels get a "#n" suffix
/// so every row stays its own bar.
pub fn chart_data(g: &Grid, label: Option<usize>, value: usize, max: usize) -> Vec<(String, f64)> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    g.order
        .iter()
        .filter(|id| !g.changes.deleted.contains(id))
        .take(max)
        .enumerate()
        .map(|(i, &id)| {
            let mut l = label
                .map(|c| g.value(id, c))
                .unwrap_or_else(|| (i + 1).to_string());
            let n = seen.entry(l.clone()).or_default();
            *n += 1;
            if *n > 1 {
                l = format!("{l} #{n}");
            }
            (l, num(&g.value(id, value)).unwrap_or(0.))
        })
        .collect()
}

/// Statistics of a column over the shown rows: (label, value) pairs for the value panel.
pub fn column_stats(g: &Grid, col: usize) -> Vec<(&'static str, String)> {
    let vals: Vec<String> = g
        .order
        .iter()
        .filter(|id| !g.changes.deleted.contains(id))
        .map(|&id| g.value(id, col))
        .collect();
    let present: Vec<&String> = vals.iter().filter(|v| *v != "NULL").collect();
    let distinct = present
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    let mut out = vec![
        ("rows", vals.len().to_string()),
        ("NULL", (vals.len() - present.len()).to_string()),
        ("distinct", distinct.to_string()),
    ];
    let nums: Vec<f64> = present.iter().filter_map(|v| num(v)).collect();
    let fmt = |x: f64| {
        if x.fract() == 0. && x.abs() < 1e15 {
            format!("{}", x as i64)
        } else if x.abs() >= 1e15 || x.abs() < 1e-4 {
            format!("{x}") // with 4 decimals 0.00004 would show as "0."
        } else {
            let s = format!("{x:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    };
    if !nums.is_empty() && nums.len() == present.len() {
        let sum: f64 = nums.iter().sum();
        out.push((
            "min",
            fmt(nums.iter().copied().fold(f64::INFINITY, f64::min)),
        ));
        out.push((
            "max",
            fmt(nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        ));
        out.push(("sum", fmt(sum)));
        out.push(("avg", fmt(sum / nums.len() as f64)));
    } else if !present.is_empty() {
        let short = |s: &&String| {
            if s.chars().count() > 40 {
                format!("{}…", s.chars().take(39).collect::<String>())
            } else {
                s.to_string()
            }
        };
        out.push(("min", present.iter().min().map(short).unwrap_or_default()));
        out.push(("max", present.iter().max().map(short).unwrap_or_default()));
    }
    out
}

/// A value as a number; text like "NaN" or "inf" is no number here.
pub fn num(v: &str) -> Option<f64> {
    v.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// NULL first, then numbers by value, then everything else by text: a total order, so
/// mixed columns sort consistently.
fn compare(a: &str, b: &str) -> Ordering {
    let rank = |v: &str| match (v == "NULL", num(v)) {
        (true, _) => 0,
        (_, Some(_)) => 1,
        _ => 2,
    };
    rank(a).cmp(&rank(b)).then_with(|| match (num(a), num(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        _ => a
            .to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b)),
    })
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.r.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        Column::new(
            SharedString::from(format!("c{ix}")),
            self.r.columns[ix].clone(),
        )
        .width(px(160.))
        .sortable()
    }

    fn perform_sort(
        &mut self,
        col: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.sort = Some((col, sort));
        self.sort_order(col, sort);
    }

    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = cx.theme();
        let Some(id) = self.row_id(row) else {
            return div();
        };
        let v = self.value(id, col);
        let edited = self.changes.cells.contains_key(&(id, col));
        let deleted = self.changes.deleted.contains(&id);
        let inserted = self.changes.inserted.contains(&id);
        div()
            .size_full()
            .truncate()
            .when(v == "NULL", |d| d.text_color(t.muted_foreground).italic())
            .when(inserted, |d| d.bg(t.success.opacity(0.15)))
            .when(edited, |d| d.bg(t.warning.opacity(0.25)))
            .when(deleted, |d| {
                d.bg(t.danger.opacity(0.18))
                    .line_through()
                    .text_color(t.danger)
            })
            .child(v)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob exports its own `test` macro.
    use super::{Grid, chart_columns, chart_data, column_stats, compare};
    use crate::db::QueryResult;

    #[test]
    fn stats_for_numbers_and_text() {
        let mut g = Grid::default();
        g.set(QueryResult {
            columns: vec!["n".into(), "s".into()],
            rows: vec![
                vec!["1".into(), "b".into()],
                vec!["NULL".into(), "a".into()],
                vec!["2.5".into(), "b".into()],
            ],
            ..Default::default()
        });
        let n = column_stats(&g, 0);
        assert_eq!(
            n,
            [
                ("rows", "3".to_string()),
                ("NULL", "1".into()),
                ("distinct", "2".into()),
                ("min", "1".into()),
                ("max", "2.5".into()),
                ("sum", "3.5".into()),
                ("avg", "1.75".into())
            ]
        );
        let s = column_stats(&g, 1);
        assert_eq!(
            s[2..],
            [
                ("distinct", "2".to_string()),
                ("min", "a".into()),
                ("max", "b".into())
            ]
        );
    }

    #[test]
    fn rows_a_search_brings_back_take_their_sorted_place() {
        let mut g = Grid::default();
        g.set(QueryResult {
            columns: vec!["name".into()],
            rows: vec![vec!["b".into()], vec!["c".into()], vec!["a".into()]],
            ..Default::default()
        });
        (g.sort, g.order) = (Some((0, super::ColumnSort::Ascending)), vec![2, 0, 1]);
        g.set_search("c");
        assert_eq!(g.order, [1]);
        g.set_search("");
        assert_eq!(g.order, [2, 0, 1]); // a, b, c: still sorted
    }

    #[test]
    fn search_narrows_and_restores_rows() {
        let mut g = Grid::default();
        g.set(QueryResult {
            columns: vec!["name".into()],
            rows: vec![
                vec!["Marco".into()],
                vec!["Eri".into()],
                vec!["Tarma".into()],
            ],
            ..Default::default()
        });
        g.order = vec![2, 0, 1]; // sorted
        g.set_search("AR");
        assert_eq!(g.order, [2, 0]); // Tarma, Marco: sort kept
        g.set_search("");
        assert_eq!(g.order.len(), 3);
        assert!(g.order.contains(&1));
    }

    #[test]
    fn chart_picks_label_and_numeric_columns() {
        let mut g = Grid::default();
        let row = |a: &str, b: &str, c: &str| vec![a.to_string(), b.to_string(), c.to_string()];
        g.set(QueryResult {
            columns: vec!["name".into(), "logs".into(), "dams".into()],
            rows: vec![
                row("Marco", "10", "NULL"),
                row("Eri", "2.5", "3"),
                row("Marco", "1", "1"),
            ],
            ..Default::default()
        });
        assert_eq!(chart_columns(&g.r), (Some(0), vec![1, 2]));
        assert_eq!(
            chart_data(&g, Some(0), 1, 10),
            [
                ("Marco".to_string(), 10.),
                ("Eri".into(), 2.5),
                ("Marco #2".into(), 1.)
            ]
        );
        g.set(QueryResult {
            columns: vec!["year".into(), "n".into()],
            rows: vec![vec!["2020".into(), "4".into()]],
            ..Default::default()
        });
        assert_eq!(chart_columns(&g.r), (Some(0), vec![1]));
    }

    #[test]
    fn sorting_and_edits_keep_rows_straight() {
        let mut g = Grid::default();
        g.set(QueryResult {
            columns: vec!["n".into()],
            rows: vec![vec!["10".into()], vec!["NULL".into()], vec!["9".into()]],
            ..Default::default()
        });
        let keys: Vec<String> = (0..3).map(|id| g.value(id, 0)).collect();
        g.order.sort_by(|&a, &b| compare(&keys[a], &keys[b]));
        assert_eq!(g.order, [1, 2, 0]); // NULL, 9, 10: numeric, not text order
        let mut mixed = ["1a", "Nan", "10", "2", "NULL"];
        mixed.sort_by(|a, b| compare(a, b));
        assert_eq!(mixed, ["NULL", "2", "10", "1a", "Nan"]); // numbers, then text
        g.changes.cells.insert((0, 0), Some("1".into()));
        assert_eq!(g.value(g.row_id(2).unwrap(), 0), "1");
        let d = g.add_row();
        assert_eq!(g.row_id(d), Some(3));
        g.changes.deleted.insert(2);
        assert_eq!(
            g.shown().rows,
            [
                vec!["NULL".to_string()],
                vec!["1".into()],
                vec!["NULL".into()]
            ]
        );
        g.revert();
        assert_eq!(
            (g.r.rows.len(), g.order.len(), g.changes.is_empty()),
            (3, 3, true)
        );
    }
}
