use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use blake2::{Blake2s256, Digest};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const SECRET_LEN: usize = 32;

#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Secret([u8; SECRET_LEN]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({})", self.fingerprint())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    Empty,
    NotDecodable { chars: usize },
    WrongLength { bytes: usize },
    LowEntropy,
}

impl SecretError {
    pub fn describe(&self) -> String {
        match self {
            SecretError::Empty => "is empty".into(),
            SecretError::NotDecodable { chars } => {
                format!(
                    "is not valid base64 or hex ({chars} {})",
                    if *chars == 1 {
                        "character"
                    } else {
                        "characters"
                    }
                )
            }
            SecretError::WrongLength { bytes } if *bytes < SECRET_LEN => {
                format!("is too short ({bytes} bytes, need {SECRET_LEN})")
            }
            SecretError::WrongLength { bytes } => {
                format!("is too long ({bytes} bytes, need exactly {SECRET_LEN})")
            }
            SecretError::LowEntropy => "does not look random (too many repeated bytes)".into(),
        }
    }
}

impl Secret {
    pub fn generate() -> Self {
        let mut bytes = [0u8; SECRET_LEN];
        getrandom::fill(&mut bytes).expect("operating system random source is available");
        Secret(bytes)
    }

    pub fn parse(text: &str) -> Result<Self, SecretError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SecretError::Empty);
        }
        let mut bytes = decode(text).ok_or(SecretError::NotDecodable {
            chars: text.chars().count(),
        })?;
        if bytes.len() != SECRET_LEN {
            let n = bytes.len();
            bytes.zeroize();
            return Err(SecretError::WrongLength { bytes: n });
        }
        let mut out = [0u8; SECRET_LEN];
        out.copy_from_slice(&bytes);
        bytes.zeroize();
        let secret = Secret(out);
        if distinct_bytes(&secret.0) < 16 {
            return Err(SecretError::LowEntropy);
        }
        Ok(secret)
    }

    pub fn from_bytes(bytes: [u8; SECRET_LEN]) -> Self {
        Secret(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }

    pub fn encode(&self) -> String {
        STANDARD.encode(self.0)
    }

    pub fn fingerprint(&self) -> String {
        let digest = Blake2s256::new()
            .chain_update(b"burrow secret fingerprint")
            .chain_update(self.0)
            .finalize();
        digest[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() == SECRET_LEN * 2 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return decode_hex(text);
    }
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(v) = engine.decode(text) {
            return Some(v);
        }
    }
    if text.len().is_multiple_of(2) && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return decode_hex(text);
    }
    None
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn distinct_bytes(bytes: &[u8]) -> usize {
    let mut seen = [false; 256];
    bytes.iter().for_each(|b| seen[*b as usize] = true);
    seen.iter().filter(|s| **s).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_round_trips() {
        let s = Secret::generate();
        assert_eq!(Secret::parse(&s.encode()).unwrap(), s);
        let hex: String = s.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(Secret::parse(&hex).unwrap(), s);
        let url = URL_SAFE_NO_PAD.encode(s.as_bytes());
        assert_eq!(Secret::parse(&url).unwrap(), s);
    }

    #[test]
    fn rejects_weak() {
        assert_eq!(Secret::parse(""), Err(SecretError::Empty));
        assert_eq!(
            Secret::parse("not base64!"),
            Err(SecretError::NotDecodable { chars: 11 })
        );
        assert_eq!(
            Secret::parse("hunter2"),
            Err(SecretError::NotDecodable { chars: 7 })
        );
        assert_eq!(
            Secret::parse(&STANDARD.encode([1u8; 12])),
            Err(SecretError::WrongLength { bytes: 12 })
        );
        assert_eq!(
            Secret::parse(&STANDARD.encode([0u8; 32])),
            Err(SecretError::LowEntropy)
        );
        let seq: Vec<u8> = (0..32u8).map(|i| i % 8).collect();
        assert_eq!(
            Secret::parse(&STANDARD.encode(seq)),
            Err(SecretError::LowEntropy)
        );
        assert_eq!(
            Secret::parse(&STANDARD.encode([1u8; 48])),
            Err(SecretError::WrongLength { bytes: 48 })
        );
    }

    #[test]
    fn debug_hides_value() {
        let s = Secret::generate();
        let dbg = format!("{s:?}");
        assert!(!dbg.contains(&s.encode()));
        assert_eq!(s.fingerprint().len(), 8);
    }
}
