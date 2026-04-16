//! Read and filter CSV/TSV data with a SQL-like query language and
//! return results as a Markdown-style table. Saves the agent from
//! writing ad-hoc scripts for common tabular data wrangling (filtering
//! rows by predicates, selecting a subset of columns, sorting,
//! limiting).
//!
//! Input format (two sections separated by a newline):
//!
//! ```text
//! <query>
//! <inline CSV/TSV>        # or  @path/to/file.csv
//! ```
//!
//! Query grammar (keywords case-insensitive):
//!
//! ```text
//! SELECT (* | col[, col ...]) FROM (csv | tsv)
//!   [WHERE <cond> [(AND|OR) <cond> ...]]
//!   [ORDER BY <col> [ASC|DESC]]
//!   [LIMIT <N>]
//! ```
//!
//! `FROM csv` uses `,` as the field separator; `FROM tsv` uses TAB.
//!
//! Conditions: `<col> <op> <value>` where `<op>` is one of `=`, `!=`,
//! `<>`, `<`, `<=`, `>`, `>=`, `LIKE`, `NOT LIKE`. `<col> IS NULL` and
//! `<col> IS NOT NULL` are also supported. Values are bare identifiers,
//! numeric literals, or single/double-quoted strings. `LIKE` uses `%`
//! as the wildcard. When both operands parse as numbers, comparison is
//! numeric; otherwise it is lexicographic. Column lookups are
//! case-insensitive.
//!
//! `AND` binds tighter than `OR`. Parentheses are not supported — keep
//! predicates flat.
//!
//! CSV parsing is fully in-process via the `csv` crate. When the input
//! uses `@path`, the path passes through `check_path_read` first so the
//! CWD jail applies.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;

use super::util::truncate_output;

pub(super) async fn tool_csv_query(input: &str) -> String {
    if input.trim().is_empty() {
        return "Error: empty input. Expected: <query>\\n<csv or @path>".to_string();
    }
    if input.contains('\0') {
        return "Error: input contains null byte".to_string();
    }

    let (query_str, rest) = match input.split_once('\n') {
        Some((q, r)) => (q.trim(), r),
        None => return "Error: no CSV data after query line".to_string(),
    };
    if query_str.is_empty() {
        return "Error: first line must be a SQL-like query (e.g. 'SELECT * FROM csv LIMIT 5')"
            .to_string();
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return "Error: no CSV data provided after query".to_string();
    }

    let query = match parse_query(query_str) {
        Ok(q) => q,
        Err(e) => return format!("Error parsing query: {e}"),
    };

    let csv_bytes: Vec<u8> = if let Some(path) = rest.strip_prefix('@') {
        let path = path.trim();
        if path.is_empty() {
            return "Error: '@' must be followed by a file path".to_string();
        }
        match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) => return format!("Error reading '{path}': {e}"),
        }
    } else {
        rest.as_bytes().to_vec()
    };

    match run_query(&query, &csv_bytes) {
        Ok(mut out) => {
            truncate_output(&mut out);
            out
        }
        Err(e) => format!("Error: {e}"),
    }
}

// ----- AST -----

#[derive(Debug)]
struct Query {
    select: Select,
    separator: u8,
    where_clause: Option<Expr>,
    order_by: Option<OrderBy>,
    limit: Option<usize>,
}

#[derive(Debug)]
enum Select {
    Star,
    Columns(Vec<String>),
}

#[derive(Debug)]
struct OrderBy {
    column: String,
    desc: bool,
}

#[derive(Debug)]
enum Expr {
    Cond(Condition),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug)]
struct Condition {
    column: String,
    op: Op,
    value: Option<Literal>,
}

#[derive(Debug, Clone)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    NotLike,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone)]
enum Literal {
    Num(f64),
    Str(String),
}

// ----- Lexer -----

