use burrow_admin_proto::Path;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::serde_json::Value;
use burrow_config::Entry;
use burrow_config::renames::{origins, within};

use super::Changeset;

impl Changeset {
    pub fn changed_entities(&self) -> usize {
        let ops = self.ops();
        let sections = [
            burrow_config::ClientEntry::section(),
            burrow_config::VisitorEntry::section(),
            burrow_config::ServiceEntry::section(),
        ];
        let mut seen: Vec<(Path, bool)> = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            let Some(p) = op.path() else {
                continue;
            };
            let p: Path = p.iter().take(2).cloned().collect();
            let [section, name] = p.as_slice() else {
                if burrow_config::refs::part_of(&self.base, &self.draft, &p, &self.renames)
                    .is_none()
                {
                    seen.push((Vec::new(), true));
                }
                continue;
            };
            let earlier = ops[..i].iter().filter_map(|o| match o {
                Op::Rename { path, to, .. } => Some((path.as_slice(), to.as_str())),
                _ => None,
            });
            let base = self.base.get(section).and_then(Value::as_object);
            let names = origins(
                base.into_iter().flat_map(|m| m.keys().map(String::as_str)),
                within(section, earlier),
            );
            let (id, known) = match names.get(name) {
                Some(o) if o.base => (vec![section.clone(), o.name.clone()], true),
                _ => (p.clone(), false),
            };
            if burrow_config::refs::part_of(&self.base, &self.draft, &id, &self.renames).is_some() {
                continue;
            }
            let entity = sections.contains(&section.as_str());
            seen.push(if entity {
                (id, known)
            } else {
                (Vec::new(), true)
            });
        }
        seen.sort();
        seen.dedup();
        seen.len()
    }
}
