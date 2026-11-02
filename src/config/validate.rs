use std::path::Path;

use crate::base::failure::Failure;
use crate::base::style::Style;
use crate::config::diagnostics::{Diagnostic, Report};
use crate::config::env::EnvSource;
use crate::config::{Loaded, Outcome, Role, load_file};

pub struct Validation {
    pub outcome: Outcome,
    pub exit: u8,
}

pub fn validate(
    path: &Path,
    role: Option<Role>,
    strict: bool,
    env: &dyn EnvSource,
) -> Result<Validation, Failure> {
    let outcome = load_file(path, role, env).map_err(|e| {
        let (m, h) = e.describe(path);
        Failure::usage(m).hint(h)
    })?;
    let failed = outcome.report.has_errors() || (strict && outcome.report.warning_count() > 0);
    Ok(Validation {
        exit: u8::from(failed),
        outcome,
    })
}

pub fn summary_line(outcome: &Outcome, strict: bool) -> String {
    let r = &outcome.report;
    let w = r.warning_count();
    let warn = if w > 0 {
        format!(", {w} {}", crate::config::diagnostics::plural(w, "warning"))
    } else {
        String::new()
    };
    match &outcome.loaded {
        Some(Loaded::Server(s, _)) if !(strict && w > 0) => {
            let visitors = match s.visitors.len() {
                0 => String::new(),
                n => format!(", {n} {}", crate::config::diagnostics::plural(n, "visitor")),
            };
            format!(
                "{}: valid server config ({} {}{visitors}, {} {}){warn}",
                r.file(),
                s.clients.len(),
                crate::config::diagnostics::plural(s.clients.len(), "client"),
                s.services.len(),
                crate::config::diagnostics::plural(s.services.len(), "service"),
            )
        }
        Some(Loaded::Client(s, _)) if !(strict && w > 0) => {
            format!("{}: valid client config for \"{}\"{warn}", r.file(), s.name)
        }
        Some(Loaded::Visitor(s, _)) if !(strict && w > 0) => format!(
            "{}: valid visitor config for \"{}\" ({} {}){warn}",
            r.file(),
            s.name,
            s.forwards.len(),
            crate::config::diagnostics::plural(s.forwards.len(), "forward"),
        ),
        _ if strict && !r.has_errors() => format!(
            "{}: {w} {} (failing because of --strict)",
            r.file(),
            crate::config::diagnostics::plural(w, "warning")
        ),
        _ => r.summary(),
    }
}

fn web_summary(outcome: &Outcome) -> Option<crate::config::web_summary::WebSummary> {
    match &outcome.loaded {
        #[cfg(feature = "web")]
        Some(Loaded::Server(s, _)) => {
            crate::config::web_summary::summary(s).map(|w| crate::config::web_summary::WebSummary {
                account: crate::config::web_summary::account_url(s),
                ..w
            })
        }
        #[cfg(not(feature = "web"))]
        Some(Loaded::Server(s, _)) => crate::config::web_summary::summary(s),
        _ => None,
    }
}

struct FileLine {
    shown: String,
    read_only: Option<&'static str>,
    sections: Vec<(String, Vec<String>)>,
}

fn file_lines(outcome: &Outcome) -> Vec<FileLine> {
    if outcome.files.len() < 2 {
        return Vec::new();
    }
    let main = outcome.files[0].name.as_str();
    outcome
        .files
        .iter()
        .map(|f| {
            let path = std::path::Path::new(&f.name);
            let mut sections: Vec<(String, Vec<String>)> = Vec::new();
            for (section, name) in &f.items {
                match sections.iter_mut().find(|(s, _)| s == section) {
                    Some((_, names)) => names.push(name.clone()),
                    None => sections.push((section.clone(), vec![name.clone()])),
                }
            }
            FileLine {
                shown: crate::config::load::short_name(main, &f.name),
                read_only: crate::config::docs::read_only(path).map(|r| r.why()),
                sections,
            }
        })
        .collect()
}

fn files_text(outcome: &Outcome) -> String {
    let lines = file_lines(outcome);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("read {} files:\n", lines.len());
    for l in lines {
        let what = if l.sections.is_empty() {
            "nothing".to_string()
        } else {
            l.sections
                .iter()
                .map(|(s, names)| format!("{s} {}", names.join(", ")))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let lock = l
            .read_only
            .map(|why| format!(" (read-only: {why})"))
            .unwrap_or_default();
        out.push_str(&format!("  {}{lock}: {what}\n", l.shown));
    }
    out
}

fn files_json(outcome: &Outcome) -> Option<serde_json::Value> {
    let lines = file_lines(outcome);
    (!lines.is_empty()).then(|| {
        lines
            .into_iter()
            .map(|l| {
                let mut v = serde_json::json!({
                    "file": l.shown,
                    "read_only": l.read_only,
                });
                for (s, names) in l.sections {
                    v[s] = names.into();
                }
                v
            })
            .collect()
    })
}

pub fn render_text(outcome: &Outcome, strict: bool, style: Style) -> String {
    let mut s = outcome.report.render_text(style);
    s.push_str(&files_text(outcome));
    s.push_str(&summary_line(outcome, strict));
    s.push('\n');
    if let Some(web) = web_summary(outcome) {
        s.push_str(&crate::config::web_summary::text(&web));
    }
    s
}

pub fn render_json(outcome: &Outcome, strict: bool) -> String {
    let mut v = outcome.report.render_json();
    v["role"] = outcome.role.map(|r| r.as_str()).into();
    v["strict"] = strict.into();
    v["valid"] =
        (!outcome.report.has_errors() && !(strict && outcome.report.warning_count() > 0)).into();
    if let Some(web) = web_summary(outcome) {
        v["web"] = serde_json::to_value(web).expect("json serializes");
    }
    if let Some(files) = files_json(outcome) {
        v["files"] = files;
    }
    let mut s = serde_json::to_string_pretty(&v).expect("json serializes");
    s.push('\n');
    s
}

pub fn unreadable_json(path: &Path, f: &Failure, strict: bool) -> String {
    let mut report = crate::config::diagnostics::Report::new(path.display().to_string(), "");
    let mut d = Diagnostic::error("read", f.message.clone()).without_snippet();
    if let Some(h) = &f.hint {
        d = d.help(h.clone());
    }
    report.push(d);
    let mut v = report.render_json();
    v["role"] = serde_json::Value::Null;
    v["strict"] = strict.into();
    let mut s = serde_json::to_string_pretty(&v).expect("json serializes");
    s.push('\n');
    s
}

pub fn startup_report(report: &Report, style: Style) -> String {
    let warnings: Vec<&Diagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| !d.is_error())
        .collect();
    if warnings.is_empty() {
        return String::new();
    }
    let mut r = report.clone();
    r.diagnostics = warnings.into_iter().cloned().collect();
    r.render_text(style)
}
