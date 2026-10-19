use burrow_admin_proto::schema::{Node, SemType};
use burrow_admin_proto::serde_json::Value;

pub const GRAMMAR: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Render<'a> {
    Known(&'a SemType),
    Fallback(&'a SemType),
    ServerChecked,
    Raw,
}

impl Render<'_> {
    pub fn local_checks(&self) -> bool {
        matches!(self, Render::Known(_) | Render::Fallback(_))
    }
}

fn fits(sem: &SemType, v: Option<&Value>) -> bool {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return true;
    };
    match sem {
        SemType::Object(_) | SemType::Map(_) | SemType::RefMap(_) => v.is_object(),
        SemType::List(_) => v.is_array(),
        SemType::RefList(_) | SemType::SecretRef(_) => {
            v.is_array() || v.is_string() || v.is_object()
        }
        SemType::Bool => v.is_boolean(),
        SemType::Optional(o) => fits(&o.inner.sem, Some(v)),
        SemType::Union(u) => u.branches.iter().any(|b| fits(&b.sem, Some(v))),
        SemType::Unknown(_) => false,
        _ => v.is_string() || v.is_number(),
    }
}

pub fn render_value<'a>(sem: &'a SemType, v: Option<&Value>) -> Render<'a> {
    match sem {
        SemType::Unknown(u) => u
            .fallback
            .iter()
            .find(|f| !matches!(f, SemType::Unknown(_)) && fits(f, v))
            .map(Render::Fallback)
            .unwrap_or_else(|| match v.filter(|v| !v.is_null()) {
                None if u.fallback.is_empty() => Render::Raw,
                Some(v) if v.is_object() || v.is_array() => Render::Raw,
                _ if sem.is_scalar() => Render::ServerChecked,
                _ => Render::Raw,
            }),
        known => Render::Known(known),
    }
}

pub fn render(sem: &SemType) -> Render<'_> {
    render_value(sem, None)
}

pub fn for_node<'a>(node: &'a Node, v: Option<&Value>) -> Render<'a> {
    match render_value(&node.sem, v) {
        Render::Known(s) | Render::Fallback(s) if node.grammar > GRAMMAR && s.is_scalar() => {
            Render::ServerChecked
        }
        r => r,
    }
}

pub fn needs_newer_ui(node: &Node) -> bool {
    match &node.sem {
        SemType::Unknown(_) => true,
        SemType::Object(o) => o.fields.iter().any(|f| needs_newer_ui(&f.node)),
        SemType::Map(m) => needs_newer_ui(&m.value),
        SemType::RefMap(m) => needs_newer_ui(&m.value),
        SemType::List(l) => needs_newer_ui(&l.item),
        SemType::Optional(o) => needs_newer_ui(&o.inner),
        SemType::Union(u) => u.branches.iter().any(needs_newer_ui),
        _ => node.grammar > GRAMMAR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::schema::{ObjectC, UnknownType};
    use burrow_admin_proto::serde_json::{self, json};

    fn unknown(raw: Value) -> SemType {
        serde_json::from_value(raw).expect("parses")
    }

    #[test]
    fn a_newer_type_uses_the_first_fallback_that_fits_the_value() {
        let sem = unknown(json!({"t": "rate_v2", "fallback": [{"t": "rate"}, {"t": "text"}]}));
        assert_eq!(render(&sem), Render::Fallback(&SemType::Rate));
        let obj = unknown(
            json!({"t": "geo", "fallback": [{"t": "text"}, {"t": "object", "fields": []}]}),
        );
        assert!(matches!(
            render_value(&obj, Some(&json!({"lat": 1}))),
            Render::Fallback(SemType::Object(_))
        ));
        assert!(matches!(
            render_value(&obj, Some(&json!("x"))),
            Render::Fallback(SemType::Text(_))
        ));
    }

    #[test]
    fn without_a_fitting_fallback_scalars_are_server_checked_and_structures_raw() {
        let sem = unknown(json!({"t": "rate_v3"}));
        assert_eq!(
            render_value(&sem, Some(&json!("9x"))),
            Render::ServerChecked
        );
        assert_eq!(render_value(&sem, Some(&json!({"a": 1}))), Render::Raw);
        let nested = unknown(json!({"t": "matrix", "fallback": [{"t": "future_kind"}]}));
        assert_eq!(render_value(&nested, Some(&json!([1, 2]))), Render::Raw);
        assert!(!Render::Raw.local_checks());
        assert_eq!(
            render_value(&sem, None),
            Render::Raw,
            "nothing tells its shape"
        );
        let scalarish = unknown(json!({"t": "geo", "fallback": [{"t": "future_kind"}]}));
        assert_eq!(render_value(&scalarish, None), Render::ServerChecked);
    }

    #[test]
    fn a_widened_grammar_skips_the_local_parser() {
        let mut node = Node::new(SemType::Rate);
        assert_eq!(for_node(&node, None), Render::Known(&SemType::Rate));
        assert!(!needs_newer_ui(&node));
        node.grammar = 2;
        assert_eq!(for_node(&node, None), Render::ServerChecked);
        assert!(needs_newer_ui(&node));
        let outer = Node::new(SemType::Object(ObjectC {
            fields: vec![burrow_admin_proto::schema::Field {
                key: "x".into(),
                node: Node::new(SemType::Unknown(UnknownType {
                    t: "x".into(),
                    fallback: vec![],
                    raw: json!({"t": "x"}),
                })),
            }],
        }));
        assert!(needs_newer_ui(&outer));
    }
}
