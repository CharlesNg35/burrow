use std::sync::OnceLock;

use burrow_admin_proto::schema::{Node, SemType};

use crate::admin_schema::{AdminEnum, AdminSchema};
use crate::model::ServerConfig;

fn root() -> &'static Node {
    static ROOT: OnceLock<Node> = OnceLock::new();
    ROOT.get_or_init(ServerConfig::node)
}

fn object_of(node: &Node) -> Option<&Node> {
    match &node.sem {
        SemType::Object(_) => Some(node),
        SemType::Optional(o) => object_of(&o.inner),
        SemType::Union(u) => u.branches.iter().find_map(object_of),
        _ => None,
    }
}

pub fn label_in(root: &Node, path: &[String]) -> Option<String> {
    let (key, parent) = path.split_last()?;
    let parent = object_of(root.at(parent)?)?;
    let SemType::Object(o) = &parent.sem else {
        return None;
    };
    o.fields
        .iter()
        .find(|f| &f.key == key)
        .map(|f| f.node.label.clone())
        .filter(|l| !l.is_empty())
}

pub fn label_of(path: &[String]) -> Option<String> {
    label_in(root(), path)
}

pub fn option_label<T: AdminEnum>(wire: &str) -> Option<String> {
    T::options()
        .into_iter()
        .find(|o| o.value == wire)
        .map(|o| o.label)
}

pub fn app_https() -> String {
    let label = option_label::<crate::model::TlsMode>(crate::model::TlsMode::Passthrough.wire())
        .unwrap_or_default();
    let mut c = label.chars();
    c.next()
        .map(|f| f.to_lowercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: &[&str]) -> Vec<String> {
        x.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_field_is_named_by_its_admin_label_and_a_map_key_is_not_a_field() {
        assert_eq!(
            label_of(&p(&["services", "web", "bind"])).as_deref(),
            Some("Public address")
        );
        assert_eq!(label_of(&p(&["listen"])).as_deref(), Some("Listen address"));
        assert_eq!(label_of(&p(&["visitors", "lap", "reach", "home"])), None);
        assert_eq!(label_of(&p(&["services", "web", "nope"])), None);
        assert_eq!(app_https(), "the app does its own HTTPS");
    }
}
