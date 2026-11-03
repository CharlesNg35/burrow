use burrow_admin_proto::updates::{UpdateState, UpdateStatus, rollback_window};

pub fn plain_reason(detail: &str) -> String {
    let d = detail.to_ascii_lowercase();
    let window = rollback_window();
    if d.contains("did not connect") {
        format!("didn't connect to the server within {window}")
    } else if d.contains("sha-256") || d.contains("checksum") {
        "the download didn't match its published checksum".to_string()
    } else if d.contains("does not accept") {
        "it didn't accept the config".to_string()
    } else if d.contains("installed without remote updates") {
        "remote updates aren't turned on for it".to_string()
    } else if d.contains("did not allow remote updates") {
        "another service shares its program and didn't allow updates".to_string()
    } else if d.contains("not newer") {
        "it already runs that release or a newer one".to_string()
    } else if d.contains("cannot download") || d.contains("could not") || d.contains("cannot reach")
    {
        "the release couldn't be downloaded".to_string()
    } else if d.contains("stopped") && d.contains("times") {
        "it kept stopping right after it started".to_string()
    } else if d.contains("did not start")
        || d.contains("did not answer")
        || d.contains("is not running")
        || d.contains("does not listen")
        || d.contains("reports ")
    {
        format!("didn't become healthy within {window}")
    } else {
        "the update didn't finish".to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    pub title: String,
    pub sentence: String,
    pub raw: String,
    pub to: String,
}

pub fn trouble(u: &UpdateStatus, who: Option<&str>) -> Option<Trouble> {
    if !u.state.failed() {
        return None;
    }
    let plain = plain_reason(&u.detail);
    let (title, sentence) = match (u.state, who) {
        (UpdateState::RolledBack, None) => (
            format!("Update to {} rolled back", u.to),
            format!(
                "{} started but {plain}, so {} was put back. The server is running normally.",
                u.to, u.from
            ),
        ),
        (UpdateState::RolledBack, Some(c)) => (
            format!("Update of {c} to {} rolled back", u.to),
            format!(
                "{} started but {plain}, so {c} went back to {}. It is running normally.",
                u.to, u.from
            ),
        ),
        (_, None) => (
            format!("Update to {} failed", target(u)),
            format!(
                "Nothing changed: {plain}. The server still runs {}.",
                u.from
            ),
        ),
        (_, Some(c)) => (
            format!("Update of {c} to {} failed", target(u)),
            format!("Nothing changed: {plain}. {c} still runs {}.", u.from),
        ),
    };
    Some(Trouble {
        title,
        sentence,
        raw: u.detail.clone(),
        to: u.to.clone(),
    })
}

fn target(u: &UpdateStatus) -> &str {
    if u.to.is_empty() {
        "the latest release"
    } else {
        &u.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn technical_reasons_read_as_plain_words() {
        let cases = [
            (
                "0.2.3 did not answer on its local status endpoint within 20 s; restored 0.2.2, and it runs again",
                "didn't become healthy within a minute",
            ),
            (
                "burrow-hub did not start with 0.2.3; restored 0.2.2, and it runs again",
                "didn't become healthy within a minute",
            ),
            (
                "0.2.3 did not connect to the server within 60 s",
                "didn't connect to the server within a minute",
            ),
            (
                "burrow-0.2.3-linux-amd64.tar.gz does not match its SHA-256 in SHA256SUMS",
                "the download didn't match its published checksum",
            ),
            (
                "0.2.3 does not accept /etc/burrow/x/client.yaml: error: x; nothing was changed",
                "it didn't accept the config",
            ),
            (
                "the service was installed without remote updates; to allow them, ...",
                "remote updates aren't turned on for it",
            ),
            (
                "burrow-nas also run /usr/local/bin/burrow and did not allow remote updates; ...",
                "another service shares its program and didn't allow updates",
            ),
            ("something new", "the update didn't finish"),
        ];
        for (raw, want) in cases {
            assert_eq!(plain_reason(raw), want, "{raw}");
        }
    }

    #[test]
    fn a_roll_back_and_a_failure_read_as_a_title_and_a_calm_sentence() {
        let mut u = UpdateStatus {
            from: "0.2.2".into(),
            to: "0.2.3".into(),
            state: UpdateState::RolledBack,
            detail: "0.2.3 did not answer on its local status endpoint within 20 s; restored 0.2.2"
                .into(),
            ..UpdateStatus::default()
        };
        let t = trouble(&u, None).unwrap();
        assert_eq!(t.title, "Update to 0.2.3 rolled back");
        assert_eq!(
            t.sentence,
            "0.2.3 started but didn't become healthy within a minute, so 0.2.2 was put back. The server is running normally."
        );
        assert!(
            !t.sentence.contains("endpoint"),
            "the raw reason stays in Details"
        );
        assert!(t.raw.contains("endpoint"));
        let c = trouble(&u, Some("home")).unwrap();
        assert_eq!(c.title, "Update of home to 0.2.3 rolled back");
        u.state = UpdateState::Failed;
        u.detail = "does not match its SHA-256".into();
        assert!(
            trouble(&u, None)
                .unwrap()
                .sentence
                .starts_with("Nothing changed: the download")
        );
        u.state = UpdateState::Done;
        assert_eq!(trouble(&u, None), None);
    }
}
