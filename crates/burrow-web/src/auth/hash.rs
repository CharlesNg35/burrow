use std::fmt;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params};

pub use burrow_syntax::passwd::{
    ARGON2_MAX_LANES, ARGON2_MAX_MEMORY_KIB, ARGON2_MAX_TIME, BCRYPT_COSTS,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashProblem {
    NotSupported,
    Bcrypt2x,
    Argon2Variant(&'static str),
    TooCostly {
        what: &'static str,
        asked: u32,
        most: u32,
    },
    BcryptCost(u32),
}

impl fmt::Display for HashProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashProblem::NotSupported => write!(f, "is not argon2id or bcrypt"),
            HashProblem::Bcrypt2x => {
                write!(f, "is bcrypt $2x$, an old variant with a known bug")
            }
            HashProblem::Argon2Variant(v) => write!(f, "is {v}; only argon2id is accepted"),
            HashProblem::TooCostly { what, asked, most } => {
                let unit = if *what == "memory" { " KiB" } else { "" };
                write!(
                    f,
                    "asks for more work than burrow allows (argon2 {what} {asked}{unit}, at most {most}{unit})"
                )
            }
            HashProblem::BcryptCost(c) => write!(
                f,
                "asks for a bcrypt cost burrow does not allow ({c}, allowed {} to {})",
                BCRYPT_COSTS.start(),
                BCRYPT_COSTS.end()
            ),
        }
    }
}

#[derive(Clone)]
pub(crate) enum Stored {
    Argon2(Box<PasswordHash>),
    Bcrypt(String),
}

fn bcrypt_cost(hash: &str) -> Option<Result<u32, HashProblem>> {
    let prefix = hash.get(..4)?;
    let version = match prefix {
        "$2a$" | "$2b$" | "$2y$" => true,
        "$2x$" => false,
        _ => return None,
    };
    if !version {
        return Some(Err(HashProblem::Bcrypt2x));
    }
    let ok = hash.len() == 60 && hash.is_ascii() && hash.as_bytes().get(6) == Some(&b'$');
    let cost = hash.get(4..6).and_then(|c| c.parse::<u32>().ok());
    Some(match cost {
        Some(c) if ok && BCRYPT_COSTS.contains(&c) => Ok(c),
        Some(c) if ok => Err(HashProblem::BcryptCost(c)),
        _ => Err(HashProblem::NotSupported),
    })
}

fn argon2_hash(hash: &str) -> Result<PasswordHash, HashProblem> {
    match hash.split('$').nth(1) {
        Some("argon2id") => {}
        Some("argon2i") => return Err(HashProblem::Argon2Variant("argon2i")),
        Some("argon2d") => return Err(HashProblem::Argon2Variant("argon2d")),
        _ => return Err(HashProblem::NotSupported),
    }
    let parsed = PasswordHash::new(hash).map_err(|_| HashProblem::NotSupported)?;
    if parsed.algorithm.as_str() != "argon2id" {
        return Err(HashProblem::NotSupported);
    }
    if parsed.salt.is_none() || parsed.hash.is_none() {
        return Err(HashProblem::NotSupported);
    }
    let params = Params::try_from(&parsed).map_err(|_| HashProblem::NotSupported)?;
    let caps = [
        ("memory", params.m_cost(), ARGON2_MAX_MEMORY_KIB),
        ("time cost", params.t_cost(), ARGON2_MAX_TIME),
        ("parallelism", params.p_cost(), ARGON2_MAX_LANES),
    ];
    if let Some((what, asked, most)) = caps.into_iter().find(|(_, a, m)| a > m) {
        return Err(HashProblem::TooCostly { what, asked, most });
    }
    Ok(parsed)
}

pub(crate) fn stored(hash: &str) -> Result<Stored, HashProblem> {
    let hash = hash.trim();
    match bcrypt_cost(hash) {
        Some(Ok(_)) => Ok(Stored::Bcrypt(hash.to_string())),
        Some(Err(e)) => Err(e),
        None => argon2_hash(hash).map(|h| Stored::Argon2(Box::new(h))),
    }
}

pub fn check_hash(hash: &str) -> Result<(), HashProblem> {
    stored(hash).map(|_| ())
}

pub fn hash_password(password: &[u8]) -> String {
    Argon2::new(Algorithm::Argon2id, argon2::Version::V0x13, Params::DEFAULT)
        .hash_password(password)
        .expect("argon2id with the default parameters hashes any password")
        .to_string()
}

pub fn verify_hash(password: &[u8], hash: &str) -> bool {
    stored(hash).is_ok_and(|s| verify(password, &s))
}

pub(crate) fn verify(password: &[u8], stored: &Stored) -> bool {
    match stored {
        Stored::Argon2(h) => Argon2::default()
            .verify_password(password, h.as_ref())
            .is_ok(),
        Stored::Bcrypt(h) => bcrypt::verify(password, h).unwrap_or(false),
    }
}

pub(crate) fn dummy() -> &'static Stored {
    static DUMMY: std::sync::OnceLock<Stored> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        let mut seed = [0u8; 16];
        let _ =
            aws_lc_rs::rand::SecureRandom::fill(&aws_lc_rs::rand::SystemRandom::new(), &mut seed);
        stored(&hash_password(&seed)).expect("our own hash parses")
    })
}

#[cfg(test)]
mod tests;
