use std::net::{IpAddr, SocketAddr};

use burrow_admin_proto::status::{ClientStatus, OFFLINE, ONLINE, Status, away};

use crate::wording::THIS_SERVER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Here,
    Online,
    Offline,
    Unknown,
}

impl Presence {
    pub fn up(self) -> bool {
        matches!(self, Presence::Here | Presence::Online)
    }

    pub fn word(self) -> &'static str {
        match self {
            Presence::Here => THIS_SERVER,
            Presence::Online => ONLINE,
            Presence::Offline => OFFLINE,
            Presence::Unknown => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientInfo {
    pub name: String,
    pub online: bool,
    pub local: bool,
    pub from: Option<String>,
}

pub fn client<'a>(status: &'a Status, name: &str) -> Option<&'a ClientStatus> {
    status.clients.iter().find(|c| c.name == name)
}

pub fn info(c: &ClientStatus) -> ClientInfo {
    ClientInfo {
        name: c.name.clone(),
        online: c.connected || c.local,
        local: c.local,
        from: c.from.as_deref().map(from_ip),
    }
}

pub fn of(status: Option<&Status>, name: &str) -> Option<ClientInfo> {
    status.and_then(|s| client(s, name)).map(info)
}

pub fn presence(status: Option<&Status>, reserved: &[String], name: &str) -> Presence {
    if reserved.iter().any(|r| r == name) {
        return Presence::Here;
    }
    match status {
        None => Presence::Unknown,
        Some(s) => match client(s, name) {
            Some(c) if c.local => Presence::Here,
            Some(c) if c.connected => Presence::Online,
            _ => Presence::Offline,
        },
    }
}

pub fn online(status: Option<&Status>, reserved: &[String], name: &str) -> bool {
    presence(status, reserved, name).up()
}

pub fn from_ip(from: &str) -> String {
    let from = from.trim();
    if let Ok(a) = from.parse::<SocketAddr>() {
        return a.ip().to_canonical().to_string();
    }
    let bare = from.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>()
        .map_or_else(|_| from.to_string(), |ip| ip.to_canonical().to_string())
}

pub fn detail(status: Option<&Status>, reserved: &[String], name: &str) -> String {
    match (
        presence(status, reserved, name),
        status.and_then(|s| client(s, name)),
    ) {
        (Presence::Online, Some(c)) => c.from.as_deref().map_or_else(
            || ONLINE.to_string(),
            |f| format!("{ONLINE} · {}", from_ip(f)),
        ),
        (p, _) => p.word().to_string(),
    }
}

pub fn away_text(status: &Status, name: &str, added_secs_ago: Option<u64>) -> String {
    let left = client(status, name).and_then(|c| c.left_secs_ago);
    away(left, status.uptime_secs, added_secs_ago)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Status {
        Status {
            uptime_secs: 900,
            clients: vec![
                ClientStatus {
                    name: "home".into(),
                    connected: true,
                    from: Some("[2001:db8::1]:51000".into()),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "office".into(),
                    left_secs_ago: Some(120),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "self".into(),
                    connected: true,
                    local: true,
                    ..ClientStatus::default()
                },
            ],
            ..Status::default()
        }
    }

    #[test]
    fn this_server_is_always_up_and_a_client_is_up_while_connected() {
        let st = status();
        let reserved = vec!["self".to_string()];
        assert_eq!(presence(Some(&st), &reserved, "self"), Presence::Here);
        assert_eq!(presence(None, &reserved, "self"), Presence::Here);
        assert_eq!(presence(Some(&st), &[], "self"), Presence::Here);
        assert_eq!(presence(Some(&st), &reserved, "home"), Presence::Online);
        assert_eq!(presence(Some(&st), &reserved, "office"), Presence::Offline);
        assert_eq!(presence(Some(&st), &reserved, "gone"), Presence::Offline);
        assert_eq!(presence(None, &reserved, "home"), Presence::Unknown);
        assert!(online(Some(&st), &reserved, "self") && !online(None, &[], "home"));
    }

    #[test]
    fn an_address_reads_as_its_ip_the_same_for_ipv4_and_ipv6() {
        assert_eq!(from_ip("203.0.113.7:51000"), "203.0.113.7");
        assert_eq!(from_ip("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(from_ip("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(from_ip("2001:db8::1"), "2001:db8::1");
        assert_eq!(from_ip("203.0.113.7"), "203.0.113.7");
        assert_eq!(from_ip("[::ffff:203.0.113.7]:9"), "203.0.113.7");
        assert_eq!(from_ip("unix socket"), "unix socket");
    }

    #[test]
    fn a_detail_says_where_from_or_since_when() {
        let st = status();
        let reserved = vec!["self".to_string()];
        assert_eq!(detail(Some(&st), &reserved, "home"), "online · 2001:db8::1");
        assert_eq!(detail(Some(&st), &reserved, "office"), "offline");
        assert_eq!(away_text(&st, "office", None), "left 2m ago");
        assert_eq!(detail(Some(&st), &reserved, "self"), "This server");
        assert_eq!(detail(None, &reserved, "home"), "");
        let home = of(Some(&st), "home").unwrap();
        assert!(home.online && !home.local);
        assert_eq!(home.from.as_deref(), Some("2001:db8::1"));
    }
}
