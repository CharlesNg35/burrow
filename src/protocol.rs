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

fn ranged_target(target: &str, ports: u16) -> String {
    if crate::config::check::unix::path_of(target).is_some() {
        return target.to_string();
    }
    match target.rsplit_once(':') {
        Some((host, port)) if ports > 1 => match port.parse::<u16>() {
            Ok(first) => format!("{host}:{}", crate::config::check::ports::show(first, ports)),
            Err(_) => target.to_string(),
        },
        _ => target.to_string(),
    }
}

fn split_ranged(target: String) -> (String, u16) {
    if crate::config::check::unix::path_of(&target).is_none()
        && let Some((host, ports)) = target.rsplit_once(':')
        && let Some((a, b)) = ports.split_once('-')
        && let (Ok(first), Ok(last)) = (a.parse::<u16>(), b.parse::<u16>())
        && first > 0
        && last > first
    {
        return (format!("{host}:{first}"), last - first + 1);
    }
    (target, 1)
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
                    put_str(&mut v, &ranged_target(&s.target, s.ports));
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
                    let (target, ports) = split_ranged(r.short_str()?);
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

fn put_short(v: &mut Vec<u8>, s: &str) -> Result<(), DecodeError> {
    let b = s.as_bytes();
    let n = u8::try_from(b.len()).map_err(|_| DecodeError::TooLong)?;
    v.push(n);
    v.extend_from_slice(b);
    Ok(())
}

impl VisitRequest {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut v = vec![0x20, u8::from(self.check)];
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

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn only_real_port_ranges_are_read_from_a_target() {
        let split = |t: &str| split_ranged(t.to_string());
        assert_eq!(split("10.0.0.1:30000-30010"), ("10.0.0.1:30000".into(), 11));
        assert_eq!(split("[::1]:1-3"), ("[::1]:1".into(), 3));
        assert_eq!(split("my-host:7-9"), ("my-host:7".into(), 3));
        assert_eq!(split("host:80-80"), ("host:80-80".into(), 1));
        assert_eq!(split("host:0-65535"), ("host:0-65535".into(), 1));
        assert_eq!(split("host:1-65535"), ("host:1".into(), 65535));
        assert_eq!(
            split("unix:/run/app:10-20"),
            ("unix:/run/app:10-20".into(), 1)
        );
        assert_eq!(ranged_target("unix:/run/app:10", 5), "unix:/run/app:10");
        assert_eq!(ranged_target("10.0.0.1:30000", 11), "10.0.0.1:30000-30010");
    }

    #[test]
    fn version_1_peers_read_the_same_bytes() {
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
                "020003037765620001bb0e3132372e302e302e313a383434330467616d65017530183139322e3136382e312e35303a33303030302d333030313002646200000014756e69783a2f72756e2f6d792d64622e736f636b",
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
            (Hello::Control { version: 1 }, "0101"),
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
        };
        let wire = unhex("100103646e730e3139322e3136382e312e313a3533");
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
