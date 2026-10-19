use std::fmt::Write as _;

use serde::Serialize;

use crate::config::tree::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
    pub label: Option<String>,
    pub help: Option<String>,
    pub snippet: bool,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code,
            message: message.into(),
            span: None,
            label: None,
            help: None,
            snippet: true,
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            ..Diagnostic::error(code, message)
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn without_snippet(mut self) -> Self {
        self.snippet = false;
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    pub fn detect(stream_is_tty: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let color = stream_is_tty && !no_color;
        #[cfg(windows)]
        let color = color && crossterm::ansi_support::supports_ansi();
        Style { color }
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub file: String,
    pub source: String,
    pub masked: Vec<Span>,
    pub mask_tokens: bool,
    pub diagnostics: Vec<Diagnostic>,
}

const TOKEN_MIN: usize = 24;

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

fn mask_long_tokens(line: &mut [char]) {
    let mut i = 0;
    while i < line.len() {
        if !token_char(line[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < line.len() && token_char(line[i]) {
            i += 1;
        }
        if i - start >= TOKEN_MIN {
            line[start..i].iter_mut().for_each(|c| *c = '*');
        }
    }
}

impl Report {
    pub fn new(file: impl Into<String>, source: impl Into<String>) -> Self {
        Report {
            file: file.into(),
            source: source.into(),
            masked: Vec::new(),
            mask_tokens: false,
            diagnostics: Vec::new(),
        }
    }

    pub fn push(&mut self, d: Diagnostic) {
        if !self.diagnostics.contains(&d) {
            self.diagnostics.push(d);
        }
    }

    pub fn sort(&mut self) {
        self.diagnostics.sort_by_key(|d| {
            (
                d.span.map_or(0, |s| s.start.offset + 1),
                d.severity,
                d.span.map_or(0, |s| s.start.col),
            )
        });
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics.len() - self.error_count()
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    pub fn render_text(&self, style: Style) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            self.render_one(&mut out, d, style);
            out.push('\n');
        }
        out
    }

    pub fn summary(&self) -> String {
        let (e, w) = (self.error_count(), self.warning_count());
        format!(
            "{}: {} {}, {} {}",
            self.file,
            e,
            plural(e, "error"),
            w,
            plural(w, "warning")
        )
    }

    fn render_one(&self, out: &mut String, d: &Diagnostic, style: Style) {
        let head = match d.severity {
            Severity::Error => style.paint("1;31", "error"),
            Severity::Warning => style.paint("1;33", "warning"),
        };
        let _ = writeln!(
            out,
            "{head}{} {}",
            style.paint("1", ":"),
            style.paint("1", &d.message)
        );
        let line_no = d.span.map_or(0, |s| s.start.line);
        let gutter = line_no.to_string().len().max(2);
        let pad = " ".repeat(gutter);
        let bar = style.paint("1;34", "|");
        match d.span.filter(|s| s.start.line > 0) {
            Some(span) => {
                let _ = writeln!(
                    out,
                    "{pad}{} {}:{}:{}",
                    style.paint("1;34", "-->"),
                    self.file,
                    span.start.line,
                    span.start.col
                );
                if let Some(line) = d.snippet.then(|| self.source_line(span)).flatten() {
                    let width = caret_width(&line, span);
                    let indent = line.chars().take(span.start.col - 1).collect::<String>();
                    let indent: String = indent
                        .chars()
                        .map(|c| if c == '\t' { '\t' } else { ' ' })
                        .collect();
                    let carets = style.paint(
                        if d.is_error() { "1;31" } else { "1;33" },
                        &"^".repeat(width),
                    );
                    let label = d
                        .label
                        .as_deref()
                        .map(|l| format!(" {l}"))
                        .unwrap_or_default();
                    let _ = writeln!(out, "{pad} {bar}");
                    let num = style.paint("1;34", &format!("{line_no:>gutter$}"));
                    let _ = writeln!(out, "{num} {bar} {line}");
                    let _ = writeln!(out, "{pad} {bar} {indent}{carets}{label}");
                }
            }
            None => {
                let _ = writeln!(out, "{pad}{} {}", style.paint("1;34", "-->"), self.file);
            }
        }
        if let Some(help) = &d.help {
            let _ = writeln!(out, "{pad} {} {help}", style.paint("1;34", "= help:"));
        }
    }

    fn source_line(&self, span: Span) -> Option<String> {
        if span.start.line == 0 {
            return None;
        }
        let mut at = span.start.offset.min(self.source.len());
        while !self.source.is_char_boundary(at) {
            at -= 1;
        }
        let start = self.source[..at].rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let end = self.source[start..]
            .find(['\n', '\r'])
            .map_or(self.source.len(), |i| start + i);
        let mut line: Vec<char> = self.source[start..end]
            .trim_end_matches('\r')
            .chars()
            .collect();
        for m in &self.masked {
            if m.start.line > span.start.line || m.end.line < span.start.line {
                continue;
            }
            let from = if m.start.line == span.start.line {
                m.start.col - 1
            } else {
                0
            };
            let to = if m.end.line == span.start.line {
                m.end.col - 1
            } else {
                line.len()
            };
            let to = to.min(line.len());
            for c in line.iter_mut().take(to).skip(from) {
                *c = '*';
            }
        }
        if self.mask_tokens {
            mask_long_tokens(&mut line);
        }
        Some(line.into_iter().collect())
    }

    pub fn render_json(&self) -> serde_json::Value {
        let items: Vec<_> = self
            .diagnostics
            .iter()
            .map(|d| {
                serde_json::json!({
                    "severity": d.severity,
                    "code": d.code,
                    "message": d.message,
                    "file": self.file,
                    "line": d.span.map(|s| s.start.line),
                    "column": d.span.map(|s| s.start.col),
                    "end_line": d.span.map(|s| s.end.line),
                    "end_column": d.span.map(|s| s.end.col),
                    "label": d.label,
                    "help": d.help,
                })
            })
            .collect();
        serde_json::json!({
            "file": self.file,
            "valid": !self.has_errors(),
            "errors": self.error_count(),
            "warnings": self.warning_count(),
            "diagnostics": items,
        })
    }
}

fn caret_width(line: &str, span: Span) -> usize {
    let len = line.chars().count();
    let start = span.start.col.saturating_sub(1);
    let end = if span.end.line != span.start.line {
        len
    } else if span.end.col > span.start.col {
        span.end.col - 1
    } else {
        start + 1
    };
    end.min(len).saturating_sub(start).max(1)
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tree::Pos;

    fn span(line: usize, col: usize, end_col: usize, offset: usize) -> Span {
        Span {
            start: Pos { offset, line, col },
            end: Pos {
                offset: offset + end_col - col,
                line,
                col: end_col,
            },
        }
    }

    #[test]
    fn renders_caret_under_span() {
        let src = "a: 1\nservices:\n  web:\n    client: hom\n";
        let mut r = Report::new("server.yaml", src);
        r.push(
            Diagnostic::error("unknown-client", "unknown client \"hom\"")
                .at(span(4, 13, 16, 31))
                .label("no client with this name")
                .help("did you mean \"home\"?"),
        );
        let text = r.render_text(Style { color: false });
        assert_eq!(
            text,
            "error: unknown client \"hom\"\n  --> server.yaml:4:13\n   |\n 4 |     client: hom\n   |             ^^^ no client with this name\n   = help: did you mean \"home\"?\n\n"
        );
    }

    #[test]
    fn masks_secret_spans() {
        let src = "{\"secret\": \"abcd\", \"bnd\": 1}";
        let mut r = Report::new("c.json", src);
        r.masked.push(span(1, 12, 18, 11));
        r.push(Diagnostic::error("x", "unknown field").at(span(1, 20, 25, 19)));
        let text = r.render_text(Style { color: false });
        assert!(!text.contains("abcd"));
        assert!(text.contains("******"));
    }
}
