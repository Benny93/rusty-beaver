//! SQL autocompletion for the editor: keywords, the connection's tables and their columns.
//! After `alias.` or `table.` only that table's columns are offered (aliases come from the
//! FROM / JOIN clauses of the text).

use crate::db::Driver;
use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What a connection has: table names and, once loaded, their columns (name, type).
#[derive(Default)]
pub struct Catalog {
    pub tables: Vec<String>,
    /// Lower-case table name (and, for "schema.table", the bare name too) -> columns.
    pub columns: HashMap<String, Vec<(String, String)>>,
}

impl Catalog {
    pub fn set_columns(&mut self, table: &str, cols: Vec<(String, String)>) {
        if let Some((_, bare)) = table.split_once('.') {
            self.columns.insert(bare.to_lowercase(), cols.clone());
        }
        self.columns.insert(table.to_lowercase(), cols);
    }
}

pub type SharedCatalog = Arc<Mutex<Catalog>>;

const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "ON",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "CREATE TABLE",
    "CREATE VIEW",
    "CREATE INDEX",
    "DROP TABLE",
    "ALTER TABLE",
    "ADD COLUMN",
    "PRIMARY KEY",
    "FOREIGN KEY",
    "REFERENCES",
    "NOT NULL",
    "DEFAULT",
    "DISTINCT",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS NULL",
    "IS NOT NULL",
    "LIKE",
    "BETWEEN",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "UNION",
    "UNION ALL",
    "EXISTS",
    "ASC",
    "DESC",
    "WITH",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "COALESCE",
    "CAST",
];

#[derive(Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub kind: Kind,
    pub detail: String,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Kind {
    Keyword,
    Table,
    Column,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `alias -> table` from `FROM t a`, `JOIN t AS a`, bare `FROM t` and `FROM t a, u b`.
fn aliases(text: &str) -> HashMap<String, String> {
    let spaced = text.replace(',', " , "); // a comma is a word of its own
    // words split at spaces and ( ) ; outside quotes: "my table" is one
    let mut words: Vec<&str> = vec![];
    let (mut start, mut quote) = (None, None);
    for (i, c) in spaced.char_indices() {
        if quote.is_some() {
            quote = quote.filter(|q| *q != c);
        } else if c.is_whitespace() || c == '(' || c == ')' || c == ';' {
            words.extend(start.take().map(|s| &spaced[s..i]));
        } else {
            start.get_or_insert(i);
            quote = Some(c).filter(|c| *c == '"' || *c == '`');
        }
    }
    words.extend(start.map(|s| &spaced[s..]));
    if quote.is_some() {
        // a quote still being typed (b."lo FROM …): the plain split
        words = spaced
            .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ';')
            .filter(|w| !w.is_empty())
            .collect();
    }
    let mut out = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        if !(w.eq_ignore_ascii_case("from")
            || w.eq_ignore_ascii_case("join")
            || w.eq_ignore_ascii_case("update")
            || w.eq_ignore_ascii_case("into"))
        {
            continue;
        }
        let mut at = i + 1;
        // table [AS] alias, then more of them after commas
        while let Some(table) = words
            .get(at)
            .filter(|t| **t != ",")
            .map(|t| t.replace(['"', '`'], ""))
        // "public"."users" -> public.users
        {
            out.insert(table.to_lowercase(), table.to_string());
            // users.x FROM audit.users: audit's users, not another schema's of that name
            if let Some((_, bare)) = table.split_once('.') {
                out.entry(bare.to_lowercase()).or_insert(table.to_string());
            }
            at += 1;
            if words.get(at).is_some_and(|n| n.eq_ignore_ascii_case("as")) {
                at += 1;
            }
            if let Some(a) = words.get(at).filter(|a| {
                a.chars().all(is_word) && !KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(a))
            }) {
                out.insert(a.to_lowercase(), table.to_string());
                at += 1;
            }
            if words.get(at) != Some(&",") {
                break;
            }
            at += 1;
        }
    }
    out
}

