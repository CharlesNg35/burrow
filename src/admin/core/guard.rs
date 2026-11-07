//! Finds admin changes that must be refused or confirmed, such as ones that end the signed-in visitor's session.
//! Start at `refusals` and `lockouts`; the child modules check web ports, reach and the WebSocket listener.

mod allowed;
mod effects;
mod elsewhere;
mod reach;
mod web_ports;
mod websocket;

use std::net::SocketAddr;

use burrow_admin_proto::codes::{
    LAST_WRITER, SELF_DEMOTED, SELF_REMOVED, SELF_SECRET, SELF_SOURCE, SELF_TRANSPORT,
};
use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::plan::{Impact, Level, Refusal};
use burrow_config::ServerConfig;

use crate::config::settings::{ServerSettings, Shares};
use crate::wire::secret::Secret;

pub use allowed::not_allowed;
pub use effects::impacts;
pub use elsewhere::not_here;
pub use web_ports::{busy as busy_web_ports, shared as shared_web_port};

pub struct Who<'a> {
    pub visitor: &'a str,
    pub peer: SocketAddr,
    pub secret: &'a Secret,
    pub via_websocket: bool,
}

fn refusal(code: &str, message: String) -> Refusal {
    Refusal {
        code: code.into(),
        message,
    }
}

pub fn refusals(who: &Who, new: &ServerSettings) -> Vec<Refusal> {
    let me = who.visitor;
    if !new.visitors.contains_key(me) {
        return vec![refusal(
            SELF_REMOVED,
            format!(
                "this change removes visitor \"{me}\", which you are signed in as; you would lose access"
            ),
        )];
    }
    if new
        .visitors
        .values()
        .any(|v| v.admin == Some(AdminLevel::Write))
    {
        return Vec::new();
    }
    vec![refusal(
        LAST_WRITER,
        "no visitor would keep write access, so nobody could change the config from burrow admin"
            .into(),
    )]
}

fn danger(code: &str, message: String, confirm: String) -> Impact {
    Impact {
        level: Level::Danger,
        code: code.into(),
        message,
        confirm: Some(confirm),
    }
}

pub fn lockouts(who: &Who, old: &ServerSettings, new: &ServerSettings) -> Vec<Impact> {
    let me = who.visitor;
    let mut out = Vec::new();
    if let Some(v) = new.visitors.get(me) {
        if v.admin == Some(AdminLevel::Read) {
            out.push(danger(
                SELF_DEMOTED,
                format!("takes write access away from visitor \"{me}\", which you are signed in as; after this you can only look"),
                me.to_string(),
            ));
        } else if v.admin.is_none() {
            out.push(danger(
                SELF_DEMOTED,
                format!("takes admin access away from visitor \"{me}\", which you are signed in as; this session ends"),
                me.to_string(),
            ));
        }
        if !v.secrets.contains(who.secret) {
            out.push(danger(
                SELF_SECRET,
                format!(
                    "removes secret {}, which this session signed in with; this session ends, and you sign in again with another secret of visitor \"{me}\"",
                    who.secret.fingerprint()
                ),
                me.to_string(),
            ));
        }
        if v.admin.is_some() && !v.admits_admin(who.peer.ip()) {
            out.push(danger(
                SELF_SOURCE,
                format!(
                    "admin_from of visitor \"{me}\" no longer admits your address {}; this session ends, and burrow admin works only from the new addresses",
                    who.peer.ip()
                ),
                me.to_string(),
            ));
        }
    }
    out.extend(transport(who, old, new));
    out
}

fn transport(who: &Who, old: &ServerSettings, new: &ServerSettings) -> Option<Impact> {
    if who.via_websocket {
        let kept = matches!((&old.websocket, &new.websocket), (Some(a), Some(b))
            if a.path == b.path
                && a.listen == b.listen
                && (a.shares == Shares::Nothing || a.host == b.host));
        return (!kept).then(|| {
            danger(
                SELF_TRANSPORT,
                "this session comes through the WebSocket listener, which this change turns off or moves; this session ends, and you connect again at the new address".into(),
                ServerConfig::WEBSOCKET.key().into(),
            )
        });
    }
    (new.listen.is_none() && old.listen.is_some()).then(|| {
        danger(
            SELF_TRANSPORT,
            "this session comes through the TCP listener, which this change removes; this session ends when the server restarts".into(),
            ServerConfig::LISTEN.key().into(),
        )
    })
}

#[cfg(test)]
mod tests;
