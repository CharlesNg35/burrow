use burrow_admin_proto::plan::{Change, ChangeKind, Plan};

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub head: Change,
    pub parts: Vec<Change>,
}

pub fn change_groups(plan: &Plan) -> Vec<Group> {
    let mut out: Vec<Group> = plan
        .changes
        .iter()
        .filter(|c| {
            c.part_of
                .as_ref()
                .is_none_or(|g| !plan.changes.iter().any(|h| h.path == *g))
        })
        .map(|c| Group {
            head: c.clone(),
            parts: Vec::new(),
        })
        .collect();
    for c in &plan.changes {
        if let Some(g) = c
            .part_of
            .as_ref()
            .and_then(|g| out.iter_mut().find(|x| x.head.path == *g))
        {
            g.parts.push(c.clone());
        }
    }
    let entity = |g: &Group| {
        g.head
            .path
            .first()
            .is_some_and(|s| crate::route::SECTIONS.contains(&s.as_str()))
    };
    let settings: Vec<Group> = out.iter().filter(|g| !entity(g)).cloned().collect();
    if settings.len() > 1 {
        out.retain(entity);
        let names: Vec<String> = settings.iter().map(|g| g.head.path.join(".")).collect();
        out.push(Group {
            head: Change {
                kind: ChangeKind::Set,
                path: vec!["settings".into()],
                summary: format!("Server settings: {}", names.join(", ")),
                part_of: None,
            },
            parts: settings.into_iter().map(|g| g.head).collect(),
        });
    }
    out
}
