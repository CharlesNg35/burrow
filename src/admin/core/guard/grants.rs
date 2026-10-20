use burrow_admin_logic::grant::{Grant, Power, grants};
use burrow_admin_proto::msg::Level;
use burrow_admin_proto::plan::Refusal;

use super::{Change, refusal};
use crate::config::settings::VisitorAuth;
use crate::secret::Secret;

fn power(v: &VisitorAuth) -> Power<&Secret> {
    Power {
        level: v.admin.map(Level::from),
        secrets: v.secrets.iter().collect(),
        from: v.admin_from.clone(),
    }
}

fn level(l: Level) -> &'static str {
    match l {
        Level::Read => "read",
        Level::Write => "write",
    }
}

fn what(name: &str, renamed_from: Option<&str>, g: &Grant<&Secret>) -> String {
    match (g, renamed_from) {
        (Grant::Admin(_), Some(old)) => format!(
            "an admin visitor is renamed only in the config file: visitor \"{old}\" would become \"{name}\""
        ),
        (Grant::Admin(l), None) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would get admin: {}",
            level(*l)
        ),
        (Grant::Upgrade, _) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would go from admin: read to admin: write"
        ),
        (Grant::SecretAdded(s), _) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would get the new secret {}",
            s.fingerprint()
        ),
        (Grant::FromWidened(Some(c)), _) => format!(
            "admin access is granted only in the config file: admin_from of visitor \"{name}\" would admit {c}, which the current list does not"
        ),
        (Grant::FromWidened(None), _) => format!(
            "admin access is granted only in the config file: admin_from of visitor \"{name}\" would be removed, which admits every address"
        ),
    }
}

pub fn refusals(c: &Change) -> Vec<Refusal> {
    let file = c.path.display();
    let mut out = Vec::new();
    for (name, v) in &c.next.visitors {
        let old = c.file.visitors.get(name).map(power);
        let new = power(v);
        let renamed_from = c
            .renames
            .iter()
            .find(|r| r.to == *name && r.path.len() == 2 && r.path[0] == "visitors")
            .map(|r| r.path[1].as_str());
        for g in grants(old.as_ref(), &new) {
            out.push(refusal(
                g.code(),
                format!(
                    "{}; use burrow edit -c {file} or edit the file",
                    what(name, renamed_from, &g)
                ),
            ));
        }
    }
    out
}
