use std::collections::BTreeMap;
use std::marker::PhantomData;

use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{Map, Value};

use crate::model::ServerConfig;

const MAX: usize = 6;

pub struct Field<P, T> {
    seg: [&'static str; MAX],
    len: usize,
    _t: PhantomData<fn() -> (P, T)>,
}

impl<P, T> Clone for Field<P, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P, T> Copy for Field<P, T> {}

impl<P, T> std::fmt::Debug for Field<P, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.segments()).finish()
    }
}

impl<P, T> PartialEq for Field<P, T> {
    fn eq(&self, other: &Self) -> bool {
        self.segments() == other.segments()
    }
}

impl<P, T> Eq for Field<P, T> {}

pub trait Nest {
    type Inner;
}

impl<T: Nest> Nest for Option<T> {
    type Inner = T::Inner;
}

pub trait Entry: Sized {
    const SECTION: &'static str;
    const ENTITY: &'static str;

    fn kind() -> Entity {
        Entity::parse(Self::ENTITY)
    }

    fn section() -> &'static str {
        Self::SECTION
    }

    fn entries(doc: &Value) -> Option<&Map<String, Value>> {
        doc.get(Self::SECTION).and_then(Value::as_object)
    }

    fn names(doc: &Value) -> Vec<String> {
        Self::entries(doc)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn entry<'a>(doc: &'a Value, name: &str) -> Option<&'a Value> {
        Self::entries(doc).and_then(|m| m.get(name))
    }

    fn path(name: &str) -> Path {
        vec![Self::section().to_string(), name.to_string()]
    }

    fn is_section(path: &[String]) -> bool {
        matches!(path, [s] if s == Self::section())
    }

    fn is_entry(path: &[String]) -> bool {
        matches!(path, [s, _] if s == Self::section())
    }
}

impl<P, T> Field<P, T> {
    #[doc(hidden)]
    pub const fn new(key: &'static str) -> Self {
        let mut seg = [""; MAX];
        seg[0] = key;
        Field {
            seg,
            len: 1,
            _t: PhantomData,
        }
    }

    pub const fn then<U>(self, next: Field<T::Inner, U>) -> Field<P, U>
    where
        T: Nest,
    {
        let mut seg = self.seg;
        let mut i = 0;
        while i < next.len {
            assert!(self.len + i < MAX, "a typed path is deeper than 6 segments");
            seg[self.len + i] = next.seg[i];
            i += 1;
        }
        Field {
            seg,
            len: self.len + next.len,
            _t: PhantomData,
        }
    }

    pub const fn key(self) -> &'static str {
        self.seg[self.len - 1]
    }

    pub fn segments(&self) -> &[&'static str] {
        &self.seg[..self.len]
    }

    pub fn rel(self) -> Path {
        self.segments().iter().map(|s| s.to_string()).collect()
    }

    pub fn ends(self, path: &[String]) -> bool {
        path.len() >= self.len
            && path[path.len() - self.len..]
                .iter()
                .zip(self.segments())
                .all(|(a, b)| a == b)
    }

    pub fn get(self, v: &Value) -> Option<&Value> {
        self.segments().iter().try_fold(v, |v, k| v.get(*k))
    }

    pub fn get_mut(self, v: &mut Value) -> Option<&mut Value> {
        self.segments().iter().try_fold(v, |v, k| v.get_mut(*k))
    }

    pub fn str(self, v: &Value) -> Option<&str> {
        self.get(v).and_then(Value::as_str)
    }

    pub fn set(self, v: &mut Value, to: Value) {
        let mut at = v;
        for k in self.segments() {
            if !at.is_object() {
                *at = Value::Object(Map::new());
            }
            at = at
                .as_object_mut()
                .expect("an object")
                .entry(k.to_string())
                .or_insert(Value::Null);
        }
        *at = to;
    }

    pub fn take(self, v: &mut Value) -> Option<Value> {
        let (last, parents) = self.segments().split_last()?;
        let mut at = v;
        for k in parents {
            at = at.get_mut(*k)?;
        }
        at.as_object_mut()?.remove(*last)
    }
}

impl<T> Field<ServerConfig, T> {
    pub fn path(self) -> Path {
        self.rel()
    }

    pub fn is(self, path: &[String]) -> bool {
        path.len() == self.len && self.ends(path)
    }
}

impl<P: Entry, T> Field<P, T> {
    pub fn of(self, name: &str) -> Path {
        let mut out = P::path(name);
        out.extend(self.rel());
        out
    }

    pub fn schema(self) -> Path {
        self.of("*")
    }

    pub fn is(self, path: &[String]) -> bool {
        path.len() == self.len + 2 && path[0] == P::section() && self.ends(path)
    }

    pub fn at<'a>(self, doc: &'a Value, name: &str) -> Option<&'a Value> {
        P::entry(doc, name).and_then(|e| self.get(e))
    }
}

