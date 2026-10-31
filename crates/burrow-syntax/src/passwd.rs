pub const ARGON2_MAX_MEMORY_KIB: u32 = 65536;
pub const ARGON2_MAX_TIME: u32 = 10;
pub const ARGON2_MAX_LANES: u32 = 4;
pub const BCRYPT_COSTS: std::ops::RangeInclusive<u32> = 4..=14;

pub fn valid_user(user: &str) -> bool {
    !user.is_empty() && !user.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_name_is_not_empty_and_holds_no_colon() {
        assert!(!valid_user("a:b") && !valid_user("") && valid_user("alice"));
    }
}
