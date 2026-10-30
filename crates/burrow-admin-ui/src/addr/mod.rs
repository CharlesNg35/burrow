mod entries;
mod entry_summary;
mod entry_target;
mod parts;
mod picker;

use burrow_admin_logic::addr;
use burrow_admin_logic::input::to_text;
use burrow_admin_proto::Path;
use burrow_ui_kit::{Segmented, TextInput};
use leptos::prelude::*;

use crate::domain::{FieldCtx, set_checked};
use crate::store::store;

pub(crate) use entries::{entries, entries_view};
use parts::{host_control, mode_switch, network, unix_input};
pub(crate) use picker::{merged_partner, picker};

#[derive(Clone, Debug)]
pub enum Kind {
    Bind {
        ranges: bool,
        max: Option<u16>,
        default_ip: String,
    },
    Target {
        ranges: bool,
        unix: bool,
    },
    Range {
        max: Option<u16>,
    },
}

const SPECIFIC: &str = "\u{1}specific";

fn base_hosts() -> Vec<(String, String)> {
    let mut o = vec![
        ("0.0.0.0".to_string(), "All IPv4 (0.0.0.0)".to_string()),
        ("::".to_string(), "All IPv6 (::)".to_string()),
        ("127.0.0.1".to_string(), "Loopback (127.0.0.1)".to_string()),
    ];
    let extra: Vec<String> = store().state.with(|s| {
        s.as_ref()
            .map(|s| s.runtime.addresses.clone())
            .unwrap_or_default()
    });
    for a in extra {
        if !o.iter().any(|(v, _)| *v == a) {
            o.push((a.clone(), a));
        }
    }
    o
}

pub fn addr_editor(fc: FieldCtx, kind: Kind) -> AnyView {
    let e = store().edit;
    let start = addr::split(e.peek(&fc.path).as_ref());
    let implicit = start.host.is_empty();
    let parts = RwSignal::new(start.clone());
    let port_text = RwSignal::new(start.from.clone());
    let range_on = RwSignal::new(!start.to.is_empty());
    let specific = RwSignal::new(
        !start.host.is_empty() && !base_hosts().iter().any(|(v, _)| *v == start.host),
    );
    let (ranges, unix_ok, max) = match &kind {
        Kind::Bind { ranges, max, .. } => (*ranges, false, *max),
        Kind::Target { ranges, unix } => (*ranges, *unix, None),
        Kind::Range { max } => (true, false, *max),
    };
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || {
        let mut p = parts.get_untracked();
        if !range_on.get_untracked() {
            p.to.clear();
        }
        set_checked(&fc2, &node, addr::join(&p));
    };
    sync_from_draft(fc.path.clone(), parts, port_text, range_on);
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let has_error = {
        let path = fc.path.clone();
        Signal::derive(move || {
            e.local.with(|l| l.contains_key(&path)) || e.related.with(|r| r.contains_key(&path))
        })
    };
    let c_port = commit.clone();
    let on_port = move |t: String| {
        if addr::looks_whole(&t) {
            let p = addr::split_text(&t);
            range_on.set(ranges && !p.to.is_empty());
            parts.update(|x| {
                x.host = p.host.clone();
                x.from = p.from.clone();
                x.to = p.to.clone();
                x.unix = p.unix.clone();
            });
            specific.set(!p.host.is_empty() && !base_hosts().iter().any(|(v, _)| *v == p.host));
        } else {
            parts.update(|x| x.from = t.clone());
        }
        port_text.set(t);
        c_port();
    };
    let id = fc.id.clone();
    let msg = fc.msg_id.clone();
    let count = Signal::derive(move || {
        parts.with(|p| {
            let mut p = p.clone();
            if !range_on.get() {
                p.to.clear();
            }
            addr::count(&p)
        })
    });
    let stored = {
        let path = fc.path.clone();
        Signal::derive(move || {
            e.value(&path)
                .map(|v| to_text(Some(&v)))
                .unwrap_or_default()
        })
    };
    let host_control = host_control(
        kind,
        fc.id.clone(),
        parts,
        specific,
        implicit,
        commit.clone(),
    );
    let unix_mode = Memo::new(move |_| parts.with(|p| p.unix.is_some()));
    let a = Addr {
        parts,
        port_text,
        range_on,
        touched,
        invalid,
        has_error,
        count,
    };
    let network = network(
        id.clone(),
        msg.clone(),
        a,
        ranges,
        on_port,
        commit.clone(),
        host_control,
    );
    let unix = unix_input(id.clone(), a, commit.clone());
    let mode = unix_ok.then(|| mode_switch(fc.id.clone(), unix_mode, parts, commit.clone()));
    let none = fc.node.none_label.clone();
    view! {
        <div data-addr="">
            {mode}
            {move || if unix_mode.get() { unix().into_any() } else { network().into_any() }}
            <p class="mt-1.5 text-xs text-text-muted" class:hidden=move || stored.with(String::is_empty) data-testid="stored-as" data-stored=stored>
                {format!("{} ", burrow_admin_logic::wording::WRITTEN_AS)}
                <code class="font-mono">{stored}</code>
                {(ranges && max.is_some()).then(|| max.map(|m| format!(" · up to {m} ports")))}
            </p>
            {none.map(|n| view! { <p class="mt-1.5 text-xs text-text-muted" class:hidden=move || !stored.with(String::is_empty) data-testid="stored-none">{n}</p> })}
        </div>
    }
    .into_any()
}