/// Suggestions for the word ending at byte `offset`; returns where that word starts.
pub fn suggest(text: &str, offset: usize, catalog: &Catalog) -> (usize, Vec<Suggestion>) {
    // Offsets come from agents too: never cut inside a multi-byte character.
    let before = &text[..text.floor_char_boundary(offset)];
    // aliases of the statement at the cursor, not of others in the editor that reuse a name
    // ponytail: split at every ;, also one inside a string
    let at = before.len();
    let statement = &text[before.rfind(';').map_or(0, |i| i + 1)
        ..text[at..].find(';').map_or(text.len(), |i| at + i)];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(before.len());
    let prefix = before[start..].to_lowercase();
    // b."lo: the opening quote sits between the dot and the word
    let head = before[..start]
        .strip_suffix(['"', '`'])
        .unwrap_or(&before[..start]);
    let qualifier = head.strip_suffix('.').map(|q| {
        let qs = q
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c) || *c == '"' || *c == '`')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(q.len());
        q[qs..]
            .trim_matches(|c| c == '"' || c == '`')
            .to_lowercase()
    });
    let mut out = vec![];
    let col = |name: &str, ty: &str, table: &str| Suggestion {
        label: name.to_string(),
        kind: Kind::Column,
        detail: format!("{ty} · {table}"),
    };
    if let Some(q) = qualifier {
        // alias.col / table.col
        let table = aliases(statement).get(&q).cloned().unwrap_or(q);
        for (name, ty) in catalog
            .columns
            .get(&table.to_lowercase())
            .into_iter()
            .flatten()
        {
            if name.to_lowercase().starts_with(&prefix) {
                out.push(col(name, ty, &table));
            }
        }
        if out.is_empty() {
            // schema.ta: the schema's tables
            for t in &catalog.tables {
                if let Some((s, b)) = t.split_once('.')
                    && s.eq_ignore_ascii_case(&table)
                    && b.to_lowercase().starts_with(&prefix)
                {
                    out.push(Suggestion {
                        label: b.to_string(),
                        kind: Kind::Table,
                        detail: format!("table · {s}"),
                    });
                }
            }
        }
        return (start, out);
    }
    if prefix.is_empty() {
        return (start, out);
    }
    for k in KEYWORDS
        .iter()
        .filter(|k| k.to_lowercase().starts_with(&prefix))
    {
        out.push(Suggestion {
            label: k.to_string(),
            kind: Kind::Keyword,
            detail: "keyword".into(),
        });
    }
    for t in catalog.tables.iter().filter(|t| {
        t.to_lowercase().starts_with(&prefix)
            || t.split_once('.')
                .is_some_and(|(_, b)| b.to_lowercase().starts_with(&prefix))
    }) {
        out.push(Suggestion {
            label: t.clone(),
            kind: Kind::Table,
            detail: "table".into(),
        });
    }
    // Columns of the tables this statement uses first, then everything else.
    let used: Vec<String> = aliases(statement)
        .into_values()
        .map(|t| t.to_lowercase())
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut tables: Vec<(&String, &Vec<(String, String)>)> = catalog.columns.iter().collect();
    tables.sort_by_key(|(t, _)| (!used.contains(t), (*t).clone()));
    for (table, cols) in tables {
        for (name, ty) in cols {
            if name.to_lowercase().starts_with(&prefix) && seen.insert(name.to_lowercase()) {
                out.push(col(name, ty, table));
            }
        }
    }
    (start, out)
}

pub struct SqlCompletion {
    pub catalog: SharedCatalog,
    /// How names are quoted when inserted.
    pub driver: Driver,
}

/// Words Postgres or MySQL reserve that are also common names (user, order, key): unquoted
/// they are no column (SELECT user is current_user).
/// ponytail: the usual suspects, not each dialect's full list
const RESERVED: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "BETWEEN",
    "BY",
    "CASE",
    "CHECK",
    "COLUMN",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_USER",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXISTS",
    "FALSE",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "GRANT",
    "GROUP",
    "HAVING",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "KEYS",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PRIMARY",
    "RANGE",
    "READ",
    "REFERENCES",
    "RIGHT",
    "ROWS",
    "SELECT",
    "SET",
    "SHOW",
    "TABLE",
    "THEN",
    "TO",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USER",
    "USING",
    "VALUES",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
    "WRITE",
];

