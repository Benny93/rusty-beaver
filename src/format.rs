//! SQL formatter: upper-case keywords, each major clause on its own line, select lists and
//! conditions indented. Strings, quoted identifiers and comments are copied untouched.

const CLAUSES: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION ALL",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "VALUES",
    "SET",
    "INSERT INTO",
    "UPDATE",
    "DELETE FROM",
    "LEFT OUTER JOIN",
    "RIGHT OUTER JOIN",
    "FULL OUTER JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "INNER JOIN",
    "CROSS JOIN",
    "FULL JOIN",
    "JOIN",
    "RETURNING",
    "WITH",
];

const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASE",
    "CAST",
    "COLUMN",
    "COMMIT",
    "CREATE",
    "CROSS",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "FOREIGN",
    "FROM",
    "FULL",
    "GROUP",
    "HAVING",
    "IF",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
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
    "REFERENCES",
    "RETURNING",
    "RIGHT",
    "ROLLBACK",
    "SELECT",
    "SET",
    "TABLE",
    "THEN",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WITH",
];

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    Word(&'a str),
    /// Strings, quoted names, comments, numbers, operators: copied as they are.
    Raw(&'a str),
    Comma,
    Open,
    Close,
    Semi,
}

fn lex(sql: &str) -> Vec<Tok<'_>> {
    let b = sql.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => {
                i += 1;
                // a doubled quote ('it''s') stays inside
                while i < b.len() && (b[i] != c || b.get(i + 1) == Some(&c)) {
                    i += if b[i] == c { 2 } else { 1 };
                }
                i += 1;
                out.push(Tok::Raw(&sql[start..i.min(b.len())]));
            }
            // :name parameters and $1 placeholders stay one piece (not ": name", "$ 1")
            b':' if b
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') =>
            {
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Tok::Raw(&sql[start..i]));
            }
            b'$' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                out.push(Tok::Raw(&sql[start..i]));
            }
            b'$' if crate::db::dollar_quote_end(sql, i).is_some() => {
                i = crate::db::dollar_quote_end(sql, i).unwrap_or(b.len());
                out.push(Tok::Raw(&sql[start..i]));
            }
            // MySQL # comments too; on Postgres (#> and #- operators) the rest of the line is
            // only kept as it is
            b'#' | b'-' if c == b'#' || b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                out.push(Tok::Raw(&sql[start..i]));
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                out.push(Tok::Raw(&sql[start..i]));
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b'(' => {
                out.push(Tok::Open);
                i += 1;
            }
            b')' => {
                out.push(Tok::Close);
                i += 1;
            }
            b';' => {
                out.push(Tok::Semi);
                i += 1;
            }
            _ if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 => {
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'_'
                        || b[i] == b'.'
                        || b[i] >= 0x80)
                {
                    i += 1;
                    // the sign of an exponent: 1.5e-3
                    if c.is_ascii_digit()
                        && matches!(b[i - 1], b'e' | b'E')
                        && matches!(b.get(i), Some(b'+' | b'-'))
                        && b.get(i + 1).is_some_and(u8::is_ascii_digit)
                    {
                        i += 1;
                    }
                }
                // E'…', N'…', X'…', B'…': the prefix belongs to its string
                if i == start + 1 && b"EeNnXxBb".contains(&c) && b.get(i) == Some(&b'\'') {
                    i += 1;
                    while i < b.len() && (b[i] != b'\'' || b.get(i + 1) == Some(&b'\'')) {
                        i += if b[i] == b'\'' || c.eq_ignore_ascii_case(&b'e') && b[i] == b'\\' {
                            2
                        } else {
                            1
                        };
                    }
                    i = (i + 1).min(b.len());
                    out.push(Tok::Raw(&sql[start..i]));
                    continue;
                }
                out.push(Tok::Word(&sql[start..i]));
            }
            _ => {
                // operators: one run of punctuation
                // where another token starts: a parameter, a comment, a dollar quote
                let starts = |i: usize| {
                    let next = b.get(i + 1);
                    b[i] == b':'
                        && b[i - 1] != b':'
                        && next.is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
                        || b[i] == b'$' && next.is_some_and(u8::is_ascii_digit)
                        || b[i] == b'-' && next == Some(&b'-')
                        || b[i] == b'/' && next == Some(&b'*')
                        || b[i] == b'$' && crate::db::dollar_quote_end(sql, i).is_some()
                };
                while i < b.len()
                    && !b[i].is_ascii_alphanumeric()
                    && !b[i].is_ascii_whitespace()
                    && !b"'\"`,();_".contains(&b[i])
                    && !(i > start && starts(i))
                // a=:id, a=$1, a=--x
                {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                out.push(Tok::Raw(&sql[start..i]));
            }
        }
    }
    out
}

