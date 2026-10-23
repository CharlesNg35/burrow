#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Token {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("operating system random source is available");
        Token(hex::encode(bytes))
    }

    pub fn from_stored(t: String) -> Token {
        Token(t)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, given: &str) -> bool {
        use subtle::ConstantTimeEq;
        self.0.as_bytes().ct_eq(given.as_bytes()).into()
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_random_and_compared_whole() {
        let t = Token::generate();
        assert_eq!(t.as_str().len(), 64);
        assert_ne!(t.as_str(), Token::generate().as_str());
        assert!(t.matches(t.as_str()));
        assert!(!t.matches(&t.as_str()[..63]));
        assert!(!t.matches(&format!("{}0", t.as_str())));
        assert!(!t.matches(""));
        assert_eq!(format!("{t:?}"), "Token(..)");
    }
}
