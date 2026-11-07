use serde::{Deserialize, Serialize};

use crate::status::ClientStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    #[default]
    ByHand,
    Service,
    Container,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteUpdates {
    #[default]
    Off,
    On,
    Blocked,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClientBuild {
    pub version: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub run: RunMode,
    #[serde(default)]
    pub remote: RemoteUpdates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateState {
    #[default]
    Asked,
    Downloading,
    Restarting,
    Done,
    RolledBack,
    Failed,
    Refused,
    TimedOut,
    Waiting,
    #[serde(other)]
    Unknown,
}

impl UpdateState {
    pub fn finished(self) -> bool {
        !matches!(
            self,
            UpdateState::Asked
                | UpdateState::Downloading
                | UpdateState::Restarting
                | UpdateState::Waiting
        )
    }

    pub fn failed(self) -> bool {
        self.finished() && self != UpdateState::Done
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub id: u32,
    pub from: String,
    pub to: String,
    pub state: UpdateState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub secs_ago: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RolloutStatus {
    pub to: String,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub clients: Vec<String>,
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
    #[serde(default)]
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LatestRelease {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub checked_secs_ago: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocalClient {
    pub name: String,
    pub server: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub connected_secs: Option<u64>,
    pub build: ClientBuild,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<UpdateStatus>,
}

pub const FIRST_REPORTING: &str = "0.2.2";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateClients {
    #[serde(default)]
    pub clients: Vec<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServerUpdate {
    #[serde(default)]
    pub run: RunMode,
    #[serde(default)]
    pub remote: RemoteUpdates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<UpdateStatus>,
}

impl ServerUpdate {
    pub fn running(&self) -> bool {
        self.last.as_ref().is_some_and(|l| !l.state.finished())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateServer {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateServerReply {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub asked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    #[serde(default)]
    pub note: String,
}

pub const ROLLBACK_SECS: u64 = 60;

pub fn rollback_window() -> String {
    match ROLLBACK_SECS {
        60 => "a minute".into(),
        s => burrow_syntax::duration::span(s),
    }
}

pub fn server_update_note() -> String {
    format!(
        "The server restarts with the new release: clients, visitors and websites pause for a few seconds and reconnect on their own. If the new release does not come up healthy within {}, the old one is put back.",
        rollback_window()
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Skipped {
    pub client: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateClientsReply {
    pub to: String,
    #[serde(default)]
    pub asked: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Known(String),
    Older,
    Unknown,
}

pub fn version(c: &ClientStatus) -> Shown {
    match (&c.build, c.connected && !c.local) {
        (Some(b), _) => Shown::Known(b.version.clone()),
        (None, true) => Shown::Older,
        (None, false) => Shown::Unknown,
    }
}

pub fn version_text(c: &ClientStatus) -> String {
    match version(c) {
        Shown::Known(v) => v,
        Shown::Older => format!("older than {FIRST_REPORTING}"),
        Shown::Unknown => String::new(),
    }
}

pub fn run_text(b: &ClientBuild) -> &'static str {
    match b.run {
        RunMode::Service => "service",
        RunMode::Container => "container",
        RunMode::ByHand => "started by hand",
        RunMode::Unknown => "unknown",
    }
}

pub fn platform_text(b: &ClientBuild) -> String {
    match (b.os.is_empty(), b.arch.is_empty()) {
        (false, false) => format!("{}/{}", b.os, b.arch),
        (false, true) => b.os.clone(),
        _ => b.arch.clone(),
    }
}

pub fn remote_text(b: &ClientBuild) -> String {
    match b.remote {
        RemoteUpdates::On => "allowed".into(),
        RemoteUpdates::Off => "off in its config".into(),
        RemoteUpdates::Blocked => match &b.why_not {
            Some(why) => format!("on, but {why}"),
            None => "on, but not possible here".into(),
        },
        RemoteUpdates::Unknown => "unknown".into(),
    }
}

pub fn hand_hint() -> String {
    format!(
        "update it by hand once (sudo burrow update); from {FIRST_REPORTING} on it can be updated from here"
    )
}

pub fn job_text(u: &UpdateStatus) -> String {
    let mut line = format!("{} → {}: {}", u.from, u.to, state_text(u.state));
    if !u.detail.is_empty() && u.state.finished() {
        line.push_str(&format!(": {}", u.detail));
    }
    line
}

pub fn state_text(state: UpdateState) -> &'static str {
    match state {
        UpdateState::Asked => "asked",
        UpdateState::Waiting => "waiting its turn",
        UpdateState::Downloading => "downloading and checking",
        UpdateState::Restarting => "restarting",
        UpdateState::Done => "updated",
        UpdateState::RolledBack => "rolled back",
        UpdateState::Failed => "failed",
        UpdateState::Refused => "refused",
        UpdateState::TimedOut => "did not come back",
        UpdateState::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_job_reads_as_one_line_everywhere() {
        let mut u = UpdateStatus {
            from: "0.2.2".into(),
            to: "0.2.3".into(),
            state: UpdateState::Downloading,
            detail: "step 2".into(),
            ..UpdateStatus::default()
        };
        assert_eq!(job_text(&u), "0.2.2 → 0.2.3: downloading and checking");
        u.state = UpdateState::RolledBack;
        u.detail = "did not connect within a minute".into();
        assert_eq!(
            job_text(&u),
            "0.2.2 → 0.2.3: rolled back: did not connect within a minute"
        );
        u.state = UpdateState::Done;
        u.detail.clear();
        assert_eq!(job_text(&u), "0.2.2 → 0.2.3: updated");
    }

    #[test]
    fn a_connected_client_without_a_report_is_older_than_the_first_reporting_release() {
        let old = ClientStatus {
            name: "nas".into(),
            connected: true,
            ..ClientStatus::default()
        };
        assert_eq!(version(&old), Shown::Older);
        assert_eq!(version_text(&old), "older than 0.2.2");
        let gone = ClientStatus {
            connected: false,
            ..old.clone()
        };
        assert_eq!(version(&gone), Shown::Unknown);
        let new = ClientStatus {
            build: Some(ClientBuild {
                version: "0.2.1".into(),
                os: "linux".into(),
                arch: "arm64".into(),
                run: RunMode::Service,
                remote: RemoteUpdates::Blocked,
                why_not: Some("the service needs installing again".into()),
            }),
            ..gone
        };
        assert_eq!(version_text(&new), "0.2.1");
        let b = new.build.as_ref().unwrap();
        assert_eq!(platform_text(b), "linux/arm64");
        assert_eq!(run_text(b), "service");
        assert_eq!(remote_text(b), "on, but the service needs installing again");
    }

    #[test]
    fn unknown_words_from_a_newer_peer_read_as_unknown() {
        let b: ClientBuild =
            serde_json::from_str(r#"{"version":"0.3.0","run":"pod","remote":"later"}"#).unwrap();
        assert_eq!(
            (b.run, b.remote),
            (RunMode::Unknown, RemoteUpdates::Unknown)
        );
        let u: UpdateStatus =
            serde_json::from_str(r#"{"id":1,"from":"a","to":"b","state":"paused"}"#).unwrap();
        assert_eq!(u.state, UpdateState::Unknown);
        assert!(u.state.finished());
    }
}
