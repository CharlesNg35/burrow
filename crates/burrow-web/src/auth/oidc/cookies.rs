use std::time::Duration;

use burrow_syntax::duration::now_secs;
use http::header::{COOKIE, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::seal::Sealer;

pub const OURS: &str = "__Host-burrow-";
pub const SESSION: &str = "__Host-burrow-session";
pub const PENDING: &str = "__Host-burrow-login-";
pub const SESSION_LIFE: Duration = Duration::from_secs(12 * 3600);
pub const PENDING_LIFE: Duration = Duration::from_secs(600);
const PENDING_ID: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Session {
    pub p: String,
    pub e: String,
    pub x: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub p: String,
    pub s: String,
    pub v: String,
    pub n: String,
    pub t: String,
    pub x: u64,
}

trait Expiring {
    fn until(&self) -> u64;
}

impl Expiring for Session {
    fn until(&self) -> u64 {
        self.x
    }
}

impl Expiring for Pending {
    fn until(&self) -> u64 {
        self.x
    }
}

pub(crate) fn deadline(life: Duration) -> u64 {
    now_secs() + life.as_secs()
}

pub(crate) fn pending_name(state: &str) -> String {
    let id: String = state.chars().take(PENDING_ID).collect();
    format!("{PENDING}{id}")
}

fn purpose(name: &str, host: &str) -> String {
    format!("{name}|{}", host.to_ascii_lowercase())
}

fn pairs(h: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    h.get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().split_once('='))
}

pub(crate) fn names(h: &HeaderMap) -> Vec<String> {
    pairs(h)
        .filter(|(n, _)| n.starts_with(OURS))
        .map(|(n, _)| n.to_string())
        .collect()
}

fn read<T: DeserializeOwned + Expiring>(
    s: &Sealer,
    h: &HeaderMap,
    name: &str,
    host: &str,
) -> Option<T> {
    let aad = purpose(name, host);
    let now = now_secs();
    pairs(h)
        .filter(|(n, _)| *n == name)
        .filter_map(|(_, v)| s.open(&aad, v))
        .filter_map(|b| serde_json::from_slice::<T>(&b).ok())
        .find(|t| t.until() > now)
}

pub(crate) fn session(s: &Sealer, h: &HeaderMap, host: &str) -> Option<Session> {
    read(s, h, SESSION, host)
}

pub(crate) fn pending(s: &Sealer, h: &HeaderMap, host: &str, state: &str) -> Option<Pending> {
    read::<Pending>(s, h, &pending_name(state), host).filter(|p| p.s == state)
}

fn header(text: String) -> HeaderValue {
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub(crate) fn set<T: Serialize>(
    s: &Sealer,
    name: &str,
    host: &str,
    value: &T,
    life: Duration,
) -> HeaderValue {
    let json = serde_json::to_vec(value).unwrap_or_default();
    let sealed = s.seal(&purpose(name, host), &json);
    header(format!(
        "{name}={sealed}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={}",
        life.as_secs()
    ))
}

pub(crate) fn clear(name: &str) -> HeaderValue {
    header(format!(
        "{name}=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
}

pub(crate) fn strip_ours(h: &mut HeaderMap) {
    if names(h).is_empty() {
        return;
    }
    let kept: Vec<String> = pairs(h)
        .filter(|(n, _)| !n.starts_with(OURS))
        .map(|(n, v)| format!("{n}={v}"))
        .collect();
    h.remove(COOKIE);
    if !kept.is_empty()
        && let Ok(v) = HeaderValue::from_str(&kept.join("; "))
    {
        h.insert(COOKIE, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_cookie(text: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_str(text).unwrap());
        h
    }

    fn value_of(set_cookie: &HeaderValue) -> String {
        let t = set_cookie.to_str().unwrap();
        t.split(';').next().unwrap().to_string()
    }

    #[test]
    fn a_session_cookie_is_secure_http_only_lax_and_read_back_only_on_its_host() {
        let s = Sealer::from_bytes(&[1; 32]);
        let sess = Session {
            p: "google".into(),
            e: "alice@gmail.com".into(),
            x: deadline(SESSION_LIFE),
        };
        let v = set(&s, SESSION, "Wiki.example.com", &sess, SESSION_LIFE);
        let text = v.to_str().unwrap();
        assert!(text.starts_with("__Host-burrow-session="), "{text}");
        assert!(
            text.ends_with("; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=43200"),
            "{text}"
        );
        let h = with_cookie(&format!("theme=dark; {}", value_of(&v)));
        assert_eq!(session(&s, &h, "wiki.example.com"), Some(sess.clone()));
        assert_eq!(session(&s, &h, "other.example.com"), None);
        let old = Session {
            x: now_secs() - 1,
            ..sess
        };
        let h = with_cookie(&value_of(&set(&s, SESSION, "w", &old, SESSION_LIFE)));
        assert_eq!(
            session(&s, &h, "w"),
            None,
            "an expired session is no session"
        );
    }

    #[test]
    fn a_pending_login_is_found_by_its_state_only() {
        let s = Sealer::from_bytes(&[2; 32]);
        let p = Pending {
            p: "google".into(),
            s: "abcdefghijklmnopqrstuvwxyz".into(),
            v: "verifier".into(),
            n: "nonce".into(),
            t: "/x".into(),
            x: deadline(PENDING_LIFE),
        };
        let name = pending_name(&p.s);
        assert_eq!(name, "__Host-burrow-login-abcdefghijklmnop");
        let h = with_cookie(&value_of(&set(&s, &name, "h", &p, PENDING_LIFE)));
        assert_eq!(pending(&s, &h, "h", &p.s), Some(p.clone()));
        assert_eq!(pending(&s, &h, "h", "abcdefghijklmnopXYZ"), None);
    }

    #[test]
    fn our_cookies_are_taken_out_before_the_app_sees_the_request() {
        let mut h = with_cookie("a=1; __Host-burrow-session=x; b=2");
        h.append(COOKIE, HeaderValue::from_static("__Host-burrow-login-q=y"));
        assert_eq!(
            names(&h),
            ["__Host-burrow-session", "__Host-burrow-login-q"]
        );
        strip_ours(&mut h);
        assert_eq!(h.get_all(COOKIE).iter().collect::<Vec<_>>(), ["a=1; b=2"]);
        let mut only = with_cookie("__Host-burrow-session=x");
        strip_ours(&mut only);
        assert!(only.get(COOKIE).is_none());
        let mut none = with_cookie("a=1");
        strip_ours(&mut none);
        assert_eq!(none[COOKIE], "a=1");
        assert_eq!(
            clear(SESSION).to_str().unwrap(),
            "__Host-burrow-session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
        );
    }
}
