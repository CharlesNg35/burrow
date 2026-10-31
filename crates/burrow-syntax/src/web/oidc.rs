pub const RESERVED: &str = "/.burrow/";
pub const LOGIN_PATH: &str = "/.burrow/login";
pub const CALLBACK_PATH: &str = "/.burrow/login/callback";
pub const LOGOUT_PATH: &str = "/.burrow/logout";
pub const DISCOVERY: &str = "/.well-known/openid-configuration";

pub fn user_allowed(allow: &[String], email: &str) -> bool {
    let email = email.trim();
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    allow
        .iter()
        .map(|a| a.trim())
        .any(|a| match a.strip_prefix('@') {
            Some(d) => !d.is_empty() && d.eq_ignore_ascii_case(domain),
            None => a.eq_ignore_ascii_case(email),
        })
}

pub fn allow_entry(entry: &str) -> Result<(), &'static str> {
    let e = entry.trim();
    let (local, domain) = match e.strip_prefix('@') {
        Some(d) => ("x", d),
        None => e
            .rsplit_once('@')
            .ok_or("is neither an email address nor @domain")?,
    };
    let good_domain = domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if local.is_empty() || !good_domain || local.contains(char::is_whitespace) {
        return Err("is neither an email address nor @domain");
    }
    Ok(())
}

pub fn issuer_problem(issuer: &str) -> Option<&'static str> {
    let i = issuer.trim();
    let rest = i
        .strip_prefix("https://")
        .or_else(|| i.strip_prefix("http://"));
    match rest {
        None => Some("must start with https://"),
        Some(r) if r.is_empty() || r.starts_with('/') => Some("has no host name"),
        Some(_) if i.contains(['?', '#']) => Some("must not have a query or fragment"),
        Some(_) => None,
    }
}

pub fn discovery_url(issuer: &str) -> String {
    format!("{}{DISCOVERY}", issuer.trim().trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_is_allowed_by_email_or_by_the_whole_domain() {
        let allow = vec!["alice@gmail.com".to_string(), " @Example.COM ".to_string()];
        assert!(user_allowed(&allow, "Alice@Gmail.com"));
        assert!(user_allowed(&allow, "bob@example.com"));
        assert!(!user_allowed(&allow, "bob@gmail.com"));
        assert!(!user_allowed(&allow, "bob@sub.example.com"));
        assert!(!user_allowed(&allow, "bob@evilexample.com"));
        assert!(!user_allowed(&allow, "example.com"));
        assert!(!user_allowed(&["@".to_string()], "a@b.c"));
    }

    #[test]
    fn an_allow_entry_is_an_email_or_an_at_domain() {
        assert!(allow_entry("alice@gmail.com").is_ok());
        assert!(allow_entry("@mycompany.com").is_ok());
        for bad in ["alice", "@", "@com", "alice@", "a b@x.com", "@.x.com", "x@y.com."] {
            assert!(allow_entry(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_issuer_is_an_http_url_without_query_and_its_discovery_document_sits_below_it() {
        assert_eq!(issuer_problem("https://accounts.google.com"), None);
        assert_eq!(issuer_problem("http://10.0.0.5:8080/realms/home"), None);
        assert!(issuer_problem("accounts.google.com").is_some());
        assert!(issuer_problem("https://").is_some());
        assert!(issuer_problem("https://a.example.com/?x=1").is_some());
        assert_eq!(
            discovery_url("https://sso.example.com/realms/home/"),
            "https://sso.example.com/realms/home/.well-known/openid-configuration"
        );
    }
}
