mod control;
mod grants;
mod visit;

use burrow_config::{Cipher, Protocol};
use burrow_syntax::name::MAX_NAME_LEN;
pub use control::{ByeReason, ClientMsg, Hello, MAX_ALLOW_RULES, ServerMsg, ServiceInfo};
pub use grants::{GRANTS_REQUEST, GRANTS_REQUEST_V2, GrantedClient, GrantedService, Grants};
use visit::put_short;
pub use visit::{Connect, Status, VisitRequest, VisitTarget};

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
mod tests;
