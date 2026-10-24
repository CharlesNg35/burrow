use burrow_config::{Cipher, Protocol};
use burrow_syntax::name::MAX_NAME_LEN;

pub const MAGIC: [u8; 4] = *b"BRW1";
pub const VERSION: u8 = 1;
pub const PREAMBLE_FIXED: usize = 7;
pub const TOKEN_LEN: usize = 16;
pub const KEEPALIVE_RECORD: u8 = 0x00;
pub const STALE_RECORD: u8 = 0x01;

pub const FLAG_AES_GCM: u8 = 0b01;
pub const FLAG_PLAINTEXT: u8 = 0b10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Control = 1,
    Data = 2,
    Visit = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preamble {
    pub kind: ChannelKind,
    pub cipher: Cipher,
    pub plaintext: bool,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    BadMagic,
    BadVersion,
    BadKind,
    BadFlags,
    BadName,
    Truncated,
    Trailing,
    UnknownType(u8),
    TooLong,
}

impl Preamble {
    pub fn flags(&self) -> u8 {
        let mut f = 0;
        if self.cipher == Cipher::AesGcm {
            f |= FLAG_AES_GCM;
        }
        if self.plaintext {
            f |= FLAG_PLAINTEXT;
        }
        f
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(PREAMBLE_FIXED + self.name.len());
        v.extend_from_slice(&MAGIC);
        v.push(self.kind as u8);
        v.push(self.flags());
        v.push(self.name.len() as u8);
        v.extend_from_slice(self.name.as_bytes());
        v
    }

    pub fn decode_fixed(
        head: &[u8; PREAMBLE_FIXED],
    ) -> Result<(ChannelKind, u8, usize), DecodeError> {
        if head[..4] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let kind = match head[4] {
            1 => ChannelKind::Control,
            2 => ChannelKind::Data,
            3 => ChannelKind::Visit,
            _ => return Err(DecodeError::BadKind),
        };
        let flags = head[5];
        if flags & !(FLAG_AES_GCM | FLAG_PLAINTEXT) != 0 {
            return Err(DecodeError::BadFlags);
        }
        let len = head[6] as usize;
        if len == 0 || len > MAX_NAME_LEN {
            return Err(DecodeError::BadName);
        }
        Ok((kind, flags, len))
    }

    pub fn from_parts(kind: ChannelKind, flags: u8, name: &[u8]) -> Result<Self, DecodeError> {
        let name = std::str::from_utf8(name).map_err(|_| DecodeError::BadName)?;
        if !burrow_syntax::name::valid_name(name) {
            return Err(DecodeError::BadName);
        }
        Ok(Preamble {
            kind,
            cipher: if flags & FLAG_AES_GCM != 0 {
                Cipher::AesGcm
            } else {
                Cipher::Chachapoly
            },
            plaintext: flags & FLAG_PLAINTEXT != 0,
            name: name.to_string(),
        })
    }

    pub fn prologue(&self) -> Vec<u8> {
        let mut p = b"burrow/1\0".to_vec();
        p.extend_from_slice(&self.encode());
        p
    }
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, DecodeError> {
        let (&b, rest) = self.buf.split_first().ok_or(DecodeError::Truncated)?;
        self.buf = rest;
        Ok(b)
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        Ok(a)
    }

    fn short_str(&mut self) -> Result<String, DecodeError> {
        let n = self.u8()? as usize;
        let b = self.bytes(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError::BadName)
    }

    fn token(&mut self) -> Result<[u8; TOKEN_LEN], DecodeError> {
        let mut t = [0u8; TOKEN_LEN];
        t.copy_from_slice(self.bytes(TOKEN_LEN)?);
        Ok(t)
    }

    fn strings(&mut self) -> Result<Vec<String>, DecodeError> {
        let n = self.u8()? as usize;
        if n > MAX_ALLOW_RULES {
            return Err(DecodeError::TooLong);
        }
        (0..n)
            .map(|_| {
                let s = self.short_str()?;
                if s.chars().any(char::is_control) {
                    Err(DecodeError::BadName)
                } else {
                    Ok(s)
                }
            })
            .collect()
    }

