use burrow_admin_logic::wording::{diag_text, help_shown};
use burrow_admin_proto::diag::{Diag, Severity};
use config_editor_leptos::{FieldPath, HostDiagnostic, TextPosition, TextRange};

pub const SOURCE: &str = "burrow";

#[derive(PartialEq)]
enum Place {
    Path(Vec<String>),
    At(u32, u32, Option<(u32, u32)>),
    Document,
}

struct Merged {
    place: Place,
    severity: Severity,
    code: String,
    messages: Vec<String>,
}

fn place_of(d: &Diag) -> Place {
    match (&d.path, d.line) {
        (Some(p), _) if !p.is_empty() => Place::Path(p.clone()),
        (_, Some(line)) if line > 0 => Place::At(
            line,
            d.col.unwrap_or(1).max(1),
            d.end_line
                .filter(|e| *e >= line)
                .map(|e| (e, d.end_col.unwrap_or(1).max(1))),
        ),
        _ => Place::Document,
    }
}

fn message_of(d: &Diag) -> String {
    let text = diag_text(d);
    match help_shown(&d.code, d.help.as_deref()) {
        Some(h) if !h.is_empty() => format!("{text} {h}"),
        _ => text,
    }
}

fn severity(s: Severity) -> config_editor_leptos::Severity {
    match s {
        Severity::Error => config_editor_leptos::Severity::Error,
        Severity::Warning => config_editor_leptos::Severity::Warning,
        Severity::Info => config_editor_leptos::Severity::Info,
    }
}

pub(super) fn field(path: &[String]) -> FieldPath {
    let pointer: String = path
        .iter()
        .map(|k| format!("/{}", k.replace('~', "~0").replace('/', "~1")))
        .collect();
    FieldPath::from_pointer(&pointer)
}

fn host(m: Merged) -> HostDiagnostic {
    let sev = severity(m.severity);
    let message = m.messages.join("; ");
    let d = match m.place {
        Place::Path(p) => HostDiagnostic::at_path(field(&p), sev, message),
        Place::At(line, col, end) => HostDiagnostic::at_range(
            TextRange::new(
                TextPosition::new(line as usize, col as usize),
                match end {
                    Some((l, c)) => TextPosition::new(l as usize, c as usize),
                    None => TextPosition::new(line as usize + 1, 1),
                },
            ),
            sev,
            message,
        ),
        Place::Document => HostDiagnostic::document(sev, message),
    };
    let d = d.with_source(SOURCE);
    if m.code.is_empty() {
        d
    } else {
        d.with_code(m.code)
    }
}

