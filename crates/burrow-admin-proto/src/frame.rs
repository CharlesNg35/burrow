pub const ADMIN_OPEN: u8 = 0x23;
pub const ADMIN_ACCEPTED: u8 = 0x24;
pub const FLAG_MORE: u8 = 0x01;
pub const FLAG_PING: u8 = 0x80;
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
pub const MAX_CHUNK: usize = 65_518;
pub const MAX_OPEN_RECORD: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminOpen {
    pub min: u8,
    pub max: u8,
    pub client: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Truncated,
    Trailing,
    UnknownType,
    TooLong,
    ReservedFlags,
    TooLarge,
    NotUtf8,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FrameError::Truncated => "the record ends too early",
            FrameError::Trailing => "the record has extra bytes",
            FrameError::UnknownType => "the record has an unknown type",
            FrameError::TooLong => "a field is too long",
            FrameError::ReservedFlags => "the record sets reserved flags",
            FrameError::TooLarge => "the message is larger than 4 MiB",
            FrameError::NotUtf8 => "the message is not UTF-8",
        })
    }
}

fn put_str8(v: &mut Vec<u8>, s: &str) -> Result<(), FrameError> {
    let n = u8::try_from(s.len()).map_err(|_| FrameError::TooLong)?;
    v.push(n);
    v.extend_from_slice(s.as_bytes());
    Ok(())
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, FrameError> {
        let (&b, rest) = self.0.split_first().ok_or(FrameError::Truncated)?;
        self.0 = rest;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], FrameError> {
        if self.0.len() < n {
            return Err(FrameError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }

    fn str8(&mut self) -> Result<String, FrameError> {
        let n = self.u8()? as usize;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| FrameError::NotUtf8)
    }
}

impl AdminOpen {
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut v = vec![ADMIN_OPEN, self.min, self.max];
        put_str8(&mut v, &self.client)?;
        let n = u16::try_from(self.features.len()).map_err(|_| FrameError::TooLong)?;
        v.extend_from_slice(&n.to_be_bytes());
        for f in &self.features {
            put_str8(&mut v, f)?;
        }
        if v.len() > MAX_OPEN_RECORD {
            return Err(FrameError::TooLong);
        }
        Ok(v)
    }

    pub fn decode(b: &[u8]) -> Result<Self, FrameError> {
        let mut r = Reader(b);
        if r.u8()? != ADMIN_OPEN {
            return Err(FrameError::UnknownType);
        }
        let min = r.u8()?;
        let max = r.u8()?;
        let client = r.str8()?;
        let hi = r.u8()?;
        let lo = r.u8()?;
        let n = u16::from_be_bytes([hi, lo]);
        let mut features = Vec::new();
        for _ in 0..n {
            features.push(r.str8()?);
        }
        if !r.0.is_empty() {
            return Err(FrameError::Trailing);
        }
        Ok(AdminOpen {
            min,
            max,
            client,
            features,
        })
    }

    pub fn is_admin_open(b: &[u8]) -> bool {
        b.first() == Some(&ADMIN_OPEN)
    }
}

pub fn accepted(version: u8) -> [u8; 2] {
    [ADMIN_ACCEPTED, version]
}

pub fn decode_accepted(b: &[u8]) -> Option<u8> {
    match b {
        [ADMIN_ACCEPTED, v] => Some(*v),
        _ => None,
    }
}

pub fn chunks(message: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
    if message.len() > MAX_MESSAGE {
        return Err(FrameError::TooLarge);
    }
    if message.is_empty() {
        return Ok(vec![vec![0]]);
    }
    let pieces: Vec<&[u8]> = message.chunks(MAX_CHUNK).collect();
    let last = pieces.len() - 1;
    Ok(pieces
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let mut rec = Vec::with_capacity(c.len() + 1);
            rec.push(if i < last { FLAG_MORE } else { 0 });
            rec.extend_from_slice(c);
            rec
        })
        .collect())
}

