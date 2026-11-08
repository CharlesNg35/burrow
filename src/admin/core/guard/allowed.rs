use std::collections::BTreeMap;
use std::net::SocketAddr;

use burrow_admin_proto::codes::TARGET_NOT_ALLOWED;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_config::ServiceEntry;

use crate::config::settings::{ServerSettings, Service};
use crate::policy::allowlist::AllowList;

fn list_of(given: &[String]) -> Option<AllowList> {
    if given.iter().any(|r| r == burrow_syntax::rule::ANY) {
        return None;
    }
    let list = AllowList::from_rules(given);
    match &list {
        AllowList::Rules(r) if r.len() == given.len() => Some(list),
        _ => None,
    }
}

fn refused(
    svc: &Service,
    entry: &crate::config::settings::Entry,
    allow: &AllowList,
) -> Option<String> {
    let base = svc.target_of(entry);
    if let Some(path) = base.strip_prefix("unix:") {
        return (!allow.permits_socket(path)).then(|| base.to_string());
    }
    let last = svc.ports.max(1) - 1;
    [0, last]
        .into_iter()
        .filter_map(|i| svc.entry_target_at(entry, i).parse::<SocketAddr>().ok())
        .find(|a| !allow.permits(*a))
        .map(|a| a.to_string())
}

pub fn not_allowed(
    allows: &BTreeMap<String, Vec<String>>,
    running: &ServerSettings,
    next: &ServerSettings,
) -> Vec<Diag> {
    let mut out = Vec::new();
    for (name, svc) in &next.services {
        if running.services.get(name) == Some(svc) {
            continue;
        }
        for entry in &svc.entries {
            let Some(allow) = allows.get(&entry.client).and_then(|g| list_of(g)) else {
                continue;
            };
            let Some(target) = refused(svc, entry, &allow) else {
                continue;
            };
            let client = &entry.client;
            out.push(Diag {
                severity: Severity::Warning,
                code: TARGET_NOT_ALLOWED.into(),
                message: format!(
                    "services.{name}.target: client \"{client}\" does not allow {target}, so it refuses this service"
                ),
                path: Some(ServiceEntry::TARGET.of(name)),
                also: Vec::new(),
                subjects: vec![client.clone()],
                label: None,
                help: Some(format!(
                    "add the target to allow in {client}'s client.yaml, or print that file again with: {}",
                    crate::base::failure::cmd(&format!("client-config {client}"))
                )),
                line: None,
                col: None,
                end_line: None,
                end_col: None,
                before: false,
                file: None,
            });
        }
    }
    out
}
