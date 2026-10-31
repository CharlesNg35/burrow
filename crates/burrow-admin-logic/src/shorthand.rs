use burrow_admin_proto::schema::{Field, SemType, UnionC};
use burrow_admin_proto::serde_json::{Map, Value};

use crate::tree::inner;

#[derive(Debug, Clone, PartialEq)]
pub struct Shorthand {
    pub text: Field,
    pub flags: Vec<Field>,
}

pub fn of(u: &UnionC) -> Option<Shorthand> {
    if u.branches.len() != 2 {
        return None;
    }
    let short = u
        .branches
        .iter()
        .any(|b| matches!(inner(b).sem, SemType::Text(_)));
    let fields = u.branches.iter().find_map(|b| match &inner(b).sem {
        SemType::Object(o) => Some(o.fields.clone()),
        _ => None,
    })?;
    let (text, flags) = fields.split_first()?;
    let plain = matches!(inner(&text.node).sem, SemType::Text(_));
    let bools = flags
        .iter()
        .all(|f| matches!(inner(&f.node).sem, SemType::Bool));
    (short && plain && bools).then(|| Shorthand {
        text: text.clone(),
        flags: flags.to_vec(),
    })
}

impl Shorthand {
    pub fn text_of(&self, v: Option<&Value>) -> String {
        match v {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Object(m)) => m
                .get(&self.text.key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    pub fn flag_of(&self, v: Option<&Value>, key: &str) -> bool {
        v.and_then(|v| v.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn compose(&self, text: &str, on: &[&str]) -> Value {
        let text = text.trim();
        if on.is_empty() {
            return Value::String(text.to_string());
        }
        let mut m = Map::new();
        m.insert(self.text.key.clone(), Value::String(text.to_string()));
        for f in self.flags.iter().filter(|f| on.contains(&f.key.as_str())) {
            m.insert(f.key.clone(), Value::Bool(true));
        }
        Value::Object(m)
    }

    pub fn set_text(&self, v: Option<&Value>, text: &str) -> Value {
        let on = self.on(v);
        self.compose(text, &on.iter().map(String::as_str).collect::<Vec<_>>())
    }

    pub fn set_flag(&self, v: Option<&Value>, key: &str, value: bool) -> Value {
        let mut on = self.on(v);
        on.retain(|k| k != key);
        if value {
            on.push(key.to_string());
        }
        let text = self.text_of(v);
        self.compose(&text, &on.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn on(&self, v: Option<&Value>) -> Vec<String> {
        self.flags
            .iter()
            .filter(|f| self.flag_of(v, &f.key))
            .map(|f| f.key.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_config::admin_schema::AdminSchema;

    fn redirect() -> Shorthand {
        match burrow_config::RedirectTarget::node().sem {
            SemType::Union(u) => of(&u).expect("a shorthand union"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_redirect_is_a_text_with_a_permanent_flag() {
        let s = redirect();
        assert_eq!(s.text.key, burrow_config::RedirectRule::TO.key());
        assert_eq!(s.flags.len(), 1);
        let permanent = burrow_config::RedirectRule::PERMANENT.key();
        let to = json!("https://example.com");
        assert_eq!(s.text_of(Some(&to)), "https://example.com");
        assert!(!s.flag_of(Some(&to), permanent));
        let on = s.set_flag(Some(&to), permanent, true);
        assert_eq!(on, json!({"to": "https://example.com", "permanent": true}));
        assert_eq!(s.text_of(Some(&on)), "https://example.com");
        let moved = s.set_text(Some(&on), "https://example.net");
        assert_eq!(
            moved,
            json!({"to": "https://example.net", "permanent": true})
        );
        assert_eq!(
            s.set_flag(Some(&moved), permanent, false),
            json!("https://example.net"),
            "back to the short form"
        );
        assert_eq!(
            s.set_text(Some(&to), "  "),
            json!(""),
            "an emptied target stays, so its row and error stay; Remove takes it out"
        );
    }
}