#[derive(Clone, Copy)]
struct Addr {
    parts: RwSignal<addr::Parts>,
    port_text: RwSignal<String>,
    range_on: RwSignal<bool>,
    touched: RwSignal<bool>,
    invalid: Signal<bool>,
    has_error: Signal<bool>,
    count: Signal<Option<u32>>,
}

fn sync_from_draft(
    path: Path,
    parts: RwSignal<addr::Parts>,
    port_text: RwSignal<String>,
    range_on: RwSignal<bool>,
) {
    let e = store().edit;
    Effect::new(move |_| {
        let now = e.value(&path);
        let mine = parts.with_untracked(|p| {
            let mut p = p.clone();
            if !range_on.get_untracked() {
                p.to.clear();
            }
            addr::join(&p)
        });
        if now != mine {
            let p = addr::split(now.as_ref());
            port_text.set(p.from.clone());
            range_on.set(!p.to.is_empty());
            parts.set(p);
        }
    });
}

pub fn server_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let start = addr::split_server(e.peek(&fc.path).as_ref());
    let server = RwSignal::new(start);
    {
        let path = fc.path.clone();
        Effect::new(move |_| {
            let now = e.value(&path);
            if now != server.with_untracked(addr::join_server) {
                server.set(addr::split_server(now.as_ref()));
            }
        });
    }
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || set_checked(&fc2, &node, server.with_untracked(addr::join_server));
    let c2 = commit.clone();
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    view! {
        <div class="flex flex-wrap items-center gap-2" data-server-address="">
            <Segmented
                name=format!("{}-scheme", fc.id)
                label="Address kind"
                options=vec![("name".to_string(), "host name".to_string(), false), ("ws".to_string(), "ws://".to_string(), false), ("wss".to_string(), "wss://".to_string(), false)]
                value=Signal::derive(move || server.with(|s| s.scheme.to_string()))
                on_change=Callback::new(move |v: String| {
                    server.update(|s| {
                        s.scheme = match v.as_str() {
                            "ws" => "ws",
                            "wss" => "wss",
                            _ => "name",
                        }
                    });
                    c2();
                })
            />
            <div class="min-w-60 flex-1">
                <TextInput
                    id=fc.id.clone()
                    mono=true
                    invalid=invalid
                    described_by=fc.msg_id.clone()
                    placeholder="tunnel.example.net"
                    value=Signal::derive(move || server.with(|s| s.rest.clone()))
                    on_input=move |t: String| {
                        server.update(|s| s.rest = t);
                        commit();
                    }
                    on_blur=move |_| { let _ = touched.try_set(true); }
                />
            </div>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_main_target_has_one_name_in_the_schema_and_the_client_rows() {
        use burrow_config::admin_schema::AdminSchema;
        let schema =
            burrow_admin_proto::serde_json::to_string(&burrow_config::ServerConfig::node())
                .unwrap()
                .to_lowercase();
        let rows = [
            include_str!("entries.rs"),
            include_str!("entry_target.rs"),
            include_str!("entry_summary.rs"),
        ]
        .concat()
        .to_lowercase();
        for old in ["service target", "own target"] {
            assert!(!schema.contains(old), "the schema still says {old}");
            assert!(!rows.contains(old), "the client rows still say {old}");
        }
    }
}
