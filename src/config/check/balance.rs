use std::collections::BTreeMap;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use burrow_admin_proto::codes;
use burrow_config::{Mode, ServiceEntry, WeightedClient};

pub(super) fn check_weights(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, u32> {
    let mut weights = BTreeMap::new();
    for (i, c) in svc.clients.iter().enumerate() {
        if c.given_weight().is_none() {
            continue;
        }
        let at = i.to_string();
        let place = [
            "services",
            name,
            "clients",
            &at,
            WeightedClient::WEIGHT.key(),
        ];
        let client = c.name();
        if svc.mode != Mode::Balance {
            out.push(
                Diagnostic::error(
                    codes::WEIGHT_NOT_BALANCE,
                    format!("service \"{name}\" gives client \"{client}\" a weight, but weights only apply with mode: balance"),
                )
                .at(spans.value(&place))
                .label("only used by mode: balance")
                .help("remove the weight, or set mode: balance"),
            );
        } else if c.weight() != 1 {
            weights.insert(client.to_string(), c.weight());
        }
    }
    weights
}
