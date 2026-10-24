use super::{DecodeError, Reader, proto_byte, proto_from, put_str, put_strings};
use burrow_config::Protocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub protocol: Protocol,
    pub service: String,
    pub target: String,
    pub only: Option<Vec<String>>,
}

impl Connect {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![0x10, proto_byte(self.protocol)];
        put_str(&mut v, &self.service);
        put_str(&mut v, &self.target);
        match &self.only {
            None => v.push(0),
            Some(rules) => {
                v.push(1);
                put_strings(&mut v, rules);
            }
        }
        v
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        if r.u8()? != 0x10 {
            return Err(DecodeError::UnknownType(b[0]));
        }
        let c = Connect {
            protocol: proto_from(r.u8()?)?,
            service: r.short_str()?,
            target: r.short_str()?,
            only: match r.u8()? {
                0 => None,
                1 => Some(r.strings()?),
                x => return Err(DecodeError::UnknownType(x)),
            },
        };
        r.end()?;
        Ok(c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok = 0,
    Denied = 1,
    Unreachable = 2,
    Unresolved = 3,
    Invalid = 4,
    Forbidden = 5,
    Offline = 6,
    Busy = 7,
}

impl Status {
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        match b {
            [0] => Ok(Status::Ok),
            [1] => Ok(Status::Denied),
            [2] => Ok(Status::Unreachable),
            [3] => Ok(Status::Unresolved),
            [4] => Ok(Status::Invalid),
            [5] => Ok(Status::Forbidden),
            [6] => Ok(Status::Offline),
            [7] => Ok(Status::Busy),
            [x] => Err(DecodeError::UnknownType(*x)),
            [] => Err(DecodeError::Truncated),
            _ => Err(DecodeError::Trailing),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisitTarget {
    Service { name: String, index: u16 },
    Client { client: String, target: String },
}

impl VisitTarget {
    pub fn service(name: impl Into<String>) -> Self {
        VisitTarget::Service {
            name: name.into(),
            index: 0,
        }
    }

    pub fn at(&self, offset: u16) -> Self {
        match self {
            VisitTarget::Service { name, index } => VisitTarget::Service {
                name: name.clone(),
                index: index.saturating_add(offset),
            },
            VisitTarget::Client { client, target } => VisitTarget::Client {
                client: client.clone(),
                target: burrow_syntax::ports::port_at(target, offset),
            },
        }
    }
}

impl std::fmt::Display for VisitTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisitTarget::Service { name, index: 0 } => write!(f, "service \"{name}\""),
            VisitTarget::Service { name, index } => {
                write!(f, "service \"{name}\" port {}", u32::from(*index) + 1)
            }
            VisitTarget::Client { client, target } => write!(f, "{target} on client \"{client}\""),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitRequest {
    pub check: bool,
    pub protocol: Protocol,
    pub to: VisitTarget,
}

const VISIT_CHECK: u8 = 1;

const VISIT_UDP: u8 = 2;

pub(super) fn put_short(v: &mut Vec<u8>, s: &str) -> Result<(), DecodeError> {
    let b = s.as_bytes();
    let n = u8::try_from(b.len()).map_err(|_| DecodeError::TooLong)?;
    v.push(n);
    v.extend_from_slice(b);
    Ok(())
}

impl VisitRequest {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut flags = 0;
        if self.check {
            flags |= VISIT_CHECK;
        }
        if self.protocol == Protocol::Udp {
            flags |= VISIT_UDP;
        }
        let mut v = vec![0x20, flags];
        match &self.to {
            VisitTarget::Service { name, index } => {
                v.push(1);
                put_short(&mut v, name)?;
                v.extend_from_slice(&index.to_be_bytes());
            }
            VisitTarget::Client { client, target } => {
                v.push(2);
                put_short(&mut v, client)?;
                put_short(&mut v, target)?;
            }
        }
        Ok(v)
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        match r.u8()? {
            0x20 => {}
            x => return Err(DecodeError::UnknownType(x)),
        }
        let flags = r.u8()?;
        if flags & !(VISIT_CHECK | VISIT_UDP) != 0 {
            return Err(DecodeError::UnknownType(flags));
        }
        let check = flags & VISIT_CHECK != 0;
        let protocol = if flags & VISIT_UDP != 0 {
            Protocol::Udp
        } else {
            Protocol::Tcp
        };
        let name = |r: &mut Reader| {
            let n = r.short_str()?;
            if burrow_syntax::name::valid_name(&n) {
                Ok(n)
            } else {
                Err(DecodeError::BadName)
            }
        };
        let to = match r.u8()? {
            1 => VisitTarget::Service {
                name: name(&mut r)?,
                index: r.u16()?,
            },
            2 => VisitTarget::Client {
                client: name(&mut r)?,
                target: r.short_str()?,
            },
            x => return Err(DecodeError::UnknownType(x)),
        };
        r.end()?;
        Ok(VisitRequest {
            check,
            protocol,
            to,
        })
    }
}
