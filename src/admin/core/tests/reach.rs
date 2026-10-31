#![cfg(test)]

use burrow_admin_proto::codes::{CLOSES_PORT, REMOVES_VISITORS, STOPS_WEBSITE};
use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Level, Plan};

use super::*;

const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$3Zrnt9ZV6D9A/Ed3Je/Hww$lg26ncNCHaoX2amIl0p8mWtR1BCyscvYzi1EmeCfbvg";

fn reached_three_ways() -> World {
    let mut w = world();
    let (laptop, phone) = (Secret::generate(), Secret::generate());
    w.text = w.text.replace(
        "\nclients:\n",
        "\nweb: { https: 443, letsencrypt: { email: ops@example.com } }\n\nclients:\n",
    );
    w.text = w.text.replace(
        "    admin: write\n",
        &format!(
            "    admin: write\n  laptop:\n    secret: {}\n  phone:\n    secret: {}\n",
            laptop.encode(),
            phone.encode()
        ),
    );
    w.text.push_str(&format!(
        "  nas:\n    client: home\n    bind: 9000\n    target: 8080\n    visitors: [laptop, phone]\n    web:\n      hosts: [nas.example.com]\n      basic_auth: {{ alice: \"{HASH}\", bob: \"{HASH}\" }}\n      redirect_from: [www.nas.example.com]\n"
    ));
    w
}

fn removing(key: &str) -> Plan {
    let w = reached_three_ways();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    let ops = [Op::Remove {
        path: p(&["services", "nas", key]),
    }];
    match plan::compute(&core, &s, &base, &ops) {
        Ok(c) => c.plan,
        Err(e) => panic!("{key} was refused: {}", e.message),
    }
}

fn impact(plan: &Plan, code: &str) -> (Level, String, Option<String>) {
    let i = plan
        .impacts
        .iter()
        .find(|i| i.code == code)
        .unwrap_or_else(|| panic!("no {code} in {:#?}", plan.impacts));
    (i.level, i.message.clone(), i.confirm.clone())
}

#[test]
fn turning_off_the_public_port_says_which_port_closes() {
    let plan = removing("bind");
    let (level, msg, _) = impact(&plan, CLOSES_PORT);
    assert_eq!(level, Level::Warning);
    assert_eq!(msg, "closes port 9000 (removes bind: 0.0.0.0:9000)");
    assert!(
        !plan
            .impacts
            .iter()
            .any(|i| i.code == burrow_admin_proto::codes::REBINDS_SERVICE),
        "one impact, not two: {:#?}",
        plan.impacts
    );
}

#[test]
fn turning_off_the_website_lists_the_settings_it_removes() {
    let plan = removing("web");
    let (level, msg, confirm) = impact(&plan, STOPS_WEBSITE);
    assert_eq!(level, Level::Warning);
    assert_eq!(
        msg,
        "stops website nas.example.com and removes its settings: HTTPS, basic_auth (2 users), redirects"
    );
    assert_eq!(confirm.as_deref(), Some("nas"), "a confirm-level warning");
}

#[test]
fn turning_off_visitors_names_them() {
    let plan = removing("visitors");
    let (_, msg, _) = impact(&plan, REMOVES_VISITORS);
    assert_eq!(
        msg,
        "removes visitors laptop, phone: they can no longer reach nas with burrow visit"
    );
}
