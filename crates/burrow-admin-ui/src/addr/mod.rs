mod picker;

use burrow_admin_logic::addr;
use burrow_admin_logic::input::to_text;
use burrow_ui_kit::{Segmented, Select, TextInput, Toggle, Tone, tag_class};
use leptos::prelude::*;

use crate::domain::{FieldCtx, set_checked};
use crate::store::store;

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
    {
        let path = fc.path.clone();
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
    let c_to = commit.clone();
    let c_host = commit.clone();
    let c_range = commit.clone();
    let c_unix = commit.clone();
    let c_mode = commit.clone();
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
    let fc_id = fc.id.clone();
    let host_control = move || match &kind {
        Kind::Bind { default_ip, .. } => {
            let default_ip = default_ip.clone();
            let d2 = default_ip.clone();
            let opts = Signal::derive(move || {
                let mut o = base_hosts();
                o.push((SPECIFIC.to_string(), "Specific…".to_string()));
                o
            });
            let value = Signal::derive(move || {
                if specific.get() {
                    return SPECIFIC.to_string();
                }
                let h = parts.with(|p| p.host.clone());
                if h.is_empty() { d2.clone() } else { h }
            });
            let host_id = format!("{}-host", fc_id);
            let host_id2 = host_id.clone();
            let c_host = c_host.clone();
            let c_host2 = c_host.clone();
            view! {
                <div class="w-[220px] shrink-0 max-sm:w-full">
                    <Select
                        id=host_id
                        label="Address"
                        options=opts
                        value=value
                        on_change=Callback::new(move |v: String| {
                            if v == SPECIFIC {
                                specific.set(true);
                                return;
                            }
                            specific.set(false);
                            let host = if v == default_ip && implicit { String::new() } else { v };
                            parts.update(|p| p.host = host);
                            c_host();
                        })
                    />
                </div>
                <Show when=move || specific.get()>
                    {
                        let c_host2 = c_host2.clone();
                        let id = format!("{host_id2}-ip");
                        view! {
                            <div class="w-40 shrink-0">
                                <TextInput
                                    id=id
                                    label="IP address"
                                    mono=true
                                    value=Signal::derive(move || parts.with(|p| p.host.clone()))
                                    on_input={
                                        let c_host2 = c_host2.clone();
                                        move |t: String| {
                                            parts.update(|p| p.host = t.trim().to_string());
                                            c_host2();
                                        }
                                    }
                                />
                            </div>
                        }
                    }
                </Show>
                <span class="font-bold text-text-faint max-sm:hidden" aria-hidden="true">":"</span>
            }
            .into_any()
        }
        Kind::Target { .. } => {
            let host_id = format!("{}-host", fc_id);
            let c_host = c_host.clone();
            view! {
                <div class="w-[240px] min-w-0 max-sm:w-full">
                    <TextInput
                        id=host_id
                        label="Host"
                        mono=true
                        placeholder="127.0.0.1"
                        value=Signal::derive(move || parts.with(|p| p.host.clone()))
                        on_input=move |t: String| {
                            parts.update(|p| p.host = t.trim().to_string());
                            c_host();
                        }
                    />
                </div>
                <span class="font-bold text-text-faint max-sm:hidden" aria-hidden="true">":"</span>
            }
            .into_any()
        }
        Kind::Range { .. } => ().into_any(),
    };
    let unix_mode = Memo::new(move |_| parts.with(|p| p.unix.is_some()));
    let network = {
        let id = id.clone();
        let msg = msg.clone();
        move || {
            let id = id.clone();
            let range_id = format!("{id}-range");
            let msg = msg.clone();
            let on_port = on_port.clone();
            let c_to = c_to.clone();
            let c_range = c_range.clone();
            let host_control = host_control.clone();
            view! {
                <div class="flex flex-wrap items-center gap-2" data-parts="network">
                    {host_control()}
                    <div class="w-[100px] shrink-0">
                        <TextInput
                            id=id.clone()
                            label=if range_on.get_untracked() { "First port" } else { "Port" }
                            numeric=true
                            mono=true
                            invalid=invalid
                            described_by=msg.clone()
                            value=port_text
                            on_input=on_port
                            on_blur=move |_| {
                                let _ = touched.try_set(true);
                                port_text.set(parts.with_untracked(|p| p.from.clone()));
                            }
                        />
                    </div>
                    <Show when=move || range_on.get()>
                        {
                            let c_to = c_to.clone();
                            let id = format!("{}-to", id);
                            view! {
                                <span class="font-bold text-text-faint" aria-hidden="true">"–"</span>
                                <div class="w-[100px] shrink-0">
                                    <TextInput
                                        id=id
                                        label="Last port"
                                        numeric=true
                                        mono=true
                                        value=Signal::derive(move || parts.with(|p| p.to.clone()))
                                        on_input={
                                            let c_to = c_to.clone();
                                            move |t: String| {
                                                parts.update(|p| p.to = t.trim().to_string());
                                                c_to();
                                            }
                                        }
                                    />
                                </div>
                            }
                        }
                    </Show>
                    {move || count.get().map(|n| view! {
                        <span class=tag_class(if has_error.get() { Tone::Danger } else { Tone::Accent }) data-testid="port-count">
                            {format!("{n} port{}", if n == 1 { "" } else { "s" })}
                        </span>
                    })}
                    {ranges.then(|| {
                        let c_range = c_range.clone();
                        let id = range_id.clone();
                        view! {
                            <div class="ml-4">
                                <Toggle
                                    id=id
                                    checked=range_on
                                    label="Port range"
                                    on_change=move |on: bool| {
                                        range_on.set(on);
                                        if !on {
                                            c_range();
                                        }
                                    }
                                />
                            </div>
                        }
                    })}
                </div>
            }
        }
    };
    let unix = {
        let id = id.clone();
        move || {
            let c_unix = c_unix.clone();
            view! {
                <TextInput
                    id=id.clone()
                    label="Unix socket path"
                    mono=true
                    prefix="unix:"
                    invalid=invalid
                    value=Signal::derive(move || parts.with(|p| p.unix.clone().unwrap_or_default()))
                    on_input=move |t: String| {
                        parts.update(|p| p.unix = Some(t.trim().to_string()));
                        c_unix();
                    }
                    on_blur=move |_| { let _ = touched.try_set(true); }
                />
            }
        }
    };
    let mode = unix_ok.then(|| {
        let name = format!("{}-kind", fc.id);
        view! {
            <div class="mb-2">
                <Segmented
                    name=name
                    label="Target kind"
                    options=vec![("network".to_string(), "Network".to_string(), false), ("unix".to_string(), "Unix socket".to_string(), false)]
                    value=Signal::derive(move || if unix_mode.get() { "unix".to_string() } else { "network".to_string() })
                    on_change=Callback::new(move |v: String| {
                        parts.update(|p| p.unix = (v == "unix").then(String::new));
                        c_mode();
                    })
                />
            </div>
        }
    });
    view! {
        <div data-addr="">
            {mode}
            {move || if unix_mode.get() { unix().into_any() } else { network().into_any() }}
            <p class="mt-1.5 text-xs text-text-muted" data-testid="stored-as" data-stored=stored>
                "Saved as "
                <code class="font-mono">{stored}</code>
                {(ranges && max.is_some()).then(|| max.map(|m| format!(" · up to {m} ports")))}
            </p>
        </div>
    }
    .into_any()
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
                options=vec![("host".to_string(), "host".to_string(), false), ("ws".to_string(), "ws://".to_string(), false), ("wss".to_string(), "wss://".to_string(), false)]
                value=Signal::derive(move || server.with(|s| s.scheme.to_string()))
                on_change=Callback::new(move |v: String| {
                    server.update(|s| {
                        s.scheme = match v.as_str() {
                            "ws" => "ws",
                            "wss" => "wss",
                            _ => "host",
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