pub fn ping() -> [u8; 1] {
    [FLAG_PING]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Ping,
    Partial,
    Message(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct Assembler {
    buf: Vec<u8>,
}

impl Assembler {
    pub fn new() -> Self {
        Assembler::default()
    }

    pub fn push(&mut self, record: &[u8]) -> Result<Frame, FrameError> {
        let (&flags, chunk) = record.split_first().ok_or(FrameError::Truncated)?;
        if flags & !(FLAG_MORE | FLAG_PING) != 0 {
            return Err(FrameError::ReservedFlags);
        }
        if flags & FLAG_PING != 0 {
            if flags != FLAG_PING || !chunk.is_empty() {
                return Err(FrameError::ReservedFlags);
            }
            return Ok(Frame::Ping);
        }
        if chunk.len() > MAX_CHUNK {
            return Err(FrameError::TooLong);
        }
        if self.buf.len() + chunk.len() > MAX_MESSAGE {
            self.buf = Vec::new();
            return Err(FrameError::TooLarge);
        }
        self.buf.extend_from_slice(chunk);
        if flags & FLAG_MORE != 0 {
            return Ok(Frame::Partial);
        }
        let msg = std::mem::take(&mut self.buf);
        if std::str::from_utf8(&msg).is_err() {
            return Err(FrameError::NotUtf8);
        }
        Ok(Frame::Message(msg))
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(n: usize) -> Vec<u8> {
        let msg: Vec<u8> = (0..n).map(|i| b'a' + (i % 26) as u8).collect();
        let recs = chunks(&msg).unwrap();
        let mut a = Assembler::new();
        let mut out = None;
        for (i, r) in recs.iter().enumerate() {
            assert!(r.len() <= MAX_CHUNK + 1);
            match a.push(r).unwrap() {
                Frame::Message(m) => {
                    assert_eq!(i, recs.len() - 1);
                    out = Some(m);
                }
                Frame::Partial => assert!(i < recs.len() - 1),
                Frame::Ping => panic!(),
            }
        }
        assert_eq!(out.as_deref(), Some(&msg[..]));
        msg
    }

    #[test]
    fn chunking_at_the_boundaries() {
        for n in [
            0,
            1,
            MAX_CHUNK - 1,
            MAX_CHUNK,
            MAX_CHUNK + 1,
            2 * MAX_CHUNK,
            MAX_MESSAGE,
        ] {
            roundtrip(n);
        }
        assert_eq!(chunks(&vec![0; MAX_MESSAGE + 1]), Err(FrameError::TooLarge));
        assert_eq!(chunks(&[1; MAX_CHUNK]).unwrap().len(), 1);
        assert_eq!(chunks(&[1; MAX_CHUNK + 1]).unwrap().len(), 2);
    }

    #[test]
    fn an_oversized_message_is_refused_while_it_arrives() {
        let mut a = Assembler::new();
        let mut rec = vec![FLAG_MORE];
        rec.extend_from_slice(&[b'x'; MAX_CHUNK]);
        let mut total = 0;
        let err = loop {
            match a.push(&rec) {
                Ok(Frame::Partial) => total += MAX_CHUNK,
                Err(e) => break e,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(err, FrameError::TooLarge);
        assert!(total <= MAX_MESSAGE);
    }

    #[test]
    fn flags_are_checked() {
        let mut a = Assembler::new();
        assert_eq!(a.push(&ping()).unwrap(), Frame::Ping);
        assert_eq!(a.push(&[0x02, b'{']), Err(FrameError::ReservedFlags));
        assert_eq!(a.push(&[0x81]), Err(FrameError::ReservedFlags));
        assert_eq!(a.push(&[FLAG_PING, 1]), Err(FrameError::ReservedFlags));
        assert_eq!(a.push(&[]), Err(FrameError::Truncated));
        assert_eq!(a.push(&[0, 0xff]), Err(FrameError::NotUtf8));
    }

    #[test]
    fn admin_open_round_trips() {
        let o = AdminOpen {
            min: 1,
            max: 1,
            client: "burrow-admin-ui/0.1.0".into(),
            features: vec!["secret.reveal-once".into()],
        };
        let b = o.encode().unwrap();
        assert_eq!(b[0], 0x23);
        assert_eq!(AdminOpen::decode(&b).unwrap(), o);
        assert_eq!(
            AdminOpen::decode(&b[..b.len() - 1]),
            Err(FrameError::Truncated)
        );
        let mut extra = b.clone();
        extra.push(0);
        assert_eq!(AdminOpen::decode(&extra), Err(FrameError::Trailing));
        assert_eq!(AdminOpen::decode(&[0x20]), Err(FrameError::UnknownType));
        assert_eq!(decode_accepted(&accepted(1)), Some(1));
        assert_eq!(decode_accepted(&[5]), None);
        let big = AdminOpen {
            features: (0..40).map(|i| format!("feature-{i:020}")).collect(),
            ..o
        };
        assert_eq!(big.encode(), Err(FrameError::TooLong));
    }
}
