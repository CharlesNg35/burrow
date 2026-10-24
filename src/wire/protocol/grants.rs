use super::{DecodeError, Reader, proto_byte, proto_from, put_short, put_strings};
use burrow_config::Protocol;

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
