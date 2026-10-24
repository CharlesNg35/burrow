use burrow_admin_logic::addr;
use burrow_ui_kit::{Segmented, Select, TextInput, Toggle, Tone, tag_class};
use leptos::prelude::*;

use super::{Addr, Kind, SPECIFIC, base_hosts};

pub(super) fn host_control(
    kind: Kind,
    fc_id: String,
    parts: RwSignal<addr::Parts>,
    specific: RwSignal<bool>,
    implicit: bool,
    c_host: impl Fn() + Clone + Send + Sync + 'static,
) -> impl Fn() -> AnyView + Clone + Send + Sync {
    move || match &kind {
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
    }
}

pub(super) fn network(
    id: String,
    msg: String,
    a: Addr,
    ranges: bool,
    on_port: impl Fn(String) + Clone + Send + Sync + 'static,
    commit: impl Fn() + Clone + Send + Sync + 'static,
    host_control: impl Fn() -> AnyView + Clone + Send + Sync + 'static,
) -> impl Fn() -> AnyView {
    let Addr {
        parts,
        port_text,
        range_on,
        touched,
        invalid,
        has_error,
        count,
    } = a;
    let (c_to, c_range) = (commit.clone(), commit);
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
        .into_any()
    }
}

pub(super) fn unix_input(
    id: String,
    a: Addr,
    c_unix: impl Fn() + Clone + Send + Sync + 'static,
) -> impl Fn() -> AnyView {
    let Addr {
        parts,
        touched,
        invalid,
        ..
    } = a;
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
        .into_any()
    }
}

pub(super) fn mode_switch(
    fc_id: String,
    unix_mode: Memo<bool>,
    parts: RwSignal<addr::Parts>,
    c_mode: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let name = format!("{}-kind", fc_id);
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
}