pub fn host_problems(diags: &[Diag], file: &str, main: &str) -> Vec<HostDiagnostic> {
    let mut out: Vec<Merged> = Vec::new();
    for d in diags
        .iter()
        .filter(|d| d.file.as_deref().unwrap_or(main) == file)
    {
        let place = place_of(d);
        let message = message_of(d);
        match out
            .iter_mut()
            .find(|m| m.place == place && m.severity == d.severity)
        {
            Some(m) => {
                if !m.messages.contains(&message) {
                    m.messages.push(message);
                }
                if m.code != d.code {
                    m.code.clear();
                }
            }
            None => out.push(Merged {
                place,
                severity: d.severity,
                code: d.code.clone(),
                messages: vec![message],
            }),
        }
    }
    out.into_iter().map(host).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use config_editor_leptos::DiagnosticTarget;

    fn diag(
        sev: Severity,
        code: &str,
        msg: &str,
        path: Option<&[&str]>,
        file: Option<&str>,
    ) -> Diag {
        Diag {
            severity: sev,
            code: code.into(),
            message: msg.into(),
            path: path.map(|p| p.iter().map(|s| s.to_string()).collect()),
            also: Vec::new(),
            label: None,
            help: None,
            line: None,
            col: None,
            end_line: None,
            end_col: None,
            before: false,
            subjects: Vec::new(),
            file: file.map(str::to_string),
        }
    }

    #[test]
    fn a_problem_lands_on_its_field_in_its_own_file() {
        let diags = vec![
            diag(
                Severity::Error,
                "x-port",
                "port in use",
                Some(&["services", "web", "bind"]),
                None,
            ),
            diag(
                Severity::Warning,
                "x-other",
                "elsewhere",
                Some(&["clients", "home"]),
                Some("server.d/10-web.yaml"),
            ),
        ];
        let main = host_problems(&diags, "server.yaml", "server.yaml");
        assert_eq!(main.len(), 1);
        assert_eq!(
            main[0].target,
            DiagnosticTarget::Path(FieldPath::from("/services/web/bind"))
        );
        assert_eq!(main[0].severity, config_editor_leptos::Severity::Error);
        assert_eq!(main[0].code.as_deref(), Some("x-port"));
        assert_eq!(main[0].source.as_deref(), Some(SOURCE));
        let drop_in = host_problems(&diags, "server.d/10-web.yaml", "server.yaml");
        assert_eq!(drop_in.len(), 1);
        assert_eq!(drop_in[0].severity, config_editor_leptos::Severity::Warning);
        assert!(host_problems(&diags, "server.d/other.yaml", "server.yaml").is_empty());
    }

    #[test]
    fn a_key_with_a_slash_stays_one_key() {
        let d = diag(
            Severity::Error,
            "x",
            "bad",
            Some(&["web", "paths", "/api/v1"]),
            None,
        );
        let got = host_problems(&[d], "server.yaml", "server.yaml");
        assert_eq!(
            got[0].target,
            DiagnosticTarget::Path(FieldPath::root().key("web").key("paths").key("/api/v1"))
        );
    }

    #[test]
    fn list_items_are_found_by_their_index() {
        let d = diag(
            Severity::Error,
            "x",
            "bad",
            Some(&["services", "web", "allow_from", "1"]),
            None,
        );
        let got = host_problems(&[d], "server.yaml", "server.yaml");
        assert_eq!(
            got[0].target,
            DiagnosticTarget::Path(FieldPath::from("/services/web/allow_from/1"))
        );
    }

    #[test]
    fn problems_with_the_same_file_path_and_severity_become_one() {
        let at = Some(&["visitors", "lap"][..]);
        let diags = vec![
            diag(Severity::Error, "a", "first", at, None),
            diag(Severity::Error, "b", "second", at, Some("server.yaml")),
            diag(Severity::Error, "b", "second", at, None),
            diag(Severity::Warning, "c", "a warning", at, None),
            diag(
                Severity::Error,
                "d",
                "elsewhere",
                at,
                Some("server.d/x.yaml"),
            ),
        ];
        let got = host_problems(&diags, "server.yaml", "server.yaml");
        assert_eq!(got.len(), 2, "{got:?}");
        let text = got[0].message.to_lowercase();
        assert!(text.contains("first") && text.contains("second"), "{text}");
        assert_eq!(text.matches("second").count(), 1, "the same text once");
        assert_eq!(got[0].code, None, "two codes name none");
        assert_eq!(got[1].severity, config_editor_leptos::Severity::Warning);
    }

    #[test]
    fn without_a_path_a_problem_marks_its_line_or_the_whole_file() {
        let mut lined = diag(Severity::Error, "yaml", "bad indent", None, None);
        lined.line = Some(4);
        lined.col = Some(3);
        let root = diag(Severity::Info, "i", "about the file", Some(&[]), None);
        let got = host_problems(&[lined, root], "server.yaml", "server.yaml");
        assert_eq!(
            got[0].target,
            DiagnosticTarget::Range(TextRange::new(
                TextPosition::new(4, 3),
                TextPosition::new(5, 1)
            ))
        );
        assert_eq!(got[1].target, DiagnosticTarget::Document);
        assert_eq!(got[1].severity, config_editor_leptos::Severity::Info);
        let mut spans = diag(Severity::Error, "yaml", "a block", None, None);
        spans.line = Some(2);
        spans.col = Some(1);
        spans.end_line = Some(4);
        spans.end_col = Some(7);
        let got = host_problems(&[spans], "server.yaml", "server.yaml");
        assert_eq!(
            got[0].target,
            DiagnosticTarget::Range(TextRange::new(
                TextPosition::new(2, 1),
                TextPosition::new(4, 7)
            ))
        );
    }

    #[test]
    fn the_help_follows_the_message() {
        let mut d = diag(Severity::Error, "x-thing", "the thing is wrong", None, None);
        d.help = Some("Set it to a port number.".into());
        let got = host_problems(&[d], "server.yaml", "server.yaml");
        assert!(
            got[0].message.ends_with("Set it to a port number."),
            "{}",
            got[0].message
        );
    }
}
