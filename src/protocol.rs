use crate::config::check::MAX_NAME_LEN;
use crate::config::model::{Cipher, Protocol};

pub const MAGIC: [u8; 4] = *b"BRW1";
pub const VERSION: u8 = 1;
pub const PREAMBLE_FIXED: usize = 7;
pub const TOKEN_LEN: usize = 16;
pub const KEEPALIVE_RECORD: u8 = 0x00;

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
        if !crate::config::check::valid_name(name) {
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
    Control { version: u8 },
    Data { version: u8, token: [u8; TOKEN_LEN] },
    Visit { version: u8 },
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
            Hello::Control { version } => vec![1, *version],
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
            1 => Hello::Control { version: r.u8()? },
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMsg {
    Pong,
}

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
                    v.extend_from_slice(&s.ports.to_be_bytes());
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
                    let ports = r.u16()?;
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
        }
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        let m = match r.u8()? {
            0x81 => ClientMsg::Pong,
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
}

impl Connect {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![0x10, proto_byte(self.protocol)];
        put_str(&mut v, &self.service);
        put_str(&mut v, &self.target);
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
                target: crate::config::check::ports::port_at(target, offset),
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
    pub to: VisitTarget,
}

impl VisitRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![0x20, u8::from(self.check)];
        match &self.to {
            VisitTarget::Service { name, index } => {
                v.push(1);
                put_str(&mut v, name);
                v.extend_from_slice(&index.to_be_bytes());
            }
            VisitTarget::Client { client, target } => {
                v.push(2);
                put_str(&mut v, client);
                put_str(&mut v, target);
            }
        }
        v
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { buf: b };
        match r.u8()? {
            0x20 => {}
            x => return Err(DecodeError::UnknownType(x)),
        }
        let check = match r.u8()? {
            0 => false,
            1 => true,
            x => return Err(DecodeError::UnknownType(x)),
        };
        let name = |r: &mut Reader| {
            let n = r.short_str()?;
            if crate::config::check::valid_name(&n) {
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
        Ok(VisitRequest { check, to })
    }
}

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
        };
        assert_eq!(Connect::decode(&c.encode()).unwrap(), c);
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
                to: VisitTarget::service("db"),
            },
            VisitRequest {
                check: true,
                to: VisitTarget::Client {
                    client: "home".into(),
                    target: "[::1]:22".into(),
                },
            },
        ] {
            let b = r.encode();
            assert_eq!(VisitRequest::decode(&b).unwrap(), r);
            for i in 0..b.len() {
                assert!(VisitRequest::decode(&b[..i]).is_err());
            }
            let mut t = b.clone();
            t.push(0);
            assert_eq!(VisitRequest::decode(&t), Err(DecodeError::Trailing));
        }
        assert!(VisitRequest::decode(&[0x20, 2, 1, 2, b'd', b'b']).is_err());
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
