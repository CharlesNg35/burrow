use crate::diagnostics::{Diagnostic, Severity};

const MARK: &str = "# burrow edit:";

pub fn annotate(text: &str, problems: &[Diagnostic]) -> String {
    let mut out = format!("{MARK} not saved, because of the problems below.\n");
    for d in problems.iter().filter(|d| d.severity == Severity::Error) {
        let at = d
            .span
            .map(|s| format!("line {}: ", s.start.line))
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
        assert!(noted.contains("line 2: "), "{noted}");
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
