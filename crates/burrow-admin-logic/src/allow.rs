use burrow_admin_proto::status::ClientStatus;
use burrow_syntax::rule::ANY;

pub const SET_THERE: &str = "set on that machine";
pub const UNKNOWN: &str = "unknown";
pub const OFFLINE: &str = "not connected";
pub const ANYTHING: &str = "anything (allow_all)";

pub fn shown(feature: bool, client: Option<&ClientStatus>) -> String {
    if !feature {
        return UNKNOWN.into();
    }
    let Some(c) = client.filter(|c| c.connected) else {
        return OFFLINE.into();
    };
    match c.allow.as_deref() {
        None => UNKNOWN.into(),
        Some([one]) if one == ANY => ANYTHING.into(),
        Some(rules) => rules.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(allow: Option<Vec<&str>>) -> ClientStatus {
        ClientStatus {
            name: "home".into(),
            connected: true,
            allow: allow.map(|a| a.into_iter().map(String::from).collect()),
            ..ClientStatus::default()
        }
    }

    #[test]
    fn the_allow_line_says_what_the_client_sent_or_why_it_is_not_known() {
        let rules = on(Some(vec!["127.0.0.1:*", "unix:/run/app.sock"]));
        assert_eq!(shown(true, Some(&rules)), "127.0.0.1:*, unix:/run/app.sock");
        assert_eq!(shown(true, Some(&on(Some(vec!["any"])))), ANYTHING);
        assert_eq!(shown(true, Some(&on(None))), UNKNOWN);
        assert_eq!(shown(false, Some(&rules)), UNKNOWN, "an older server");
        let gone = ClientStatus {
            connected: false,
            ..rules
        };
        assert_eq!(shown(true, Some(&gone)), OFFLINE);
        assert_eq!(shown(true, None), OFFLINE);
    }
}