impl<P: Entry, V> Field<P, BTreeMap<String, V>> {
    pub fn entry_of(self, name: &str, key: &str) -> Path {
        let mut out = self.of(name);
        out.push(key.to_string());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use burrow_admin_proto::serde_json::json;

    const PROTOCOL: &str = ServiceEntry::PROTOCOL.key();

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn typed_paths_follow_the_model() {
        assert_eq!(ServerConfig::LISTEN.path(), p(&["listen"]));
        assert_eq!(
            ServerConfig::SELF.then(SelfEntry::ALLOW).path(),
            p(&["self", "allow"])
        );
        assert_eq!(
            ServerConfig::LIMITS.then(Limits::MAX_HANDSHAKES).path(),
            p(&["limits", "max_handshakes"])
        );
        assert_eq!(
            ServerConfig::WEBSOCKET
                .then(WebSocketConfig::TLS)
                .then(TlsFiles::CERT)
                .path(),
            p(&["websocket", "tls", "cert"])
        );
        assert_eq!(
            ServiceEntry::BIND.of("web"),
            p(&["services", "web", "bind"])
        );
        assert_eq!(ServiceEntry::BIND.schema(), p(&["services", "*", "bind"]));
        assert_eq!(
            ServiceEntry::HEALTH_CHECK
                .then(HealthCheck::INTERVAL_SECS)
                .of("web"),
            p(&["services", "web", "health_check", "interval_secs"])
        );
        assert_eq!(
            VisitorEntry::REACH.entry_of("laptop", "home"),
            p(&["visitors", "laptop", "reach", "home"])
        );
        assert_eq!(VisitorEntry::ADMIN_FROM.rel(), p(&["admin_from"]));
        assert_eq!(PROTOCOL, "protocol");
        assert_eq!(ServiceEntry::section(), "services");
        assert_eq!(ClientEntry::kind(), Entity::Client);
        assert_eq!(VisitorEntry::path("ops"), p(&["visitors", "ops"]));
        assert!(VisitorEntry::is_entry(&p(&["visitors", "ops"])));
        assert!(!VisitorEntry::is_entry(&p(&["services", "ops"])));
        assert!(ServiceEntry::BIND.is(&ServiceEntry::BIND.of("x")));
        assert!(!ServiceEntry::BIND.is(&VisitorEntry::ADMIN.of("x")));
        assert!(!ServerConfig::LISTEN.is(&p(&["metrics", "listen"])));
        assert!(ServerConfig::LISTEN.ends(&p(&["metrics", "listen"])));
    }

    #[test]
    fn typed_paths_read_and_write_json_trees() {
        let mut doc = json!({
            "services": {"web": {"bind": 8080, "health_check": {"interval_secs": 5}}},
            "limits": {}
        });
        assert_eq!(ServiceEntry::BIND.at(&doc, "web"), Some(&json!(8080)));
        assert_eq!(ServiceEntry::BIND.at(&doc, "db"), None);
        assert_eq!(ServiceEntry::names(&doc), vec!["web".to_string()]);
        assert_eq!(VisitorEntry::names(&doc), Vec::<String>::new());
        let hc = ServiceEntry::HEALTH_CHECK.then(HealthCheck::INTERVAL_SECS);
        assert_eq!(hc.at(&doc, "web"), Some(&json!(5)));
        ServerConfig::LIMITS
            .then(Limits::MAX_PENDING)
            .set(&mut doc, json!(9));
        assert_eq!(doc["limits"]["max_pending"], 9);
        ServerConfig::POOL
            .then(Pool::MAX_IDLE)
            .set(&mut doc, json!(2));
        assert_eq!(doc["pool"]["max_idle"], 2);
        let web = &mut doc["services"]["web"];
        assert_eq!(ServiceEntry::BIND.take(web), Some(json!(8080)));
        assert_eq!(ServiceEntry::BIND.take(web), None);
        assert_eq!(
            ServiceEntry::TARGET.str(&json!({"target": "x:1"})),
            Some("x:1")
        );
    }

    #[test]
    fn value_enums_know_their_wire_names() {
        assert_eq!(Protocol::Udp.wire(), "udp");
        assert_eq!(Protocol::from_wire("tcp"), Some(Protocol::Tcp));
        assert_eq!(Protocol::from_wire("sctp"), None);
        assert_eq!(Protocol::ALL, [Protocol::Tcp, Protocol::Udp]);
        assert_eq!(Mode::Balance.wire(), "balance");
        assert_eq!(ProxyVersion::from_wire("v2"), Some(ProxyVersion::V2));
        for m in Mode::ALL {
            assert_eq!(
                burrow_admin_proto::serde_json::to_value(m).unwrap(),
                json!(m.wire())
            );
        }
    }
}
