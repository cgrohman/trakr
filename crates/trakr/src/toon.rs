//! Minimal hand-rolled TOON (Token-Oriented Object Notation) output for the
//! CLI's known, small response shapes. See https://toonformat.dev/.

use std::fmt::Write as _;

fn quote_if_needed(v: &str) -> String {
    if v.is_empty() || v.contains(',') || v.contains('\n') || v.contains('"') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// `name:\n  key: value\n  ...`
pub fn record(name: &str, pairs: &[(&str, String)]) -> String {
    let mut out = format!("{name}:\n");
    for (k, v) in pairs {
        let _ = writeln!(out, "  {k}: {v}");
    }
    out
}

/// `name[N]{f1,f2,...}:\n  v1,v2,...`
pub fn table(name: &str, fields: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = format!("{name}[{}]{{{}}}:\n", rows.len(), fields.join(","));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| quote_if_needed(c)).collect();
        let _ = writeln!(out, "  {}", cells.join(","));
    }
    out
}

/// `help[N]:\n  line1\n  line2`
pub fn help(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("help[{}]:\n", lines.len());
    for l in lines {
        let _ = writeln!(out, "  {l}");
    }
    out
}

/// `error: message\nhelp: suggestion`
pub fn error(message: &str, help: &str) -> String {
    format!("error: {message}\nhelp: {help}\n")
}
