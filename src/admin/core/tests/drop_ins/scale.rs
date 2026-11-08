#![cfg(test)]
#![allow(clippy::disallowed_macros)]

use std::time::Instant;

use burrow_admin_proto::msg::{Apply, req};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::text_edit::{EditText, TextFile};
use serde_json::json;

use super::Set;
use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::{dispatch, text};
use crate::wire::secret::Secret;

const FILES: usize = 100;
const SERVICES_PER_FILE: usize = 10;

fn drop_in(f: usize) -> String {
    let mut out = format!("# team {f}: kept by the platform group\nclients:\n");
    for c in 0..2 {
        out.push_str(&format!(
            "  host-{f}-{c}:\n    secret: {} # rotated in March\n    max_connections: 200\n",
            Secret::generate().encode()
        ));
    }
    out.push_str(&format!(
        "visitors:\n  dev-{f}:\n    secret: {}\n    reach:\n      host-{f}-0: any\n",
        Secret::generate().encode()
    ));
    out.push_str("services:\n");
    for s in 0..SERVICES_PER_FILE {
        let n = f * SERVICES_PER_FILE + s;
        out.push_str(&format!(
            "  app-{n}: # the {s}th app of team {f}\n    client: host-{f}-{}\n    bind: {}\n    target: 127.0.0.1:{}\n    max_connections: 100\n    allow_from: [10.0.0.0/8, 192.168.0.0/16]\n",
            s % 2,
            20_000 + n,
            3000 + s
        ));
    }
    out
}

fn ms(t: Instant) -> String {
    format!("{:.1} ms", t.elapsed().as_secs_f64() * 1000.0)
}

#[test]
#[ignore = "a measurement: cargo test --release -p burrow --lib scale -- --ignored --nocapture"]
fn a_hundred_drop_ins_with_a_thousand_services() {
    let names: Vec<String> = (0..FILES).map(|f| format!("team-{f:03}.yaml")).collect();
    let texts: Vec<String> = (0..FILES).map(drop_in).collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .map(String::as_str)
        .zip(texts.iter().map(String::as_str))
        .collect();
    let set = Set::new(&files);
    let s = set.session(true);
    let mut rows = Vec::new();
    let docs = set.core.docs().unwrap();
    let bytes: usize = docs.list.iter().map(|d| d.text.len()).sum();
    rows.push((
        "files on disk".to_string(),
        format!("{} files, {bytes} bytes", docs.list.len()),
    ));
    let t = Instant::now();
    let checked = plan::check_docs(&set.core, &docs);
    rows.push(("check_docs".into(), ms(t)));
    assert!(
        checked.valid,
        "{:#?}",
        checked
            .diags
            .iter()
            .filter(|d| d.is_error())
            .take(5)
            .collect::<Vec<_>>()
    );
    let t = Instant::now();
    let reply = dispatch(&set.core, &s, req::CONFIG_TEXT, &json!({})).unwrap();
    let took = ms(t);
    let size = serde_json::to_vec(&reply).unwrap().len();
    rows.push((
        "config_text (first)".into(),
        format!("{took}, {size} bytes"),
    ));
    let t = Instant::now();
    dispatch(&set.core, &s, req::CONFIG_TEXT, &json!({})).unwrap();
    rows.push(("config_text (again)".into(), ms(t)));
    let edit: EditText = serde_json::from_value(reply).unwrap();
    for query in ["target", "app-999", "no such text"] {
        let t = Instant::now();
        let r = dispatch(
            &set.core,
            &s,
            req::SEARCH_TEXT,
            &json!({ "query": query, "limit": 200 }),
        )
        .unwrap();
        let n = r["matches"].as_array().map_or(0, Vec::len);
        rows.push((
            format!("search_text {query:?}"),
            format!("{}, {n} matches", ms(t)),
        ));
    }
    let one = edit
        .files
        .iter()
        .find(|f| f.name.ends_with("team-050.yaml"))
        .unwrap();
    let ops = vec![Op::ReplaceText {
        files: vec![TextFile {
            name: one.name.clone(),
            text: one
                .text
                .replace("target: 127.0.0.1:3000", "target: 127.0.0.1:3100"),
        }],
    }];
    let base = text::docs_rev(&docs);
    for label in ["validate, one file (live check)", "validate again (Review)"] {
        let t = Instant::now();
        let c = plan::compute(&set.core, &s, &base, &ops).unwrap();
        let took = ms(t);
        let size = serde_json::to_vec(&c.plan).unwrap().len();
        assert!(
            c.plan.valid,
            "{:#?}",
            c.plan
                .diags
                .iter()
                .filter(|d| d.is_error())
                .take(5)
                .collect::<Vec<_>>()
        );
        rows.push((label.into(), format!("{took}, plan {size} bytes")));
    }
    let plan = plan::compute(&set.core, &s, &base, &ops).unwrap().plan;
    let t = Instant::now();
    let begun = plan::apply_begin(
        &set.core,
        &s,
        &Apply {
            base,
            ops: ops.clone(),
            plan: plan.plan.clone(),
            confirm: plan.confirms().iter().map(|c| c.to_string()).collect(),
            address: None,
        },
    )
    .unwrap();
    plan::finish(&set.core, &s, begun, ReloadResult::unchanged(), None).unwrap();
    rows.push(("apply (plan, write, history)".into(), ms(t)));
    eprintln!("\n| step | result |\n|---|---|");
    for (k, v) in rows {
        eprintln!("| {k} | {v} |");
    }
}