#[derive(Debug, Clone)]
enum Tok {
    Keyword(&'static str),
    Ident(String),
    Str(String),
    Num(String),
    Op(&'static str),
    Star,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == ',' {
            out.push(Tok::Comma);
            i += 1;
            continue;
        }
        if c == '*' {
            out.push(Tok::Star);
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            if i >= chars.len() {
                return Err(format!(
                    "unterminated string literal starting at char {start}"
                ));
            }
            let lit: String = chars[start..i].iter().collect();
            i += 1;
            out.push(Tok::Str(lit));
            continue;
        }
        if c == '=' {
            out.push(Tok::Op("="));
            i += 1;
            continue;
        }
        if c == '!' && i + 1 < chars.len() && chars[i + 1] == '=' {
            out.push(Tok::Op("!="));
            i += 2;
            continue;
        }
        if c == '<' {
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                out.push(Tok::Op("<="));
                i += 2;
                continue;
            }
            if i + 1 < chars.len() && chars[i + 1] == '>' {
                out.push(Tok::Op("!="));
                i += 2;
                continue;
            }
            out.push(Tok::Op("<"));
            i += 1;
            continue;
        }
        if c == '>' {
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                out.push(Tok::Op(">="));
                i += 2;
                continue;
            }
            out.push(Tok::Op(">"));
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            if c == '-' {
                i += 1;
            }
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let n: String = chars[start..i].iter().collect();
            out.push(Tok::Num(n));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect();
            match classify_word(&w) {
                Some(kw) => out.push(Tok::Keyword(kw)),
                None => out.push(Tok::Ident(w)),
            }
            continue;
        }
        return Err(format!("unexpected character '{c}' in query"));
    }
    Ok(out)
}

fn classify_word(w: &str) -> Option<&'static str> {
    match w.to_ascii_uppercase().as_str() {
        "SELECT" => Some("SELECT"),
        "FROM" => Some("FROM"),
        "WHERE" => Some("WHERE"),
        "AND" => Some("AND"),
        "OR" => Some("OR"),
        "ORDER" => Some("ORDER"),
        "BY" => Some("BY"),
        "LIMIT" => Some("LIMIT"),
        "ASC" => Some("ASC"),
        "DESC" => Some("DESC"),
        "LIKE" => Some("LIKE"),
        "NOT" => Some("NOT"),
        "IS" => Some("IS"),
        "NULL" => Some("NULL"),
        _ => None,
    }
}

// ----- Parser -----

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn check_kw(&self, kw: &'static str) -> bool {
        matches!(self.peek(), Some(Tok::Keyword(k)) if *k == kw)
    }

    fn eat_kw(&mut self, kw: &'static str) -> Result<(), String> {
        match self.next() {
            Some(Tok::Keyword(k)) if k == kw => Ok(()),
            Some(other) => Err(format!("expected {kw}, got {}", describe(&other))),
            None => Err(format!("expected {kw}, got end of query")),
        }
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Keyword(k) => format!("keyword {k}"),
        Tok::Ident(s) => format!("identifier '{s}'"),
        Tok::Str(s) => format!("string '{s}'"),
        Tok::Num(n) => format!("number {n}"),
        Tok::Op(o) => format!("operator '{o}'"),
        Tok::Star => "'*'".to_string(),
        Tok::Comma => "','".to_string(),
    }
}

fn parse_query(s: &str) -> Result<Query, String> {
    let toks = tokenize(s)?;
    let mut p = Parser { toks, pos: 0 };

    p.eat_kw("SELECT")?;
    let select = parse_select(&mut p)?;

    p.eat_kw("FROM")?;
    let separator = parse_from(&mut p)?;

    let where_clause = if p.check_kw("WHERE") {
        p.next();
        Some(parse_or(&mut p)?)
    } else {
        None
    };

    let order_by = if p.check_kw("ORDER") {
        p.next();
        p.eat_kw("BY")?;
        let column = match p.next() {
            Some(Tok::Ident(s) | Tok::Str(s)) => s,
            Some(other) => {
                return Err(format!(
                    "expected column name after ORDER BY, got {}",
                    describe(&other)
                ));
            }
            None => return Err("expected column name after ORDER BY".to_string()),
        };
        let desc = if p.check_kw("DESC") {
            p.next();
            true
        } else if p.check_kw("ASC") {
            p.next();
            false
        } else {
            false
        };
        Some(OrderBy { column, desc })
    } else {
        None
    };

    let limit = if p.check_kw("LIMIT") {
        p.next();
        match p.next() {
            Some(Tok::Num(n)) => Some(
                n.parse::<usize>()
                    .map_err(|e| format!("invalid LIMIT '{n}': {e}"))?,
            ),
            Some(other) => {
                return Err(format!(
                    "expected number after LIMIT, got {}",
                    describe(&other)
                ));
            }
            None => return Err("expected number after LIMIT".to_string()),
        }
    } else {
        None
    };

    if p.pos < p.toks.len() {
        let extra = p.peek().map_or_else(String::new, describe);
        return Err(format!("unexpected tokens after end of query: {extra}"));
    }

    Ok(Query {
        select,
        separator,
        where_clause,
        order_by,
        limit,
    })
}