pub fn format_sql(sql: &str) -> String {
    let toks = lex(sql);
    let mut out = String::new();
    let mut depth = 0usize; // parentheses
    let mut i = 0;
    let newline = |out: &mut String, indent: usize| {
        while out.ends_with(' ') {
            out.pop();
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&"    ".repeat(indent));
    };
    while i < toks.len() {
        match &toks[i] {
            Tok::Word(w) => {
                // Longest clause starting here ("LEFT OUTER JOIN" before "LEFT JOIN" ...).
                let clause = CLAUSES.iter().find(|c| {
                    let parts: Vec<&str> = c.split(' ').collect();
                    parts.iter().enumerate().all(|(k, p)| matches!(toks.get(i + k), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(p)))
                });
                if let Some(c) = clause {
                    newline(&mut out, depth);
                    out.push_str(c);
                    i += c.split(' ').count();
                    // select / set lists and conditions continue on an indented line
                    if matches!(
                        *c,
                        "SELECT" | "SET" | "WHERE" | "HAVING" | "GROUP BY" | "ORDER BY"
                    ) {
                        newline(&mut out, depth + 1);
                    } else {
                        out.push(' ');
                    }
                    continue;
                }
                if (w.eq_ignore_ascii_case("AND") || w.eq_ignore_ascii_case("OR")) && depth == 0 {
                    newline(&mut out, 1);
                }
                if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(w)) {
                    out.push_str(&w.to_uppercase());
                } else {
                    out.push_str(w);
                }
                out.push(' ');
            }
            Tok::Raw(r) => {
                out.push_str(r);
                if r.starts_with("--") || r.starts_with('#') {
                    out.push('\n'); // a line comment ends its line
                } else {
                    out.push(' ');
                }
            }
            Tok::Comma => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push(',');
                if depth == 0 {
                    newline(&mut out, 1);
                } else {
                    out.push(' ');
                }
            }
            Tok::Open => {
                // "count(" for function calls, "IN (" after keywords
                let call = i > 0
                    && matches!(toks[i - 1], Tok::Word(w) if !KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(w)));
                if call {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                }
                out.push('(');
                depth += 1;
            }
            Tok::Close => {
                while out.ends_with(' ') {
                    out.pop();
                }
                depth = depth.saturating_sub(1);
                out.push_str(") ");
            }
            Tok::Semi => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(";\n\n");
                depth = 0;
            }
        }
        i += 1;
    }
    out.trim_end().to_string() + "\n"
}

/// Comment the lines `range` touches with `-- `, or uncomment them when all already are.
pub fn toggle_comment(text: &str, range: std::ops::Range<usize>) -> String {
    let start = text[..range.start.min(text.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let end = text[range.end.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i);
    let block = &text[start..end];
    let lines: Vec<&str> = block.split('\n').collect();
    let commented = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with("--"));
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let new: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else if commented {
                let at = l.find("--").unwrap_or(0);
                let rest = &l[at + 2..];
                format!("{}{}", &l[..at], rest.strip_prefix(' ').unwrap_or(rest))
            } else {
                format!("{}-- {}", &l[..indent], &l[indent..])
            }
        })
        .collect();
    format!("{}{}{}", &text[..start], new.join("\n"), &text[end..])
}

#[cfg(test)]
mod tests {
    use super::{format_sql, toggle_comment};

    #[test]
    fn literals_survive_formatting() {
        let out = format_sql("select 'it''s', $$a, b$$, 1.5e-3 from t");
        assert!(out.contains("'it''s'"), "{out}");
        assert!(out.contains("$$a, b$$"), "{out}");
        assert!(out.contains("1.5e-3"), "{out}");
        let out = format_sql("select * from t where a=:id and b=$1 and c = E'it\\'s' -- x\n");
        let my = format_sql("select a # note\n+ 1 from t");
        assert!(my.contains("# note\n"), "{my}"); // + 1 stays out of the MySQL comment
        for kept in [":id", "$1", "E'it\\'s'"] {
            assert!(out.contains(kept), "{kept} in {out}");
        }
        assert!(!out.contains(": id") && !out.contains("$ 1"), "{out}");
        // non-breaking space indent: no slicing inside a character
        assert!(toggle_comment("\u{a0}x\n y", 0..5).contains("-- "));
    }

    #[test]
    fn comments_toggle() {
        let t = "SELECT 1;\n  SELECT 2,\n  3;\nSELECT 4;";
        let c = toggle_comment(t, 12..24); // from line 2 into line 3
        assert_eq!(c, "SELECT 1;\n  -- SELECT 2,\n  -- 3;\nSELECT 4;");
        assert_eq!(toggle_comment(&c, 14..30), t);
        assert_eq!(toggle_comment("x", 0..0), "-- x");
    }

    #[test]
    fn formats_clauses_and_keeps_strings() {
        let sql = "select b.name, count(m.id) as n from beavers b left join missions m on m.leader_id = b.id where b.rank = 'select, from' and b.id > 1 group by b.id order by n desc limit 5;";
        assert_eq!(
            format_sql(sql),
            "SELECT\n    b.name,\n    count(m.id) AS n\nFROM beavers b\nLEFT JOIN missions m ON m.leader_id = b.id\nWHERE\n    b.rank = 'select, from'\n    AND b.id > 1\nGROUP BY\n    b.id\nORDER BY\n    n DESC\nLIMIT 5;\n"
        );
    }

    #[test]
    fn comments_and_subqueries_survive() {
        let out = format_sql(
            "-- top lodges\nselect * from lodges where id in (select lodge_id from beavers) /* keep */",
        );
        assert!(
            out.starts_with("-- top lodges\nSELECT\n    *\nFROM lodges\nWHERE\n    id IN ("),
            "{out}"
        );
        assert!(out.contains("/* keep */"));
        // formatting twice changes nothing
        assert_eq!(format_sql(&out), out);
    }
}
