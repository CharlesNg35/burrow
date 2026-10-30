use burrow_admin_proto::schema::{Node, Relation};

pub fn reason_of(node: &Node) -> Option<&str> {
    node.relations.iter().find_map(|r| match r {
        Relation::EnabledIf { reason, .. } => Some(reason.as_str()),
        _ => None,
    })
}

pub fn group_line(n: usize, reason: &str) -> String {
    match reason.strip_prefix("This needs ") {
        Some(rest) => format!("{n} settings need {rest}"),
        None => match reason.strip_prefix("This applies ") {
            Some(rest) => format!("{n} settings apply {rest}"),
            None => format!("{n} settings: {reason}"),
        },
    }
}

pub fn groups<T>(items: &[(Option<String>, T)]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    let mut from = 0;
    for (i, (reason, _)) in items.iter().enumerate() {
        let Some(r) = reason else {
            from = out.len();
            continue;
        };
        match out[from..].iter_mut().find(|(k, _)| k == r) {
            Some((_, v)) => v.push(i),
            None => out.push((r.clone(), vec![i])),
        }
    }
    out.retain(|(_, v)| v.len() >= 2);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_that_share_a_reason_are_one_group() {
        assert_eq!(
            group_line(
                3,
                "This needs burrow to do the HTTPS; this site uses passthrough"
            ),
            "3 settings need burrow to do the HTTPS; this site uses passthrough"
        );
        assert_eq!(
            group_line(2, "This applies only with two or more clients"),
            "2 settings apply only with two or more clients"
        );
        let items = [
            (Some("a".to_string()), 0),
            (None, 1),
            (Some("a".to_string()), 2),
            (Some("b".to_string()), 3),
            (Some("a".to_string()), 4),
        ];
        assert_eq!(
            groups(&items),
            [("a".to_string(), vec![2, 4])],
            "a field that always applies ends a group, so a field placed before it keeps its place"
        );
    }

    #[test]
    fn a_passthrough_site_has_one_reason_for_most_of_its_settings() {
        use burrow_config::admin_schema::AdminSchema;
        let site = burrow_config::WebSite::node();
        let items: Vec<(Option<String>, ())> = crate::tree::fields(&site)
            .iter()
            .map(|f| (reason_of(&f.node).map(str::to_string), ()))
            .collect();
        let g = groups(&items);
        let passthrough = g
            .iter()
            .find(|(r, _)| r.ends_with("this site uses passthrough"))
            .expect("a passthrough group");
        assert!(passthrough.1.len() >= 10, "{g:?}");
    }
}
