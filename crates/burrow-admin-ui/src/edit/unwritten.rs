#[derive(Clone, Debug, PartialEq)]
pub struct Unwritten {
    pub name: String,
    pub shown: String,
    pub own: String,
    pub text: String,
}

impl Unwritten {
    pub fn reason(&self) -> String {
        format!(
            "{} cannot be written as {} yet; fix the text first",
            self.name, self.own
        )
    }
}

fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn leave_title(all: &[Unwritten]) -> String {
    match all {
        [one] => format!("Leave and lose the {} text?", one.shown),
        _ => "Leave and lose text that cannot be saved yet?".into(),
    }
}

pub fn leave_question(all: &[Unwritten]) -> String {
    match all {
        [one] => format!(
            "{} has {} text that cannot be saved as {} yet.",
            one.name, one.shown, one.own
        ),
        _ => format!(
            "{} hold text that cannot be saved in their own language yet.",
            listed(
                &all.iter()
                    .map(|u| format!("{} ({})", u.name, u.shown))
                    .collect::<Vec<_>>()
            )
        ),
    }
}

pub fn lost_note(all: &[Unwritten]) -> Option<String> {
    match all {
        [] => None,
        [one] => Some(format!(
            "The server changed; the {} text in {} that could not be saved as {} yet is gone",
            one.shown, one.name, one.own
        )),
        _ => Some(format!(
            "The server changed; text that could not be saved yet is gone in {}",
            listed(&all.iter().map(|u| u.name.clone()).collect::<Vec<_>>())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(name: &str, shown: &str) -> Unwritten {
        Unwritten {
            name: name.into(),
            shown: shown.into(),
            own: "YAML".into(),
            text: String::new(),
        }
    }

    #[test]
    fn every_file_holding_such_text_is_named() {
        let one = [u("server.yaml", "TOML")];
        assert_eq!(leave_title(&one), "Leave and lose the TOML text?");
        assert_eq!(
            leave_question(&one),
            "server.yaml has TOML text that cannot be saved as YAML yet."
        );
        let two = [u("server.yaml", "TOML"), u("server.d/a.yaml", "JSON")];
        assert_eq!(
            leave_question(&two),
            "server.yaml (TOML) and server.d/a.yaml (JSON) hold text that cannot be saved in their own language yet."
        );
        assert_eq!(
            lost_note(&two).as_deref(),
            Some(
                "The server changed; text that could not be saved yet is gone in server.yaml and server.d/a.yaml"
            )
        );
        assert_eq!(lost_note(&[]), None);
        assert_eq!(
            one[0].reason(),
            "server.yaml cannot be written as YAML yet; fix the text first"
        );
    }
}
