use std::io::{BufRead, IsTerminal};

use zeroize::Zeroizing;

use crate::base::failure::Failure;

fn typed(prompt: &str) -> Result<Zeroizing<String>, Failure> {
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|e| Failure::new(format!("cannot read the password: {e}")))
}

fn piped() -> Result<Zeroizing<String>, Failure> {
    let mut line = Zeroizing::new(String::new());
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Failure::new(format!("cannot read the password: {e}")))?;
    let end = line.trim_end_matches(['\r', '\n']).len();
    line.truncate(end);
    Ok(line)
}

fn password(user: Option<&str>) -> Result<Zeroizing<String>, Failure> {
    if !std::io::stdin().is_terminal() {
        return piped();
    }
    let first = match user {
        Some(u) => typed(&format!("Password for {u}: "))?,
        None => typed("Password: ")?,
    };
    let again = typed("The same password again: ")?;
    if *first != *again {
        return Err(Failure::usage(
            "the two passwords differ; nothing was written",
        ));
    }
    Ok(first)
}

pub(super) fn line(user: Option<&str>, password: &str) -> Result<String, Failure> {
    if password.is_empty() {
        return Err(Failure::usage("the password is empty; nothing was written"));
    }
    if let Some(u) = user
        && !burrow_syntax::passwd::valid_user(u)
    {
        return Err(Failure::usage(format!(
            "user name \"{u}\" must not be empty or hold \":\""
        )));
    }
    let hash = crate::server::passwords::hash(password.as_bytes()).map_err(Failure::new)?;
    Ok(match user {
        Some(u) => format!("{u}: {hash}\n"),
        None => format!("{hash}\n"),
    })
}

pub(super) fn run(user: Option<&str>) -> Result<String, Failure> {
    let pw = password(user)?;
    line(user, &pw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_pastes_into_basic_auth_and_verifies() {
        let l = line(Some("bob"), "s3cret").unwrap();
        let hash = l.strip_prefix("bob: ").unwrap().trim_end();
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{l}");
        assert!(burrow_web::auth::verify_hash(b"s3cret", hash));
        assert!(!burrow_web::auth::verify_hash(b"s3cre", hash));
        assert!(line(None, "x").unwrap().starts_with("$argon2id$"));
    }

    #[test]
    fn empty_passwords_and_bad_names_write_nothing() {
        assert!(line(Some("bob"), "").is_err());
        assert!(line(Some("a:b"), "pw").is_err());
        assert!(line(Some(""), "pw").is_err());
    }
}
