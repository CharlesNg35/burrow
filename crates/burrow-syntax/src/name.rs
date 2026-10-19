pub const MAX_NAME_LEN: usize = 64;

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a name is required".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "a name has at most {MAX_NAME_LEN} characters, this one has {}",
            name.len()
        ));
    }
    match name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        Some(c) => Err(format!(
            "\"{c}\" is not allowed; use letters, digits, \"-\", \"_\" or \".\""
        )),
        None => Ok(()),
    }
}

pub fn suggest(base: &str, taken: &[&str]) -> String {
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(MAX_NAME_LEN - 6)
        .collect();
    let clean = clean.trim_matches('-');
    let clean = if clean.is_empty() { "new" } else { clean };
    if !taken.contains(&clean) {
        return clean.to_string();
    }
    (2..100_000)
        .map(|i| format!("{clean}-{i}"))
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or_else(|| clean.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("home"));
        assert!(valid_name("a.b-c_d"));
        assert!(!valid_name(""));
        assert!(!valid_name("a b"));
        assert!(!valid_name(&"x".repeat(65)));
        assert!(valid_name(&"x".repeat(64)));
        assert!(check_name("a b").unwrap_err().contains("\" \""));
        assert!(check_name("").is_err());
    }

    #[test]
    fn suggestions_avoid_taken_names() {
        assert_eq!(suggest("Web App", &[]), "web-app");
        assert_eq!(suggest("home", &["home"]), "home-2");
        assert_eq!(suggest("home", &["home", "home-2"]), "home-3");
        assert_eq!(suggest("", &[]), "new");
    }
}
