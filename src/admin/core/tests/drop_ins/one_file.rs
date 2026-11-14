#![cfg(test)]
#![allow(clippy::disallowed_macros)]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use burrow_admin_proto::features::CONFIG_FILES;
use burrow_admin_proto::msg::{AdminLevel, req};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::text_edit::{EditText, TextFile};
use serde_json::json;

use super::super::{FakeLive, p, session, world};
use crate::admin::adapters::FileStore;
use crate::admin::core::plan;
use crate::admin::core::{Core, dispatch, text};
use crate::wire::secret::Secret;

fn big(world: &str, n: usize) -> String {
    let mut clients = String::from("  # the fleet, one client per service\n");
    let mut services = String::new();
    for i in 0..n {
        clients.push_str(&format!(
            "  host-{i}:\n    secret: {} # rotated in March\n    max_connections: 200\n",
            Secret::generate().encode()
        ));
        services.push_str(&format!(
            "  app-{i}: # app {i}\n    client: host-{i}\n    bind: {}\n    target: 127.0.0.1:{}\n    allow_from: [10.0.0.0/8, 192.168.0.0/16]\n",
            20_000 + i,
            3000 + i % 1000
        ));
    }
    let text = world.replacen("\nvisitors:", &format!("{clients}\nvisitors:"), 1);
    format!("{text}{services}")
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "a measurement: cargo test --release -p burrow --lib one_big_file -- --ignored --nocapture"]
fn one_big_file_with_n_clients_and_services() {
    eprintln!(
        "\n| N | bytes | check_docs | config_text | form plan | text plan |\n|---|---|---|---|---|---|"
    );
    for n in [1000, 2000, 4000] {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("server.yaml");
        let body = big(&w.text, n);
        crate::base::fs::write_private(&main, &body).unwrap();
        let live = FakeLive {
            text: Arc::new(Mutex::new(body.clone())),
            status: Mutex::new(Default::default()),
            sessions: Mutex::new(Vec::new()),
            busy: Vec::new(),
            allows: Default::default(),
        };
        let core = Core::new(
            Box::new(FileStore::new(&main)),
            Box::new(live),
            Box::new(|_: &str| None),
        );
        let s = session(&w, AdminLevel::Write);
        s.hello(None, vec![CONFIG_FILES.to_string()]);
        let base = text::docs_rev(&core.docs().unwrap());
        let t = Instant::now();
        plan::check_docs(&core, &core.docs().unwrap());
        let check_ms = ms(t);
        let t = Instant::now();
        let reply = dispatch(&core, &s, req::CONFIG_TEXT, &json!({})).unwrap();
        let shown_ms = ms(t);
        let form = [Op::Set {
            path: p(&["services", "app-7", "bind"]),
            value: json!(29_999),
        }];
        let t = Instant::now();
        let c = plan::compute(&core, &s, &base, &form).unwrap();
        let form_ms = ms(t);
        assert!(
            c.plan.valid,
            "{:#?}",
            c.plan.diags.iter().take(3).collect::<Vec<_>>()
        );
        let edit: EditText = serde_json::from_value(reply).unwrap();
        let ops = [Op::ReplaceText {
            files: vec![TextFile {
                name: edit.files[0].name.clone(),
                text: edit.files[0].text.replace("bind: 20007", "bind: 29999"),
            }],
        }];
        let t = Instant::now();
        let c = plan::compute(&core, &s, &base, &ops).unwrap();
        let text_ms = ms(t);
        assert!(
            c.plan.valid,
            "{:#?}",
            c.plan.diags.iter().take(3).collect::<Vec<_>>()
        );
        eprintln!(
            "| {n} | {} | {check_ms:.0} ms | {shown_ms:.0} ms | {form_ms:.0} ms | {text_ms:.0} ms |",
            body.len()
        );
    }
}
