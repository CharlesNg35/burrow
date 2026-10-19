use std::process::ExitCode;

use burrow::config::{Loaded, Role, load_file};
use burrow::dialer::Dialer;
use burrow::dialer::admin::{AdminRx, AdminTx};
use burrow_admin_proto::frame::AdminOpen;
use burrow_admin_proto::msg::Incoming;
use serde_json::{Value, json};

struct Probe {
    rx: AdminRx,
    tx: AdminTx,
    next: u32,
}

impl Probe {
    async fn call(&mut self, t: &str, body: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let msg = json!({"id": id, "t": t, "body": body});
        self.tx
            .send(msg.to_string().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let m = self
                .rx
                .recv()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("the server closed the admin session")?;
            match Incoming::parse(&String::from_utf8_lossy(&m)) {
                Some(Incoming::Reply { re, result }) if re == id => {
                    return result.map_err(|e| format!("{}: {}", e.code.as_str(), e.message));
                }
                _ => {}
            }
        }
    }

    async fn apply(&mut self, ops: Value, confirm: &[String]) -> Result<Value, String> {
        let state = self.call("state", json!({})).await?;
        let plan = self
            .call("validate", json!({"base": state["rev"], "ops": ops}))
            .await?;
        if plan["valid"] != true || !plan["refusals"].as_array().is_none_or(Vec::is_empty) {
            return Err(format!("the plan is not applicable: {plan}"));
        }
        self.call(
            "apply",
            json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": confirm}),
        )
        .await
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    let [config, action, rest @ ..] = args.as_slice() else {
        return Err(
            "usage: admin_probe VISITOR_CONFIG state | apply OPS_JSON [CONFIRM...] | soak COUNT"
                .into(),
        );
    };
    let env = burrow::run::Env::default();
    let outcome = load_file(std::path::Path::new(config), Some(Role::Visitor), &env)
        .map_err(|e| e.describe(std::path::Path::new(config)).0)?;
    let Some(Loaded::Visitor(settings, _)) = outcome.loaded else {
        return Err(format!("{config} is not a valid visitor config"));
    };
    let open = AdminOpen {
        min: 1,
        max: 1,
        client: "admin_probe".into(),
        features: Vec::new(),
    };
    let (rx, tx, _) = Dialer::visitor(&settings)
        .open_admin(&open)
        .await
        .map_err(|e| format!("cannot open an admin session: {e:?}"))?;
    let mut p = Probe { rx, tx, next: 0 };
    p.call("welcome", json!({})).await?;
    match (action.as_str(), rest) {
        ("state", []) => {
            println!("{}", p.call("state", json!({})).await?);
        }
        ("apply", [ops, confirm @ ..]) => {
            let ops: Value = serde_json::from_str(ops).map_err(|e| e.to_string())?;
            println!("{}", p.apply(ops, confirm).await?);
        }
        ("soak", [count]) => {
            let count: u32 = count.parse().map_err(|_| "COUNT must be a number")?;
            for i in 0..count {
                let applied = p
                    .apply(
                        json!([{"op": "set", "path": ["limits", "max_pending"], "value": 600 + (i % 2)}]),
                        &[],
                    )
                    .await?;
                let state = p.call("state", json!({})).await?;
                if state["rev"] != applied["rev"] {
                    return Err(format!(
                        "cycle {i}: the file is not at the applied revision"
                    ));
                }
                let history = p.call("history", json!({"limit": 2})).await?;
                let before = history["entries"][0]["base"].clone();
                let plan = p
                    .call("revert", json!({"to": before, "base": state["rev"]}))
                    .await?;
                if plan["valid"] != true {
                    return Err(format!("cycle {i}: the revert plan is invalid: {plan}"));
                }
                p.call(
                    "apply",
                    json!({"base": plan["base"], "ops": [{"op": "replace_document", "from_history": before}], "plan": plan["plan"], "confirm": []}),
                )
                .await?;
            }
            println!("{count} apply and revert cycles done");
        }
        _ => return Err(format!("unknown action {action}")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    match rt.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), format!("{e}\n").as_bytes());
            ExitCode::FAILURE
        }
    }
}