    fn name(&mut self) -> Result<String, DecodeError> {
        let n = self.short_str()?;
        if burrow_syntax::name::valid_name(&n) {
            Ok(n)
        } else {
            Err(DecodeError::BadName)
        }
    }

    fn end(&self) -> Result<(), DecodeError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::Trailing)
        }
    }
}

fn put_str(v: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    debug_assert!(b.len() <= 255, "config checks keep wire strings short");
    let n = b.len().min(255);
    v.push(n as u8);
    v.extend_from_slice(&b[..n]);
}

fn proto_byte(p: Protocol) -> u8 {
    match p {
        Protocol::Tcp => 0,
        Protocol::Udp => 1,
    }
}

fn proto_from(b: u8) -> Result<Protocol, DecodeError> {
    match b {
        0 => Ok(Protocol::Tcp),
        1 => Ok(Protocol::Udp),
        x => Err(DecodeError::UnknownType(x)),
    }
}

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

fn put_strings(v: &mut Vec<u8>, items: &[String]) {
    let fitting: Vec<&String> = items
        .iter()
        .filter(|s| s.len() <= u8::MAX as usize)
        .take(MAX_ALLOW_RULES)
        .collect();
    v.push(fitting.len() as u8);
    for s in fitting {
        v.push(s.len() as u8);
        v.extend_from_slice(s.as_bytes());
    }
}

