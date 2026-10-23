use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::token::Token;

pub const KEEP_SECS: u64 = 12 * 3600;

#[derive(Debug, Clone)]
pub struct Session {
    pub token: Token,
    pub prefix: String,
    pub port: Option<u16>,
    pub reused: bool,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
    prefix: String,
    port: u16,
    used: u64,
}

pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("BURROW_ADMIN_SESSION_DIR") {
        return Some(PathBuf::from(d));
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("burrow").join("admin-sessions"))
}

fn file_for(dir: &Path, config: &Path) -> PathBuf {
    let key = std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf());
    let digest = ring::digest::digest(&ring::digest::SHA256, key.to_string_lossy().as_bytes());
    dir.join(format!(
        "session-{}.json",
        hex::encode(&digest.as_ref()[..12])
    ))
}

fn valid(s: &Stored) -> bool {
    let hex_of = |t: &str, n: usize| t.len() == n && t.bytes().all(|b| b.is_ascii_hexdigit());
    hex_of(&s.token, 64)
        && s.prefix.strip_prefix("/r-").is_some_and(|h| hex_of(h, 32))
        && s.port != 0
}

impl Session {
    pub fn fresh() -> Session {
        Session {
            token: Token::generate(),
            prefix: super::http::run_prefix(),
            port: None,
            reused: false,
        }
    }
}

pub struct Keep {
    pub dir: PathBuf,
    pub config: PathBuf,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn load(dir: &Path, config: &Path, now: u64) -> Session {
    let fresh = Session::fresh;
    let Ok(text) = std::fs::read_to_string(file_for(dir, config)) else {
        return fresh();
    };
    match serde_json::from_str::<Stored>(&text) {
        Ok(s) if valid(&s) && now.saturating_sub(s.used) < KEEP_SECS && s.used <= now + 60 => {
            Session {
                token: Token::from_stored(s.token),
                prefix: s.prefix,
                port: Some(s.port),
                reused: true,
            }
        }
        _ => fresh(),
    }
}

pub fn save(dir: &Path, config: &Path, s: &Session, port: u16, now: u64) -> std::io::Result<()> {
    make_private_dir(dir)?;
    let text = serde_json::to_string(&Stored {
        token: s.token.as_str().to_string(),
        prefix: s.prefix.clone(),
        port,
        used: now,
    })
    .map_err(std::io::Error::other)?;
    crate::instance::write_private(&file_for(dir, config), &text)
}

fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "server: x\n").unwrap();
        p
    }

    #[test]
    fn a_session_is_reused_for_the_same_config_until_it_has_been_unused_for_12_hours() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("state").join("admin");
        let ops = config(home.path(), "ops.yaml");
        let first = load(&store, &ops, 1000);
        assert!(!first.reused);
        save(&store, &ops, &first, 37839, 1000).unwrap();
        let again = load(&store, &ops, 1000 + KEEP_SECS - 1);
        assert!(again.reused);
        assert_eq!(again.token.as_str(), first.token.as_str());
        assert_eq!(again.prefix, first.prefix);
        assert_eq!(again.port, Some(37839));
        let late = load(&store, &ops, 1000 + KEEP_SECS);
        assert!(!late.reused, "expired after 12 h without use");
        assert_ne!(late.token.as_str(), first.token.as_str());
        let other = config(home.path(), "other.yaml");
        assert!(!load(&store, &other, 1000).reused, "one session per config");
    }

    #[test]
    fn the_session_file_is_private_and_a_broken_one_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("admin");
        let ops = config(home.path(), "ops.yaml");
        let s = load(&store, &ops, 5);
        save(&store, &ops, &s, 4000, 5).unwrap();
        let file = file_for(&store, &ops);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let d = std::fs::metadata(&store).unwrap().permissions().mode();
            assert_eq!(d & 0o777, 0o700);
        }
        for broken in [
            "not json",
            "{\"token\":\"abc\",\"prefix\":\"/r-00\",\"port\":4000,\"used\":5}",
            &format!(
                "{{\"token\":\"{}\",\"prefix\":\"/x\",\"port\":4000,\"used\":5}}",
                "a".repeat(64)
            ),
        ] {
            std::fs::write(&file, broken).unwrap();
            assert!(!load(&store, &ops, 5).reused, "{broken}");
        }
    }
}
