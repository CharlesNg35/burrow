use super::{
    ChannelKind, DecodeError, Reader, TOKEN_LEN, proto_byte, proto_from, put_str, put_strings,
};
use burrow_config::Protocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hello {
    Control {
        version: u8,
        instance: [u8; TOKEN_LEN],
    },
    Data {
        version: u8,
        token: [u8; TOKEN_LEN],
    },
    Visit {
        version: u8,
    },
}

impl Hello {
    pub fn kind(&self) -> ChannelKind {
        match self {
            Hello::Control { .. } => ChannelKind::Control,
            Hello::Data { .. } => ChannelKind::Data,
            Hello::Visit { .. } => ChannelKind::Visit,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Hello::Control { version, instance } => {
                let mut v = vec![1, *version];
                v.extend_from_slice(instance);
                v
            }
            Hello::Data { version, token } => {
                let mut v = vec![2, *version];
                v.extend_from_slice(token);
                v
            }
            Hello::Visit { version } => vec![3, *version],
        }
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        let h = match r.u8()? {
            1 => Hello::Control {
                version: r.u8()?,
                instance: r.token()?,
            },
            2 => Hello::Data {
                version: r.u8()?,
                token: r.token()?,
            },
            3 => Hello::Visit { version: r.u8()? },
            x => return Err(DecodeError::UnknownType(x)),
        };
        r.end()?;
        Ok(h)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub protocol: Protocol,
    pub bind_port: u16,
    pub target: String,
    pub ports: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    Welcome {
        token: [u8; TOKEN_LEN],
        heartbeat_secs: u16,
    },
    Services(Vec<ServiceInfo>),
    Ping,
    OpenData {
        count: u16,
    },
    Bye {
        reason: ByeReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByeReason {
    Shutdown = 1,
    Replaced = 2,
    Removed = 3,
    Timeout = 4,
    PlaintextMismatch = 5,
    Duplicate = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Pong,
    Allow(Vec<String>),
}

pub const MAX_ALLOW_RULES: usize = 64;

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ServerMsg::Welcome {
                token,
                heartbeat_secs,
            } => {
                let mut v = vec![1];
                v.extend_from_slice(token);
                v.extend_from_slice(&heartbeat_secs.to_be_bytes());
                v
            }
            ServerMsg::Services(list) => {
                let mut v = vec![2];
                v.extend_from_slice(&(list.len().min(u16::MAX as usize) as u16).to_be_bytes());
                for s in list.iter().take(u16::MAX as usize) {
                    put_str(&mut v, &s.name);
                    v.push(proto_byte(s.protocol));
                    v.extend_from_slice(&s.bind_port.to_be_bytes());
                    put_str(&mut v, &s.target);
                    v.extend_from_slice(&s.ports.max(1).to_be_bytes());
                }
                v
            }
            ServerMsg::Ping => vec![3],
            ServerMsg::OpenData { count } => {
                let mut v = vec![4];
                v.extend_from_slice(&count.to_be_bytes());
                v
            }
            ServerMsg::Bye { reason } => vec![5, *reason as u8],
        }
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        let m = match r.u8()? {
            1 => ServerMsg::Welcome {
                token: r.token()?,
                heartbeat_secs: r.u16()?,
            },
            2 => {
                let n = r.u16()? as usize;
                let mut list = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    let name = r.short_str()?;
                    let protocol = proto_from(r.u8()?)?;
                    let bind_port = r.u16()?;
                    let target = r.short_str()?;
                    let ports = r.u16()?.max(1);
                    list.push(ServiceInfo {
                        name,
                        protocol,
                        bind_port,
                        target,
                        ports,
                    });
                }
                ServerMsg::Services(list)
            }
            3 => ServerMsg::Ping,
            4 => ServerMsg::OpenData { count: r.u16()? },
            5 => ServerMsg::Bye {
                reason: match r.u8()? {
                    1 => ByeReason::Shutdown,
                    2 => ByeReason::Replaced,
                    3 => ByeReason::Removed,
                    4 => ByeReason::Timeout,
                    5 => ByeReason::PlaintextMismatch,
                    6 => ByeReason::Duplicate,
                    x => return Err(DecodeError::UnknownType(x)),
                },
            },
            x => return Err(DecodeError::UnknownType(x)),
        };
        r.end()?;
        Ok(m)
    }
}

impl ClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ClientMsg::Pong => vec![0x81],
            ClientMsg::Allow(rules) => {
                let mut v = vec![0x82];
                put_strings(&mut v, rules);
                v
            }
        }
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        let m = match r.u8()? {
            0x81 => ClientMsg::Pong,
            0x82 => ClientMsg::Allow(r.strings()?),
            x => return Err(DecodeError::UnknownType(x)),
        };
        r.end()?;
        Ok(m)
    }
}
