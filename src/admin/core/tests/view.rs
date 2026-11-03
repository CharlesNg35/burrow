use burrow_admin_proto::msg::{AdminLevel, State, req};
use burrow_admin_proto::value;
use burrow_admin_proto::view::{ConfigView, Shown, ShownRequest};
use burrow_config::{ServerConfig, ServiceEntry};
use serde_json::{Value, json};

use super::{core_for, core_running, session, world};
use crate::admin::core::{Core, dispatch, view};

const BOTH: [Shown; 2] = [Shown::Written, Shown::WithDefaults];

fn ask(core: &Core, t: &str, show: Shown) -> Value {
    let w = world();
    let s = session(&w, AdminLevel::Read);
    dispatch(core, &s, t, &json!(ShownRequest { show })).unwrap()
}

fn shown(core: &Core, show: Shown) -> ConfigView {
    serde_json::from_value(ask(core, req::CONFIG_VIEW, show)).unwrap()
}

fn read_back(text: &str) -> Value {
    crate::config::tree::parse(text).unwrap().to_json()
}

#[test]
fn the_config_file_view_is_get_configs_view_as_text_written_and_with_defaults() {
    let w = world();
    let core = core_for(&w.text);
    for show in BOTH {
        let state: State = serde_json::from_value(ask(&core, req::STATE, show)).unwrap();
        let file = shown(&core, show);
        assert_eq!(file.rev, state.rev);
        assert_eq!(file.show, show);
        assert_eq!(
            read_back(&file.text()),
            view::plain(&state.values),
            "{show:?}"
        );
        assert_eq!(file.sections, view::sections(&state.values), "{show:?}");
    }
}

#[test]
fn only_written_keys_show_until_defaults_are_asked_for() {
    let w = world();
    let core = core_for(&w.text);
    let written = read_back(&shown(&core, Shown::Written).text());
    let all = read_back(&shown(&core, Shown::WithDefaults).text());
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys(&written),
        [
            ServerConfig::LISTEN.key(),
            ServerConfig::CLIENTS.key(),
            ServerConfig::VISITORS.key(),
            ServerConfig::SERVICES.key(),
        ]
    );
    for block in [
        ServerConfig::HEARTBEAT.key(),
        ServerConfig::LIMITS.key(),
        ServerConfig::POOL.key(),
        ServerConfig::TCP.key(),
        ServerConfig::UDP.key(),
    ] {
        assert!(written.get(block).is_none(), "{block}");
        assert!(all[block].is_object(), "{block} in {all}");
    }
    for off in [
        ServerConfig::WEB.key(),
        ServerConfig::WEBSOCKET.key(),
        ServerConfig::METRICS.key(),
    ] {
        assert!(all.get(off).is_none(), "{off} stays off: {all}");
    }
    let listen = [ServerConfig::LISTEN.key().to_string()];
    assert_eq!(value::get(&all, &listen), value::get(&written, &listen));
    let target = ServiceEntry::TARGET.of(&service_name(&w.text));
    assert_eq!(value::get(&all, &target), Some(&json!(3000)));
    let entry = &target[..target.len() - 1];
    let fields = |v: &Value| {
        value::get(v, entry)
            .and_then(Value::as_object)
            .map_or(0, |m| m.len())
    };
    assert!(
        fields(&all) > fields(&written),
        "an entry gets its defaults too: {all}"
    );
}

fn service_name(text: &str) -> String {
    let s = &text[text.find(ServerConfig::SERVICES.key()).unwrap()..];
    s.lines()
        .nth(1)
        .unwrap()
        .trim()
        .split(':')
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn a_read_admin_sees_the_same_view_as_a_write_admin() {
    let w = world();
    let core = core_for(&w.text);
    for show in BOTH {
        let body = json!(ShownRequest { show });
        let read = dispatch(
            &core,
            &session(&w, AdminLevel::Read),
            req::CONFIG_VIEW,
            &body,
        );
        let write = dispatch(
            &core,
            &session(&w, AdminLevel::Write),
            req::CONFIG_VIEW,
            &body,
        );
        assert_eq!(read.unwrap(), write.unwrap());
    }
}

#[test]
fn a_file_with_errors_still_shows_what_it_says_and_the_view_follows_the_file() {
    let w = world();
    let broken = w.text.replace("target: 3000", "target: nowhere:0");
    let core = core_running(&broken, &w.text);
    let file = shown(&core, Shown::Written);
    assert!(file.text().contains("nowhere:0"), "{}", file.text());
    assert_eq!(file.rev, crate::admin::core::text::rev(&broken));
    assert_eq!(file.download_name(), "server.redacted.yaml");
}
