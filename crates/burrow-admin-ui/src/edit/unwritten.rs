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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_says_which_file_waits_to_be_written_back() {
        let u = Unwritten {
            name: "server.yaml".into(),
            shown: "TOML".into(),
            own: "YAML".into(),
            text: String::new(),
        };
        assert_eq!(
            u.reason(),
            "server.yaml cannot be written as YAML yet; fix the text first"
        );
    }
}