/// A table or column name as `driver` needs it in SQL: parts that are no plain identifier
/// there get quoted (Postgres "Orders", MySQL `my table`); `force` quotes every part (the user
/// already typed the opening quote).
pub fn sql_name(name: &str, driver: Driver, force: bool) -> String {
    name.split('.')
        .map(|p| {
            let start = p
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            let rest = |c: char| c.is_ascii_alphanumeric() || c == '_';
            // Postgres folds unquoted names to lower case: Orders would become orders
            let plain = start
                && p.chars().all(rest)
                && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(p))
                && (driver != Driver::Postgres || !p.chars().any(|c| c.is_ascii_uppercase()));
            if plain && !force {
                p.to_string()
            } else {
                driver.quote(p)
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

impl CompletionProvider for SqlCompletion {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let text = rope.to_string();
        let (start, items) = suggest(&text, offset, &self.catalog.lock().unwrap());
        // "be|: the typed quote is replaced along with the word, by a fully quoted name
        let quote = if self.driver == Driver::Mysql {
            '`'
        } else {
            '"'
        };
        let quoted = start > 0 && text[..start].ends_with(quote);
        let range = |from: usize| lsp_types::Range {
            start: rope.offset_to_position(from),
            end: rope.offset_to_position(offset),
        };
        let items = items
            .into_iter()
            .take(100)
            .map(|s| CompletionItem {
                label: s.label.clone(),
                kind: Some(match s.kind {
                    Kind::Keyword => CompletionItemKind::KEYWORD,
                    Kind::Table => CompletionItemKind::CLASS,
                    Kind::Column => CompletionItemKind::FIELD,
                }),
                detail: Some(s.detail),
                text_edit: Some(CompletionTextEdit::Edit(match s.kind {
                    Kind::Keyword => TextEdit {
                        range: range(start),
                        new_text: s.label,
                    },
                    _ => TextEdit {
                        range: range(if quoted { start - 1 } else { start }),
                        new_text: sql_name(&s.label, self.driver, quoted),
                    },
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|c| is_word(c) || c == '.')
    }
}

#[cfg(test)]
mod tests {
    use super::{Catalog, Kind, sql_name, suggest};
    use crate::db::Driver;

    #[test]
    fn inserted_names_are_quoted_where_needed() {
        assert_eq!(
            sql_name("public.dams", Driver::Postgres, false),
            "public.dams"
        );
        assert_eq!(
            sql_name("public.Orders", Driver::Postgres, false),
            "public.\"Orders\""
        );
        assert_eq!(sql_name("Orders", Driver::Mysql, false), "Orders");
        assert_eq!(sql_name("my table", Driver::Mysql, false), "`my table`");
        assert_eq!(sql_name("beavers", Driver::Sqlite, true), "\"beavers\""); // typed "be
        // keywords: SELECT user FROM t would be current_user, not the column
        assert_eq!(sql_name("user", Driver::Postgres, false), "\"user\"");
        assert_eq!(sql_name("order", Driver::Mysql, false), "`order`");
    }

    fn catalog() -> Catalog {
        let mut c = Catalog {
            tables: vec!["beavers".into(), "lodges".into(), "public.dams".into()],
            ..Default::default()
        };
        c.set_columns(
            "beavers",
            vec![
                ("name".into(), "TEXT".into()),
                ("lodge_id".into(), "INT".into()),
            ],
        );
        c.set_columns(
            "lodges",
            vec![
                ("name".into(), "TEXT".into()),
                ("river".into(), "TEXT".into()),
            ],
        );
        c.set_columns("public.dams", vec![("height".into(), "numeric".into())]);
        c
    }

    #[test]
    fn offsets_inside_characters_are_safe() {
        let c = catalog();
        let text = "SELECT 'ä' FROM be";
        for offset in 0..text.len() + 3 {
            suggest(text, offset, &c);
        }
    }

    #[test]
    fn keywords_tables_and_columns() {
        let c = catalog();
        let (start, s) = suggest("SEL", 3, &c);
        assert_eq!(
            (start, s[0].label.as_str(), s[0].kind),
            (0, "SELECT", Kind::Keyword)
        );
        let (_, s) = suggest("SELECT * FROM lo", 16, &c);
        assert!(
            s.iter()
                .any(|x| x.label == "lodges" && x.kind == Kind::Table)
        );
        let (_, s) = suggest("SELECT * FROM da", 16, &c);
        assert!(s.iter().any(|x| x.label == "public.dams"));
    }

    #[test]
    fn alias_dot_offers_that_tables_columns() {
        let c = catalog();
        let text = "SELECT l. FROM beavers b JOIN lodges AS l ON l.id = b.lodge_id";
        let (start, s) = suggest(text, 9, &c);
        assert_eq!(start, 9);
        assert_eq!(
            s.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(),
            ["name", "river"]
        );
        // a quoted name with a space is one name
        let mut c = c;
        c.set_columns("my table", vec![("x".into(), "INT".into())]);
        let (_, s) = suggest("SELECT t. FROM \"my table\" t", 9, &c);
        assert_eq!(
            s.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(),
            ["x"]
        );
        let (_, s) = suggest("SELECT beavers.lo FROM beavers", 17, &c);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].label, "lodge_id");
        // Unqualified: columns of the statement's tables come first.
        let (_, s) = suggest("SELECT ri FROM lodges", 9, &c);
        assert_eq!(s[0].label, "river");
        let (_, s) = suggest("SELECT dams.h FROM public.dams", 13, &c);
        assert_eq!(s[0].label, "height");
        let (_, s) = suggest("SELECT d.h FROM \"public\".\"dams\" d", 10, &c);
        assert_eq!(s[0].label, "height"); // the quoting the app writes itself
        // b."lo: still b's columns, the quote typed after the dot
        let (_, s) = suggest("SELECT b.\"lo FROM beavers b", 12, &c);
        assert_eq!(
            s.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(),
            ["lodge_id"]
        );
        // the alias of this statement, not one of the same name further down the editor
        let (_, s) = suggest("SELECT b. FROM beavers b; SELECT * FROM lodges b", 9, &c);
        assert_eq!(s[0].label, "name");
        assert!(s.iter().any(|x| x.label == "lodge_id"), "{s:?}");
        let (_, s) = suggest("SELECT l. FROM beavers b, lodges l", 9, &c);
        assert_eq!(s[0].label, "name"); // comma-separated FROM list
        let (start, s) = suggest("SELECT * FROM public.da", 23, &c);
        assert_eq!((start, s[0].label.as_str()), (21, "dams")); // schema.
        let mut c2 = catalog();
        c2.set_columns("audit.dams", vec![("changed".into(), "TIMESTAMP".into())]);
        let (_, s) = suggest("SELECT dams.h FROM public.dams", 13, &c2);
        assert_eq!(s[0].label, "height"); // public's dams, though audit.dams loaded last
    }
}