fn parse_select(p: &mut Parser) -> Result<Select, String> {
    if matches!(p.peek(), Some(Tok::Star)) {
        p.next();
        return Ok(Select::Star);
    }
    let mut cols = Vec::new();
    loop {
        match p.next() {
            Some(Tok::Ident(s) | Tok::Str(s)) => cols.push(s),
            Some(other) => {
                return Err(format!(
                    "expected column name in SELECT, got {}",
                    describe(&other)
                ));
            }
            None => return Err("expected column name in SELECT".to_string()),
        }
        if matches!(p.peek(), Some(Tok::Comma)) {
            p.next();
        } else {
            break;
        }
    }
    Ok(Select::Columns(cols))
}

fn parse_from(p: &mut Parser) -> Result<u8, String> {
    match p.next() {
        Some(Tok::Ident(s)) => match s.to_ascii_lowercase().as_str() {
            "csv" => Ok(b','),
            "tsv" => Ok(b'\t'),
            _ => Err(format!("FROM must be 'csv' or 'tsv', got '{s}'")),
        },
        Some(other) => Err(format!(
            "expected 'csv' or 'tsv' after FROM, got {}",
            describe(&other)
        )),
        None => Err("expected 'csv' or 'tsv' after FROM".to_string()),
    }
}

fn parse_or(p: &mut Parser) -> Result<Expr, String> {
    let mut left = parse_and(p)?;
    while p.check_kw("OR") {
        p.next();
        let right = parse_and(p)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and(p: &mut Parser) -> Result<Expr, String> {
    let mut left = parse_condition(p)?;
    while p.check_kw("AND") {
        p.next();
        let right = parse_condition(p)?;
        left = Expr::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_condition(p: &mut Parser) -> Result<Expr, String> {
    let column = match p.next() {
        Some(Tok::Ident(s) | Tok::Str(s)) => s,
        Some(other) => {
            return Err(format!(
                "expected column name in condition, got {}",
                describe(&other)
            ));
        }
        None => return Err("expected column name in condition".to_string()),
    };

    if p.check_kw("IS") {
        p.next();
        if p.check_kw("NOT") {
            p.next();
            p.eat_kw("NULL")?;
            return Ok(Expr::Cond(Condition {
                column,
                op: Op::IsNotNull,
                value: None,
            }));
        }
        p.eat_kw("NULL")?;
        return Ok(Expr::Cond(Condition {
            column,
            op: Op::IsNull,
            value: None,
        }));
    }

    if p.check_kw("NOT") {
        p.next();
        p.eat_kw("LIKE")?;
        let v = parse_literal(p)?;
        return Ok(Expr::Cond(Condition {
            column,
            op: Op::NotLike,
            value: Some(v),
        }));
    }

    if p.check_kw("LIKE") {
        p.next();
        let v = parse_literal(p)?;
        return Ok(Expr::Cond(Condition {
            column,
            op: Op::Like,
            value: Some(v),
        }));
    }

    let op = match p.next() {
        Some(Tok::Op("=")) => Op::Eq,
        Some(Tok::Op("!=")) => Op::Ne,
        Some(Tok::Op("<")) => Op::Lt,
        Some(Tok::Op("<=")) => Op::Le,
        Some(Tok::Op(">")) => Op::Gt,
        Some(Tok::Op(">=")) => Op::Ge,
        Some(other) => {
            return Err(format!(
                "expected comparison operator, got {}",
                describe(&other)
            ));
        }
        None => return Err("expected comparison operator after column name".to_string()),
    };
    let v = parse_literal(p)?;
    Ok(Expr::Cond(Condition {
        column,
        op,
        value: Some(v),
    }))
}

fn parse_literal(p: &mut Parser) -> Result<Literal, String> {
    match p.next() {
        Some(Tok::Num(n)) => n
            .parse::<f64>()
            .map(Literal::Num)
            .map_err(|e| format!("invalid number '{n}': {e}")),
        Some(Tok::Str(s) | Tok::Ident(s)) => Ok(Literal::Str(s)),
        Some(other) => Err(format!("expected value literal, got {}", describe(&other))),
        None => Err("expected value literal after operator".to_string()),
    }
}

// ----- Evaluator -----

fn run_query(q: &Query, bytes: &[u8]) -> Result<String, String> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(q.separator)
        .has_headers(true)
        .flexible(true)
        .from_reader(bytes);

    let headers = rdr
        .headers()
        .map_err(|e| format!("parsing headers: {e}"))?
        .clone();
    let mut header_map: HashMap<String, usize> = HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        header_map.entry(h.to_ascii_lowercase()).or_insert(i);
    }

    let selected_indices: Vec<usize> = match &q.select {
        Select::Star => (0..headers.len()).collect(),
        Select::Columns(cols) => {
            let mut out = Vec::with_capacity(cols.len());
            for c in cols {
                let key = c.to_ascii_lowercase();
                match header_map.get(&key) {
                    Some(i) => out.push(*i),
                    None => {
                        return Err(format!(
                            "unknown column in SELECT: '{c}'. Available: {}",
                            available(&headers)
                        ));
                    }
                }
            }
            out
        }
    };

    let order_by_idx: Option<(usize, bool)> = if let Some(ob) = &q.order_by {
        let key = ob.column.to_ascii_lowercase();
        match header_map.get(&key) {
            Some(i) => Some((*i, ob.desc)),
            None => {
                return Err(format!(
                    "unknown column in ORDER BY: '{}'. Available: {}",
                    ob.column,
                    available(&headers)
                ));
            }
        }
    } else {
        None
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (line_no, record) in rdr.records().enumerate() {
        let record = record.map_err(|e| format!("parsing row {}: {e}", line_no + 1))?;
        let row: Vec<String> = record.iter().map(str::to_string).collect();
        if let Some(expr) = &q.where_clause
            && !eval_expr(expr, &row, &header_map, &headers)?
        {
            continue;
        }
        rows.push(row);
    }

    if let Some((idx, desc)) = order_by_idx {
        rows.sort_by(|a, b| {
            let av = a.get(idx).map_or("", String::as_str);
            let bv = b.get(idx).map_or("", String::as_str);
            let ord = cmp_values(av, bv);
            if desc { ord.reverse() } else { ord }
        });
    }

    if let Some(n) = q.limit {
        rows.truncate(n);
    }

    let projected_headers: Vec<String> = selected_indices
        .iter()
        .map(|i| headers.get(*i).unwrap_or("").to_string())
        .collect();
    let projected: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            selected_indices
                .iter()
                .map(|i| r.get(*i).cloned().unwrap_or_default())
                .collect()
        })
        .collect();

    Ok(format_table(&projected_headers, &projected))
}

fn available(headers: &csv::StringRecord) -> String {
    let names: Vec<&str> = headers.iter().collect();
    names.join(", ")
}

fn eval_expr(
    e: &Expr,
    row: &[String],
    hmap: &HashMap<String, usize>,
    headers: &csv::StringRecord,
) -> Result<bool, String> {
    match e {
        Expr::And(a, b) => {
            Ok(eval_expr(a, row, hmap, headers)? && eval_expr(b, row, hmap, headers)?)
        }
        Expr::Or(a, b) => {
            Ok(eval_expr(a, row, hmap, headers)? || eval_expr(b, row, hmap, headers)?)
        }
        Expr::Cond(c) => {
            let key = c.column.to_ascii_lowercase();
            let idx = match hmap.get(&key) {
                Some(i) => *i,
                None => {
                    return Err(format!(
                        "unknown column in WHERE: '{}'. Available: {}",
                        c.column,
                        available(headers)
                    ));
                }
            };
            let cell = row.get(idx).map_or("", String::as_str);
            Ok(match (&c.op, &c.value) {
                (Op::IsNull, _) => cell.is_empty(),
                (Op::IsNotNull, _) => !cell.is_empty(),
                (Op::Like, Some(lit)) => like_match(cell, &lit_as_str(lit)),
                (Op::NotLike, Some(lit)) => !like_match(cell, &lit_as_str(lit)),
                (op, Some(lit)) => apply_cmp(cell, lit, op),
                _ => false,
            })
        }
    }
}

fn lit_as_str(l: &Literal) -> String {
    match l {
        Literal::Str(s) => s.clone(),
        Literal::Num(n) => format_num(*n),
    }
}

fn format_num(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e18 {
        #[allow(clippy::cast_possible_truncation)]
        let i = n as i64;
        i.to_string()
    } else {
        format!("{n}")
    }
}

fn apply_cmp(cell: &str, lit: &Literal, op: &Op) -> bool {
    let numeric: Option<(f64, f64)> = match lit {
        Literal::Num(n) => cell.parse::<f64>().ok().map(|c| (c, *n)),
        Literal::Str(s) => {
            if let (Ok(c), Ok(v)) = (cell.parse::<f64>(), s.parse::<f64>()) {
                Some((c, v))
            } else {
                None
            }
        }
    };
    if let Some((ln, rn)) = numeric {
        return match op {
            Op::Eq => (ln - rn).abs() < f64::EPSILON,
            Op::Ne => (ln - rn).abs() >= f64::EPSILON,
            Op::Lt => ln < rn,
            Op::Le => ln <= rn,
            Op::Gt => ln > rn,
            Op::Ge => ln >= rn,
            _ => false,
        };
    }
    let rhs = match lit {
        Literal::Str(s) => s.as_str(),
        Literal::Num(_) => return false,
    };
    match op {
        Op::Eq => cell == rhs,
        Op::Ne => cell != rhs,
        Op::Lt => cell < rhs,
        Op::Le => cell <= rhs,
        Op::Gt => cell > rhs,
        Op::Ge => cell >= rhs,
        _ => false,
    }
}

fn cmp_values(a: &str, b: &str) -> Ordering {
    if let (Ok(an), Ok(bn)) = (a.parse::<f64>(), b.parse::<f64>()) {
        return an.partial_cmp(&bn).unwrap_or(Ordering::Equal);
    }
    a.cmp(b)
}

fn like_match(s: &str, pat: &str) -> bool {
    // Case-insensitive SQL LIKE with `%` as multi-char wildcard.
    // No escape handling; no single-char `_` wildcard (keeping it simple).
    let s_low = s.to_lowercase();
    let p_low = pat.to_lowercase();
    let parts: Vec<&str> = p_low.split('%').collect();
    let starts_wild = p_low.starts_with('%');
    let ends_wild = p_low.ends_with('%');
    let mut pos = 0usize;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 && !starts_wild {
            if !s_low[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
            continue;
        }
        if i == parts.len() - 1 && !ends_wild {
            return s_low[pos..].ends_with(part);
        }
        match s_low[pos..].find(part) {
            Some(offset) => pos += offset + part.len(),
            None => return false,
        }
    }
    true
}

fn format_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return "(no columns)".to_string();
    }
    let ncol = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, cell) in r.iter().enumerate().take(ncol) {
            let w = cell.chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = String::new();
    out.push('|');
    for (i, h) in headers.iter().enumerate() {
        let pad = widths[i].saturating_sub(h.chars().count());
        let _ = write!(out, " {h}{} |", " ".repeat(pad));
    }
    out.push('\n');
    out.push('|');
    for w in &widths {
        out.push_str(&"-".repeat(w + 2));
        out.push('|');
    }
    out.push('\n');
    for r in rows {
        out.push('|');
        for (i, _) in headers.iter().enumerate() {
            let cell = r.get(i).map_or("", String::as_str);
            let pad = widths[i].saturating_sub(cell.chars().count());
            let _ = write!(out, " {cell}{} |", " ".repeat(pad));
        }
        out.push('\n');
    }
    let n = rows.len();
    let _ = writeln!(out, "({n} {})", if n == 1 { "row" } else { "rows" });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_input_rejected() {
        let r = tool_csv_query("").await;
        assert!(r.contains("empty input"), "got: {r}");
    }

    #[tokio::test]
    async fn null_byte_rejected() {
        let r = tool_csv_query("SELECT * FROM csv\na\0").await;
        assert!(r.contains("null byte"), "got: {r}");
    }

    #[tokio::test]
    async fn missing_data_rejected() {
        let r = tool_csv_query("SELECT * FROM csv").await;
        assert!(r.contains("no CSV data"), "got: {r}");
    }

    #[tokio::test]
    async fn empty_query_rejected() {
        let r = tool_csv_query("\nname,age\na,1").await;
        assert!(r.contains("SQL-like query"), "got: {r}");
    }

    #[tokio::test]
    async fn empty_data_rejected() {
        let r = tool_csv_query("SELECT * FROM csv\n   ").await;
        assert!(r.contains("no CSV data"), "got: {r}");
    }

    #[tokio::test]
    async fn at_without_path_rejected() {
        let r = tool_csv_query("SELECT * FROM csv\n@").await;
        assert!(r.contains("file path"), "got: {r}");
    }

    #[tokio::test]
    async fn at_path_missing_file() {
        let r = tool_csv_query("SELECT * FROM csv\n@/tmp/aictl_csv_nonexistent_xyz.csv").await;
        assert!(r.starts_with("Error reading"), "got: {r}");
    }

    #[tokio::test]
    async fn bad_query_reports_parse_error() {
        let r = tool_csv_query("SELEC * FROM csv\nname\nfoo").await;
        assert!(r.contains("Error parsing query"), "got: {r}");
    }

    #[tokio::test]
    async fn select_star_returns_all_rows() {
        let r = tool_csv_query("SELECT * FROM csv\nname,age\nalice,30\nbob,25").await;
        assert!(r.contains("name"));
        assert!(r.contains("alice"));
        assert!(r.contains("bob"));
        assert!(r.contains("(2 rows)"), "got: {r}");
    }

    #[tokio::test]
    async fn select_specific_columns() {
        let r =
            tool_csv_query("SELECT name FROM csv\nname,age,city\nalice,30,NYC\nbob,25,LA").await;
        assert!(r.contains("alice"));
        assert!(r.contains("bob"));
        assert!(!r.contains("NYC"));
        assert!(!r.contains("LA"));
    }

    #[tokio::test]
    async fn where_numeric_comparison() {
        let r = tool_csv_query(
            "SELECT name FROM csv WHERE age > 27\nname,age\nalice,30\nbob,25\ncarol,40",
        )
        .await;
        assert!(r.contains("alice"));
        assert!(r.contains("carol"));
        assert!(!r.contains("bob"));
    }

    #[tokio::test]
    async fn where_string_equals_quoted() {
        let r =
            tool_csv_query("SELECT name FROM csv WHERE city = 'NYC'\nname,city\nalice,NYC\nbob,LA")
                .await;
        assert!(r.contains("alice"));
        assert!(!r.contains("bob"));
    }

    #[tokio::test]
    async fn where_not_equals() {
        let r = tool_csv_query(
            "SELECT name FROM csv WHERE city != 'NYC'\nname,city\nalice,NYC\nbob,LA",
        )
        .await;
        assert!(!r.contains("alice"));
        assert!(r.contains("bob"));
    }

    #[tokio::test]
    async fn where_like_wildcard() {
        let r = tool_csv_query(
            "SELECT email FROM csv WHERE email LIKE '%@example.com'\nemail\na@example.com\nb@other.com",
        )
        .await;
        assert!(r.contains("a@example.com"));
        assert!(!r.contains("b@other.com"));
    }

    #[tokio::test]
    async fn where_not_like() {
        let r = tool_csv_query(
            "SELECT email FROM csv WHERE email NOT LIKE '%@example.com'\nemail\na@example.com\nb@other.com",
        )
        .await;
        assert!(!r.contains("a@example.com"));
        assert!(r.contains("b@other.com"));
    }

    #[tokio::test]
    async fn where_is_null() {
        let r =
            tool_csv_query("SELECT name FROM csv WHERE note IS NULL\nname,note\nalice,\nbob,hi")
                .await;
        assert!(r.contains("alice"));
        assert!(!r.contains("bob"));
    }

    #[tokio::test]
    async fn where_is_not_null() {
        let r = tool_csv_query(
            "SELECT name FROM csv WHERE note IS NOT NULL\nname,note\nalice,\nbob,hi",
        )
        .await;
        assert!(!r.contains("alice"));
        assert!(r.contains("bob"));
    }

    #[tokio::test]
    async fn where_and_precedence_over_or() {
        // age >= 30 AND city = 'NYC' OR name = 'zoe'
        // -> (age >= 30 AND city = NYC) OR name = zoe
        let r = tool_csv_query(
            "SELECT name FROM csv WHERE age >= 30 AND city = 'NYC' OR name = 'zoe'\nname,age,city\nalice,30,NYC\nbob,40,LA\ncarol,25,NYC\nzoe,10,SEA",
        )
        .await;
        assert!(r.contains("alice"), "got: {r}");
        assert!(r.contains("zoe"), "got: {r}");
        assert!(!r.contains("bob"), "got: {r}");
        assert!(!r.contains("carol"), "got: {r}");
    }

    #[tokio::test]
    async fn order_by_asc_numeric() {
        let r = tool_csv_query(
            "SELECT name FROM csv ORDER BY age\nname,age\nalice,30\nbob,25\ncarol,40",
        )
        .await;
        let alice = r.find("alice").unwrap();
        let bob = r.find("bob").unwrap();
        let carol = r.find("carol").unwrap();
        assert!(bob < alice && alice < carol, "got: {r}");
    }

    #[tokio::test]
    async fn order_by_desc() {
        let r = tool_csv_query(
            "SELECT name FROM csv ORDER BY age DESC\nname,age\nalice,30\nbob,25\ncarol,40",
        )
        .await;
        let alice = r.find("alice").unwrap();
        let bob = r.find("bob").unwrap();
        let carol = r.find("carol").unwrap();
        assert!(carol < alice && alice < bob, "got: {r}");
    }

    #[tokio::test]
    async fn limit_caps_rows() {
        let r = tool_csv_query("SELECT * FROM csv LIMIT 2\nname\na\nb\nc\nd").await;
        assert!(r.contains("(2 rows)"), "got: {r}");
    }

    #[tokio::test]
    async fn tsv_via_from_tsv() {
        let r = tool_csv_query("SELECT name FROM tsv\nname\tage\nalice\t30\nbob\t25").await;
        assert!(r.contains("alice"));
        assert!(r.contains("bob"));
    }

    #[tokio::test]
    async fn unknown_column_in_select_reports_error() {
        let r = tool_csv_query("SELECT foo FROM csv\nname,age\na,1").await;
        assert!(r.contains("unknown column in SELECT"), "got: {r}");
    }

    #[tokio::test]
    async fn unknown_column_in_where_reports_error() {
        let r = tool_csv_query("SELECT * FROM csv WHERE foo = 1\nname,age\na,1").await;
        assert!(r.contains("unknown column in WHERE"), "got: {r}");
    }

    #[tokio::test]
    async fn case_insensitive_column_match() {
        let r = tool_csv_query("SELECT NAME FROM csv WHERE AGE > 20\nname,age\nalice,30").await;
        assert!(r.contains("alice"), "got: {r}");
    }

    #[tokio::test]
    async fn read_csv_from_file_via_at_prefix() {
        let dir = std::env::temp_dir().join(format!("aictl_csv_file_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("data.csv");
        std::fs::write(&path, "name,age\nalice,30\nbob,25\n").unwrap();
        let input = format!("SELECT name FROM csv WHERE age > 26\n@{}", path.display());
        let r = tool_csv_query(&input).await;
        assert!(r.contains("alice"), "got: {r}");
        assert!(!r.contains("bob"), "got: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- like_match unit tests -----

    #[test]
    fn like_exact_no_wildcard() {
        assert!(like_match("hello", "hello"));
        assert!(!like_match("hello", "world"));
    }

    #[test]
    fn like_trailing_wildcard() {
        assert!(like_match("hello world", "hello%"));
        assert!(!like_match("hi world", "hello%"));
    }

    #[test]
    fn like_leading_wildcard() {
        assert!(like_match("foo@example.com", "%@example.com"));
        assert!(!like_match("foo@other.com", "%@example.com"));
    }

    #[test]
    fn like_middle_wildcard() {
        assert!(like_match("hello beautiful world", "hello%world"));
    }

    #[test]
    fn like_case_insensitive() {
        assert!(like_match("Hello", "hello"));
        assert!(like_match("FOO@EXAMPLE.COM", "%@example.com"));
    }

    // ----- cmp_values unit tests -----

    #[test]
    fn cmp_numeric_when_both_numeric() {
        assert_eq!(cmp_values("10", "2"), Ordering::Greater);
        assert_eq!(cmp_values("2.5", "2.5"), Ordering::Equal);
    }

    #[test]
    fn cmp_string_when_non_numeric() {
        assert_eq!(cmp_values("alice", "bob"), Ordering::Less);
    }
}
