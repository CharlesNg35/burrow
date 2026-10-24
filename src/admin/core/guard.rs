mod effects;
mod grants;

use std::net::SocketAddr;
use std::path::Path;

use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::plan::Refusal;

use crate::config::patch::Rename;
use crate::config::settings::ServerSettings;
use crate::wire::secret::Secret;

pub use effects::impacts;

pub struct Who<'a> {
    pub visitor: &'a str,
    pub peer: SocketAddr,
    pub secret: &'a Secret,
    pub via_websocket: bool,
}

pub enum Base<'a> {
    Valid(&'a ServerSettings),
    Broken {
        raw: &'a serde_json::Value,
        next_raw: &'a serde_json::Value,
    },
}

pub struct Change<'a> {
    pub running: &'a ServerSettings,
    pub base: Base<'a>,
    pub next: &'a ServerSettings,
    pub renames: &'a [Rename],
    pub path: &'a Path,
}

fn refusal(code: &str, message: String) -> Refusal {
    Refusal {
        code: code.into(),
        message,
    }
}

pub fn refusals(who: &Who, change: &Change) -> Vec<Refusal> {
    let (old, new) = (change.running, change.next);
    let mut out = Vec::new();
    let me = who.visitor;
    match new.visitors.get(me) {
        None => out.push(refusal(
            "self-removed",
            format!("this change removes visitor \"{me}\", which you are signed in as; you would lose access"),
        )),
        Some(v) => {
            if v.admin != Some(AdminLevel::Write) {
                out.push(refusal(
                    "self-demoted",
                    format!("this change takes write access away from visitor \"{me}\", which you are signed in as"),
                ));
            }
            if !v.secrets.contains(who.secret) {
                out.push(refusal(
                    "self-secret",
                    format!(
                        "this change removes secret {}, which this session signed in with",
                        who.secret.fingerprint()
                    ),
                ));
            }
            if v.admin.is_some() && !v.admits_admin(who.peer.ip()) {
                out.push(refusal(
                    "self-source",
                    format!(
                        "admin_from of visitor \"{me}\" would no longer admit your address {}",
                        who.peer.ip()
                    ),
                ));
            }
        }
    }
    if !new
        .visitors
        .values()
        .any(|v| v.admin == Some(AdminLevel::Write))
        && out
            .iter()
            .all(|r| r.code != "self-removed" && r.code != "self-demoted")
    {
        out.push(refusal(
            "last-writer",
            "no visitor would keep write access, so nobody could change the config from burrow admin".into(),
        ));
    }
    if who.via_websocket {
        let kept =
            matches!((&old.websocket, &new.websocket), (Some(a), Some(b)) if a.path == b.path);
        if !kept {
            out.push(refusal(
                "self-transport",
                "this session comes through the WebSocket listener, and this change removes it or changes its path".into(),
            ));
        }
    } else if new.listen.is_none() && old.listen.is_some() {
        out.push(refusal(
            "self-transport",
            "this session comes through the TCP listener, which this change removes".into(),
        ));
    }
    out.extend(grants::refusals(change));
    out
}

#[cfg(test)]
mod tests;
