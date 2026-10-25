pub const ARGON2_MAX_MEMORY_KIB: u64 = 65536;
pub const ARGON2_MAX_TIME: u64 = 10;
pub const ARGON2_MAX_LANES: u64 = 4;
pub const BCRYPT_COST: (u32, u32) = (4, 14);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashProblem {
    Unknown,
    OldBcrypt,
    Argon2Variant(&'static str),
    TooCostly(String),
}

fn argon2(parts: &[&str]) -> Result<(), HashProblem> {
    let params = parts
        .iter()
        .find(|p| p.contains("m="))
        .ok_or(HashProblem::Unknown)?;
    let mut seen = 0;
    for kv in params.split(',') {
        let (k, v) = kv.split_once('=').ok_or(HashProblem::Unknown)?;
        let v: u64 = v.parse().map_err(|_| HashProblem::Unknown)?;
        let (limit, what) = match k {
            "m" => (ARGON2_MAX_MEMORY_KIB, "argon2 memory"),
            "t" => (ARGON2_MAX_TIME, "argon2 time cost"),
            "p" => (ARGON2_MAX_LANES, "argon2 parallelism"),
            _ => continue,
        };
        seen += 1;
        if v > limit {
            let unit = if k == "m" { " KiB" } else { "" };
            return Err(HashProblem::TooCostly(format!(
                "{what} {v}{unit}, at most {limit}{unit}"
            )));
        }
    }
    if seen == 3 && parts.len() >= 6 && !parts[parts.len() - 1].is_empty() {
        Ok(())
    } else {
        Err(HashProblem::Unknown)
    }
}

fn bcrypt(variant: &str, parts: &[&str]) -> Result<(), HashProblem> {
    if variant == "2x" {
        return Err(HashProblem::OldBcrypt);
    }
    let (cost, rest) = match parts {
        [_, _, cost, rest] => (cost, rest),
        _ => return Err(HashProblem::Unknown),
    };
    if rest.len() != 53 || cost.len() != 2 {
        return Err(HashProblem::Unknown);
    }
    let cost: u32 = cost.parse().map_err(|_| HashProblem::Unknown)?;
    if !(BCRYPT_COST.0..=BCRYPT_COST.1).contains(&cost) {
        return Err(HashProblem::TooCostly(format!(
            "bcrypt cost {cost}, between {} and {}",
            BCRYPT_COST.0, BCRYPT_COST.1
        )));
    }
    Ok(())
}

pub fn check_hash(hash: &str) -> Result<(), HashProblem> {
    let parts: Vec<&str> = hash.trim().split('$').collect();
    match parts.get(1).copied() {
        Some("argon2id") => argon2(&parts),
        Some("argon2i") => Err(HashProblem::Argon2Variant("argon2i")),
        Some("argon2d") => Err(HashProblem::Argon2Variant("argon2d")),
        Some(v @ ("2a" | "2b" | "2y" | "2x")) => bcrypt(v, &parts),
        _ => Err(HashProblem::Unknown),
    }
}

pub fn valid_user(user: &str) -> bool {
    !user.is_empty() && !user.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "$2b$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy";

    #[test]
    fn argon2id_and_bcrypt_within_the_caps_are_accepted() {
        assert_eq!(
            check_hash("$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g"),
            Ok(())
        );
        assert_eq!(check_hash(B), Ok(()));
        assert_eq!(check_hash(&B.replace("$2b$", "$2y$")), Ok(()));
    }

    #[test]
    fn other_variants_and_costly_hashes_are_refused() {
        assert_eq!(
            check_hash(&B.replace("$2b$", "$2x$")),
            Err(HashProblem::OldBcrypt)
        );
        assert_eq!(
            check_hash("$argon2i$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"),
            Err(HashProblem::Argon2Variant("argon2i"))
        );
        assert_eq!(
            check_hash("$argon2id$v=19$m=4194304,t=2,p=1$c2FsdA$aGFzaA"),
            Err(HashProblem::TooCostly(
                "argon2 memory 4194304 KiB, at most 65536 KiB".into()
            ))
        );
        assert!(matches!(
            check_hash("$argon2id$v=19$m=1024,t=11,p=1$c2FsdA$aGFzaA"),
            Err(HashProblem::TooCostly(_))
        ));
        assert!(matches!(
            check_hash(&B.replace("$10$", "$15$")),
            Err(HashProblem::TooCostly(_))
        ));
        assert_eq!(check_hash("hunter2"), Err(HashProblem::Unknown));
        assert_eq!(check_hash("{SHA}abc"), Err(HashProblem::Unknown));
        assert!(!valid_user("a:b") && !valid_user("") && valid_user("alice"));
    }
}
