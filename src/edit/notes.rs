use crate::config::diagnostics::{Diagnostic, Severity};

const MARK: &str = "# burrow edit:";

pub fn annotate(text: &str, problems: &[Diagnostic]) -> String {
    let errors: Vec<&Diagnostic> = problems
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let header = 2 + errors
        .iter()
        .map(|d| 1 + usize::from(d.help.is_some()))
        .sum::<usize>();
    let mut out = format!("{MARK} not saved, because of the problems below.\n");
    for d in errors {
        let at = d
            .span
            .map(|s| format!("line {}: ", s.start.line + header))
            .unwrap_or_default();
        out.push_str(&format!("{MARK}   {at}{}\n", one_line(&d.message)));
        if let Some(help) = &d.help {
            out.push_str(&format!("{MARK}     help: {}\n", one_line(help)));
        }
    }
    out.push_str(&format!(
        "{MARK} fix them and save to try again, or close without saving to cancel.\n"
    ));
    out.push_str(text);
    out
}

pub fn strip(text: &str) -> String {
    let mut rest = text;
    while rest.starts_with(MARK) {
        rest = rest.split_once('\n').map_or("", |(_, after)| after);
    }
    rest.to_string()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Format;

    #[test]
    fn problems_go_on_top_and_come_off_again() {
        let text = "server: example.com\nnmae: home\n";
        let nothing = |_: &str| None;
        let checked = crate::config::load_str(
            "client.yaml",
            text,
            Format::Yaml,
            Some(crate::config::Role::Client),
            &nothing,
        );
        let noted = annotate(text, &checked.report.diagnostics);
        assert!(noted.starts_with(MARK));
        let line = noted
            .lines()
            .filter(|l| l.contains("nmae"))
            .find_map(|l| {
                l.split("line ")
                    .nth(1)?
                    .split(':')
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .unwrap();
        assert_eq!(noted.lines().nth(line - 1), Some("nmae: home"), "{noted}");
        assert!(
            noted
                .lines()
                .filter(|l| !l.starts_with(MARK))
                .eq(text.lines())
        );
        assert_eq!(strip(&noted), text);
        assert_eq!(strip(text), text);
        assert_eq!(
            strip("# my own comment\nname: home\n"),
            "# my own comment\nname: home\n"
        );
    }
}
