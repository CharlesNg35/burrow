use burrow_admin_proto::updates::{
    ClientBuild, RemoteUpdates, RunMode as ShownMode, UpdateState, UpdateStatus,
};

use super::{DecodeError, Reader, put_str};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    ByHand = 0,
    Service = 1,
    Container = 2,
}

impl RunMode {
    pub fn wire(self) -> &'static str {
        match self {
            RunMode::ByHand => "by-hand",
            RunMode::Service => "service",
            RunMode::Container => "container",
        }
    }

    fn from(b: u8) -> RunMode {
        match b {
            1 => RunMode::Service,
            2 => RunMode::Container,
            _ => RunMode::ByHand,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    Off,
    On,
    Blocked(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Trying = 1,
    Done = 2,
    RolledBack = 3,
    Failed = 4,
}

impl Outcome {
    pub fn wire(self) -> &'static str {
        match self {
            Outcome::Trying => "trying",
            Outcome::Done => "done",
            Outcome::RolledBack => "rolled-back",
            Outcome::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        [
            Outcome::Trying,
            Outcome::Done,
            Outcome::RolledBack,
            Outcome::Failed,
        ]
        .into_iter()
        .find(|o| o.wire() == s)
    }

    fn from(b: u8) -> Result<Outcome, DecodeError> {
        match b {
            1 => Ok(Outcome::Trying),
            2 => Ok(Outcome::Done),
            3 => Ok(Outcome::RolledBack),
            4 => Ok(Outcome::Failed),
            x => Err(DecodeError::UnknownType(x)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastUpdate {
    pub id: u32,
    pub outcome: Outcome,
    pub from: String,
    pub to: String,
    pub detail: String,
}

impl LastUpdate {
    pub fn status(&self) -> UpdateStatus {
        UpdateStatus {
            id: self.id,
            from: self.from.clone(),
            to: self.to.clone(),
            state: match self.outcome {
                Outcome::Trying => UpdateState::Restarting,
                Outcome::Done => UpdateState::Done,
                Outcome::RolledBack => UpdateState::RolledBack,
                Outcome::Failed => UpdateState::Failed,
            },
            detail: self.detail.clone(),
            ..UpdateStatus::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub run: RunMode,
    pub remote: Remote,
    pub last: Option<LastUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Accepted = 1,
    Refused = 2,
    Restarting = 3,
    Failed = 4,
}

impl Step {
    pub fn from(b: u8) -> Result<Step, DecodeError> {
        match b {
            1 => Ok(Step::Accepted),
            2 => Ok(Step::Refused),
            3 => Ok(Step::Restarting),
            4 => Ok(Step::Failed),
            x => Err(DecodeError::UnknownType(x)),
        }
    }
}

fn text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().map(|c| if c.is_control() { ' ' } else { c }) {
        if out.len() + c.len_utf8() > 255 {
            break;
        }
        out.push(c);
    }
    out
}

pub(super) fn put_text(v: &mut Vec<u8>, s: &str) {
    put_str(v, &text(s));
}

pub(super) fn read_text(r: &mut Reader) -> Result<String, DecodeError> {
    let s = r.short_str()?;
    if s.chars().any(char::is_control) {
        return Err(DecodeError::BadName);
    }
    Ok(s)
}

impl Report {
    pub fn build(&self) -> ClientBuild {
        let r = self;
        let (remote, why_not) = match &r.remote {
            Remote::Off => (RemoteUpdates::Off, None),
            Remote::On => (RemoteUpdates::On, None),
            Remote::Blocked(why) => (RemoteUpdates::Blocked, Some(why.clone())),
        };
        ClientBuild {
            version: r.version.clone(),
            os: r.os.clone(),
            arch: r.arch.clone(),
            run: match r.run {
                RunMode::ByHand => ShownMode::ByHand,
                RunMode::Service => ShownMode::Service,
                RunMode::Container => ShownMode::Container,
            },
            remote,
            why_not,
        }
    }

    pub(super) fn encode_into(&self, v: &mut Vec<u8>) {
        put_text(v, &self.version);
        put_text(v, &self.os);
        put_text(v, &self.arch);
        v.push(self.run as u8);
        let (remote, why) = match &self.remote {
            Remote::Off => (0, ""),
            Remote::On => (1, ""),
            Remote::Blocked(why) => (2, why.as_str()),
        };
        v.push(remote);
        put_text(v, why);
        v.push(u8::from(self.last.is_some()));
        if let Some(l) = &self.last {
            v.extend_from_slice(&l.id.to_be_bytes());
            v.push(l.outcome as u8);
            put_text(v, &l.from);
            put_text(v, &l.to);
            put_text(v, &l.detail);
        }
    }

    pub(super) fn decode_from(r: &mut Reader) -> Result<Report, DecodeError> {
        let version = read_text(r)?;
        let os = read_text(r)?;
        let arch = read_text(r)?;
        let run = RunMode::from(r.u8()?);
        let remote = r.u8()?;
        let why = read_text(r)?;
        let remote = match remote {
            0 => Remote::Off,
            1 => Remote::On,
            _ => Remote::Blocked(why),
        };
        let last = if r.u8()? == 0 {
            None
        } else {
            Some(LastUpdate {
                id: r.u32()?,
                outcome: Outcome::from(r.u8()?)?,
                from: read_text(r)?,
                to: read_text(r)?,
                detail: read_text(r)?,
            })
        };
        Ok(Report {
            version,
            os,
            arch,
            run,
            remote,
            last,
        })
    }
}