pub const GRANTS_REQUEST: [u8; 1] = [0x21];
pub const GRANTS_REQUEST_V2: [u8; 2] = [0x21, 0x01];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub services: Vec<GrantedService>,
    pub clients: Vec<GrantedClient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedService {
    pub name: String,
    pub protocol: Protocol,
    pub port: u16,
    pub ports: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedClient {
    pub name: String,
    pub reach: Vec<String>,
    pub allow: Option<Vec<String>>,
}

impl Grants {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut tcp = self.clone();
        tcp.services.retain(|s| s.protocol == Protocol::Tcp);
        tcp.fitting(false)
    }

    pub fn encode_v2(&self) -> Result<Vec<u8>, DecodeError> {
        self.fitting(true)
    }

    fn fitting(&self, v2: bool) -> Result<Vec<u8>, DecodeError> {
        let full = self.encode_all(v2)?;
        if full.len() <= crate::wire::noise::MAX_PLAINTEXT {
            return Ok(full);
        }
        let mut lean = self.clone();
        for c in &mut lean.clients {
            c.allow = None;
        }
        let lean = lean.encode_all(v2)?;
        if lean.len() <= crate::wire::noise::MAX_PLAINTEXT {
            return Ok(lean);
        }
        Err(DecodeError::TooLong)
    }

    fn encode_all(&self, v2: bool) -> Result<Vec<u8>, DecodeError> {
        let count = |n: usize| u16::try_from(n).map_err(|_| DecodeError::TooLong);
        let mut v = vec![if v2 { 0x25 } else { 0x22 }];
        v.extend_from_slice(&count(self.services.len())?.to_be_bytes());
        for s in &self.services {
            put_short(&mut v, &s.name)?;
            if v2 {
                v.push(proto_byte(s.protocol));
            }
            v.extend_from_slice(&s.port.to_be_bytes());
            v.extend_from_slice(&s.ports.to_be_bytes());
        }
        v.extend_from_slice(&count(self.clients.len())?.to_be_bytes());
        for c in &self.clients {
            put_short(&mut v, &c.name)?;
            put_strings(&mut v, &c.reach);
            match &c.allow {
                None => v.push(0),
                Some(rules) => {
                    v.push(1);
                    put_strings(&mut v, rules);
                }
            }
        }
        Ok(v)
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        let v2 = match r.u8()? {
            0x22 => false,
            0x25 => true,
            x => return Err(DecodeError::UnknownType(x)),
        };
        let services = (0..r.u16()?)
            .map(|_| {
                let name = r.name()?;
                let protocol = if v2 {
                    proto_from(r.u8()?)?
                } else {
                    Protocol::Tcp
                };
                let (port, ports) = (r.u16()?, r.u16()?);
                if ports == 0
                    || ports > burrow_syntax::ports::MAX_RANGE
                    || u32::from(port) + u32::from(ports) > 65536
                {
                    return Err(DecodeError::TooLong);
                }
                Ok(GrantedService {
                    name,
                    protocol,
                    port,
                    ports,
                })
            })
            .collect::<Result<Vec<_>, DecodeError>>()?;
        let clients = (0..r.u16()?)
            .map(|_| {
                let name = r.name()?;
                let reach = r.strings()?;
                let allow = match r.u8()? {
                    0 => None,
                    1 => Some(r.strings()?),
                    x => return Err(DecodeError::UnknownType(x)),
                };
                Ok(GrantedClient { name, reach, allow })
            })
            .collect::<Result<Vec<_>, DecodeError>>()?;
        r.end()?;
        Ok(Grants { services, clients })
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

fn put_short(v: &mut Vec<u8>, s: &str) -> Result<(), DecodeError> {
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

pub const MAX_DATAGRAM: usize = crate::wire::noise::MAX_PLAINTEXT - 4;

pub fn encode_datagram(peer: u32, payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&peer.to_be_bytes());
    out.extend_from_slice(payload);
}

pub fn decode_datagram(b: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
    let mut r = Reader { buf: b };
    let peer = r.u32()?;
    Ok((peer, r.buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datagram_with_its_peer_id_fills_one_record_at_most() {
        assert_eq!(MAX_DATAGRAM, 65515);
        let mut rec = Vec::new();
        encode_datagram(7, &vec![1u8; MAX_DATAGRAM], &mut rec);
        assert_eq!(rec.len(), crate::wire::noise::MAX_PLAINTEXT);
        let (id, payload) = decode_datagram(&rec).unwrap();
        assert_eq!((id, payload.len()), (7, MAX_DATAGRAM));
    }

    #[test]
    fn a_grants_answer_rejects_what_cannot_be_true() {
        let one = |port: u16, ports: u16| Grants {
            services: vec![GrantedService {
                name: "db".into(),
                protocol: Protocol::Tcp,
                port,
                ports,
            }],
            clients: vec![],
        };
        assert!(Grants::decode(&one(5432, 1).encode().unwrap()).is_ok());
        assert!(Grants::decode(&one(65535, 1).encode().unwrap()).is_ok());
        assert!(Grants::decode(&one(5432, 0).encode().unwrap()).is_err());
        assert!(Grants::decode(&one(65000, 1000).encode().unwrap()).is_err());
        assert!(Grants::decode(&one(1, 1001).encode().unwrap()).is_err());
        assert!(Grants::decode(&one(64536, 1000).encode().unwrap()).is_ok());
        let evil = ClientMsg::Allow(vec!["unix:/x\n  evil:".into()]).encode();
        assert!(ClientMsg::decode(&evil).is_err());
    }

    #[test]
    fn a_large_answer_drops_allow_lists_first() {
        let rule = "10.0.0.0/8:1-65535".repeat(10);
        let clients: Vec<GrantedClient> = (0..60)
            .map(|i| GrantedClient {
                name: format!("client-{i}"),
                reach: vec![],
                allow: Some(vec![rule.clone(); MAX_ALLOW_RULES]),
            })
            .collect();
        let big = Grants {
            services: vec![],
            clients,
        };
        let sent = Grants::decode(&big.encode().unwrap()).unwrap();
        assert_eq!(sent.clients.len(), 60);
        assert!(sent.clients.iter().all(|c| c.allow.is_none()));
        let huge = Grants {
            services: (0..2000)
                .map(|i| GrantedService {
                    name: format!("service-with-a-rather-long-name-{i:04}"),
                    protocol: Protocol::Tcp,
                    port: 1,
                    ports: 1,
                })
                .collect(),
            clients: vec![],
        };
        assert!(huge.encode().is_err());
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn grants_travel_with_known_and_unknown_allow_lists() {
        let grants = Grants {
            services: vec![GrantedService {
                name: "db".into(),
                protocol: Protocol::Tcp,
                port: 5432,
                ports: 1,
            }],
            clients: vec![
                GrantedClient {
                    name: "home".into(),
                    reach: vec![],
                    allow: Some(vec!["127.0.0.0/8".into()]),
                },
                GrantedClient {
                    name: "office".into(),
                    reach: vec!["10.0.0.0/8".into()],
                    allow: None,
                },
            ],
        };
        let bytes = grants.encode().unwrap();
        assert_eq!(
            bytes,
            unhex(
                "22000102646215380001000204686f6d650001010b3132372e302e302e302f38066f6666696365010a31302e302e302e302f3800"
            )
        );
        assert_eq!(Grants::decode(&bytes).unwrap(), grants);
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Grants::decode(&trailing).is_err());
        let bad = Grants {
            services: vec![GrantedService {
                name: "no spaces".into(),
                protocol: Protocol::Tcp,
                port: 1,
                ports: 1,
            }],
            clients: vec![],
        };
        assert!(Grants::decode(&bad.encode().unwrap()).is_err());
    }

    #[test]
    fn an_allow_list_is_capped_on_the_wire() {
        let many: Vec<String> = (0..100).map(|i| format!("10.0.{i}.0/24")).collect();
        let sent = ClientMsg::Allow(many.clone()).encode();
        let ClientMsg::Allow(got) = ClientMsg::decode(&sent).unwrap() else {
            panic!("an allow message")
        };
        assert_eq!(got, many[..MAX_ALLOW_RULES].to_vec());
        let mut too_many = vec![0x82, (MAX_ALLOW_RULES + 1) as u8];
        for _ in 0..=MAX_ALLOW_RULES {
            too_many.extend_from_slice(&[1, b'x']);
        }
        assert!(ClientMsg::decode(&too_many).is_err());
        assert_eq!(
            ClientMsg::decode(&ClientMsg::Pong.encode()).unwrap(),
            ClientMsg::Pong
        );
    }

    #[test]
    fn the_wire_format_stays_as_documented() {
        let services = ServerMsg::Services(vec![
            ServiceInfo {
                name: "web".into(),
                protocol: Protocol::Tcp,
                bind_port: 443,
                target: "127.0.0.1:8443".into(),
                ports: 1,
            },
            ServiceInfo {
                name: "game".into(),
                protocol: Protocol::Udp,
                bind_port: 30000,
                target: "192.168.1.50:30000".into(),
                ports: 11,
            },
            ServiceInfo {
                name: "db".into(),
                protocol: Protocol::Tcp,
                bind_port: 0,
                target: "unix:/run/my-db.sock".into(),
                ports: 1,
            },
        ]);
        let server = [
            (
                ServerMsg::Welcome {
                    token: [9; 16],
                    heartbeat_secs: 10,
                },
                "0109090909090909090909090909090909000a",
            ),
            (
                services,
                "020003037765620001bb0e3132372e302e302e313a3834343300010467616d65017530123139322e3136382e312e35303a3330303030000b02646200000014756e69783a2f72756e2f6d792d64622e736f636b0001",
            ),
            (ServerMsg::Ping, "03"),
            (ServerMsg::OpenData { count: 7 }, "040007"),
            (
                ServerMsg::Bye {
                    reason: ByeReason::Replaced,
                },
                "0502",
            ),
        ];
        for (msg, wire) in server {
            assert_eq!(msg.encode(), unhex(wire), "{msg:?}");
            assert_eq!(ServerMsg::decode(&unhex(wire)).unwrap(), msg);
        }
        assert_eq!(ClientMsg::decode(&unhex("81")).unwrap(), ClientMsg::Pong);
        let preambles = [
            (
                Preamble {
                    kind: ChannelKind::Control,
                    cipher: Cipher::Chachapoly,
                    plaintext: false,
                    name: "home".into(),
                },
                "42525731010004686f6d65",
            ),
            (
                Preamble {
                    kind: ChannelKind::Data,
                    cipher: Cipher::AesGcm,
                    plaintext: true,
                    name: "home".into(),
                },
                "42525731020304686f6d65",
            ),
        ];
        for (p, wire) in preambles {
            let b = unhex(wire);
            assert_eq!(p.encode(), b);
            let head: [u8; PREAMBLE_FIXED] = b[..PREAMBLE_FIXED].try_into().unwrap();
            let (kind, flags, _) = Preamble::decode_fixed(&head).unwrap();
            assert_eq!(
                Preamble::from_parts(kind, flags, &b[PREAMBLE_FIXED..]).unwrap(),
                p
            );
        }
        for (hello, wire) in [
            (
                Hello::Control {
                    version: 1,
                    instance: [9; 16],
                },
                "010109090909090909090909090909090909",
            ),
            (
                Hello::Data {
                    version: 1,
                    token: [7; 16],
                },
                "020107070707070707070707070707070707",
            ),
        ] {
            assert_eq!(hello.encode(), unhex(wire));
            assert_eq!(Hello::decode(&unhex(wire)).unwrap(), hello);
        }
        let connect = Connect {
            protocol: Protocol::Udp,
            service: "dns".into(),
            target: "192.168.1.1:53".into(),
            only: None,
        };
        let wire = unhex("100103646e730e3139322e3136382e312e313a353300");
        assert_eq!(connect.encode(), wire);
        assert_eq!(Connect::decode(&wire).unwrap(), connect);
        for (status, byte) in [
            (Status::Ok, 0),
            (Status::Denied, 1),
            (Status::Unreachable, 2),
            (Status::Unresolved, 3),
            (Status::Invalid, 4),
        ] {
            assert_eq!(Status::decode(&[byte]).unwrap(), status);
        }
        let mut out = Vec::new();
        encode_datagram(5, b"hi", &mut out);
        assert_eq!(out, unhex("000000056869"));
    }

    #[test]
    fn fields_too_long_for_the_wire_are_an_error() {
        let too_long = VisitRequest {
            check: false,
            protocol: Protocol::Tcp,
            to: VisitTarget::Client {
                client: "home".into(),
                target: format!("{}:8080", "a".repeat(252)),
            },
        };
        assert_eq!(too_long.encode(), Err(DecodeError::TooLong));
    }

    #[test]
    fn ports_of_a_range_are_shown_from_one() {
        assert_eq!(VisitTarget::service("game").to_string(), "service \"game\"");
        let second = VisitTarget::Service {
            name: "game".into(),
            index: 1,
        };
        assert_eq!(second.to_string(), "service \"game\" port 2");
    }

    #[test]
    fn preamble_round_trip() {
        let p = Preamble {
            kind: ChannelKind::Data,
            cipher: Cipher::AesGcm,
            plaintext: true,
            name: "home".into(),
        };
        let b = p.encode();
        let head: [u8; PREAMBLE_FIXED] = b[..PREAMBLE_FIXED].try_into().unwrap();
        let (kind, flags, len) = Preamble::decode_fixed(&head).unwrap();
        assert_eq!(len, 4);
        assert_eq!(
            Preamble::from_parts(kind, flags, &b[PREAMBLE_FIXED..]).unwrap(),
            p
        );
    }

    #[test]
    fn preamble_rejects_garbage() {
        assert_eq!(
            Preamble::decode_fixed(b"GET / H"),
            Err(DecodeError::BadMagic)
        );
        assert_eq!(
            Preamble::decode_fixed(b"BRW1\x04\x00\x01"),
            Err(DecodeError::BadKind)
        );
        assert_eq!(
            Preamble::decode_fixed(b"BRW1\x01\x04\x01"),
            Err(DecodeError::BadFlags)
        );
        assert_eq!(
            Preamble::decode_fixed(b"BRW1\x01\x00\x00"),
            Err(DecodeError::BadName)
        );
        assert_eq!(
            Preamble::decode_fixed(b"BRW1\x01\x00\x41"),
            Err(DecodeError::BadName)
        );
        assert_eq!(
            Preamble::from_parts(ChannelKind::Control, 0, b"a b"),
            Err(DecodeError::BadName)
        );
    }

    #[test]
    fn messages_round_trip() {
        let msgs = [
            ServerMsg::Welcome {
                token: [9; 16],
                heartbeat_secs: 10,
            },
            ServerMsg::Services(vec![ServiceInfo {
                name: "web".into(),
                protocol: Protocol::Udp,
                bind_port: 4000,
                target: "127.0.0.1:3000".into(),
                ports: 11,
            }]),
            ServerMsg::Ping,
            ServerMsg::OpenData { count: 7 },
            ServerMsg::Bye {
                reason: ByeReason::Replaced,
            },
        ];
        for m in msgs {
            assert_eq!(ServerMsg::decode(&m.encode()).unwrap(), m);
        }
        assert_eq!(
            ClientMsg::decode(&ClientMsg::Pong.encode()).unwrap(),
            ClientMsg::Pong
        );
        let c = Connect {
            protocol: Protocol::Tcp,
            service: "web".into(),
            target: "db.lan:5432".into(),
            only: Some(vec!["192.168.150.0/25".into(), "unix:/run/app.sock".into()]),
        };
        assert_eq!(Connect::decode(&c.encode()).unwrap(), c);
        let none = Connect {
            only: None,
            ..c.clone()
        };
        assert_eq!(Connect::decode(&none.encode()).unwrap(), none);
        let empty = Connect {
            only: Some(vec![]),
            ..c.clone()
        };
        assert_eq!(Connect::decode(&empty.encode()).unwrap().only, Some(vec![]));
        let h = Hello::Data {
            version: 1,
            token: [3; 16],
        };
        assert_eq!(Hello::decode(&h.encode()).unwrap(), h);
        let v = Hello::Visit { version: 1 };
        assert_eq!(Hello::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn visit_requests_round_trip() {
        for r in [
            VisitRequest {
                check: false,
                protocol: Protocol::Tcp,
                to: VisitTarget::service("db"),
            },
            VisitRequest {
                check: true,
                protocol: Protocol::Tcp,
                to: VisitTarget::Client {
                    client: "home".into(),
                    target: "[::1]:22".into(),
                },
            },
        ] {
            let b = r.encode().unwrap();
            assert_eq!(VisitRequest::decode(&b).unwrap(), r);
            for i in 0..b.len() {
                assert!(VisitRequest::decode(&b[..i]).is_err());
            }
            let mut t = b.clone();
            t.push(0);
            assert_eq!(VisitRequest::decode(&t), Err(DecodeError::Trailing));
        }
        assert!(VisitRequest::decode(&[0x20, 2, 1, 2, b'd', b'b']).is_err());
        assert_eq!(
            VisitRequest::decode(&[0x20, 4, 1, 2, b'd', b'b', 0, 0]),
            Err(DecodeError::UnknownType(4))
        );
        assert_eq!(
            VisitRequest::decode(&[0x20, 0x80, 1, 2, b'd', b'b', 0, 0]),
            Err(DecodeError::UnknownType(0x80))
        );
        assert!(VisitRequest::decode(&[0x20, 0, 3, 2, b'd', b'b']).is_err());
        assert_eq!(
            VisitRequest::decode(&[0x20, 0, 1, 3, b'a', b' ', b'b']),
            Err(DecodeError::BadName)
        );
        assert_eq!(
            VisitRequest::decode(&[0x10, 0, 1, 2, b'd', b'b']),
            Err(DecodeError::UnknownType(0x10))
        );
    }

    #[test]
    fn a_udp_visit_request_sets_flag_bit_1_and_a_tcp_one_keeps_the_v1_bytes() {
        let tcp = VisitRequest {
            check: false,
            protocol: Protocol::Tcp,
            to: VisitTarget::service("db"),
        };
        assert_eq!(tcp.encode().unwrap(), vec![0x20, 0, 1, 2, b'd', b'b', 0, 0]);
        let udp = VisitRequest {
            protocol: Protocol::Udp,
            ..tcp.clone()
        };
        assert_eq!(udp.encode().unwrap(), vec![0x20, 2, 1, 2, b'd', b'b', 0, 0]);
        let checked = VisitRequest {
            check: true,
            protocol: Protocol::Udp,
            to: VisitTarget::Client {
                client: "home".into(),
                target: "10.0.0.1:53".into(),
            },
        };
        let b = checked.encode().unwrap();
        assert_eq!(&b[..3], &[0x20, 3, 2]);
        for r in [udp, checked] {
            let b = r.encode().unwrap();
            assert_eq!(VisitRequest::decode(&b).unwrap(), r);
            for i in 0..b.len() {
                assert!(VisitRequest::decode(&b[..i]).is_err());
            }
        }
    }

    #[test]
    fn grants_v2_carry_each_service_protocol_and_v1_lists_tcp_only() {
        let grants = Grants {
            services: vec![
                GrantedService {
                    name: "db".into(),
                    protocol: Protocol::Tcp,
                    port: 5432,
                    ports: 1,
                },
                GrantedService {
                    name: "dns".into(),
                    protocol: Protocol::Udp,
                    port: 53,
                    ports: 2,
                },
            ],
            clients: vec![GrantedClient {
                name: "home".into(),
                reach: vec![],
                allow: None,
            }],
        };
        assert_eq!(GRANTS_REQUEST_V2, [0x21, 0x01]);
        let v2 = grants.encode_v2().unwrap();
        assert_eq!(
            v2,
            unhex("250002026462001538000103646e730100350002000104686f6d650000")
        );
        assert_eq!(Grants::decode(&v2).unwrap(), grants);
        let v1 = grants.encode().unwrap();
        assert_eq!(v1[0], 0x22);
        let old = Grants::decode(&v1).unwrap();
        assert_eq!(old.services, grants.services[..1].to_vec());
        assert_eq!(old.clients, grants.clients);
        let mut bad = v2.clone();
        bad[6] = 2;
        assert_eq!(Grants::decode(&bad), Err(DecodeError::UnknownType(2)));
        for i in 0..v2.len() {
            assert!(Grants::decode(&v2[..i]).is_err());
        }
        let mut trailing = v2;
        trailing.push(0);
        assert!(Grants::decode(&trailing).is_err());
    }

    #[test]
    fn statuses_decode() {
        for s in [
            Status::Ok,
            Status::Denied,
            Status::Unreachable,
            Status::Unresolved,
            Status::Invalid,
            Status::Forbidden,
            Status::Offline,
            Status::Busy,
        ] {
            assert_eq!(Status::decode(&[s as u8]), Ok(s));
        }
        assert_eq!(Status::decode(&[8]), Err(DecodeError::UnknownType(8)));
    }

    #[test]
    fn decode_rejects_truncated_and_trailing() {
        let w = ServerMsg::Welcome {
            token: [1; 16],
            heartbeat_secs: 5,
        }
        .encode();
        for i in 0..w.len() {
            assert!(ServerMsg::decode(&w[..i]).is_err());
        }
        let mut t = w.clone();
        t.push(0);
        assert_eq!(ServerMsg::decode(&t), Err(DecodeError::Trailing));
        assert!(ServerMsg::decode(&[2, 0xff, 0xff]).is_err());
        assert_eq!(
            decode_datagram(&[0, 0, 0, 7, 1, 2]).unwrap(),
            (7, &[1u8, 2][..])
        );
        assert!(decode_datagram(&[0, 0]).is_err());
    }
}
