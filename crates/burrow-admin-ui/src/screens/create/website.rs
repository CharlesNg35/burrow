use burrow_admin_logic::web::{needs_letsencrypt, service_name, website};
use burrow_admin_proto::serde_json::{Value, json};
use burrow_config::{ClientEntry, Entry, LetsEncrypt, ServerConfig, ServiceEntry, WebConfig};
use burrow_syntax::web::{CheckPorts, SingleLabel, checks_for, host_name};
use burrow_ui_kit::{
    Button, Callout, ChipInput, CodeBlock, ComboOption, Combobox, Edge, EdgeState, Hop, HopState,
    PathData, PathDiagram, Segmented, TextInput, Tone,
};
use leptos::prelude::*;

use crate::route::{Route, go};
use crate::store::store;

use super::{name_problem, taken};

const AUTO: &str = "auto";
const PASSTHROUGH: &str = "passthrough";
const OFF: &str = "off";
const TERMS: &str = "https://letsencrypt.org/repository/";
const TURN_ON: &str =
    "# server.yaml\nweb:\n  https: 443\n  http: 80\n  letsencrypt:\n    email: you@example.com\n";

#[derive(Clone, Copy, PartialEq)]
struct Ports {
    on: bool,
    https: bool,
    http: bool,
    numbers: CheckPorts,
}

fn shown_port(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn port_of(v: &Value) -> Option<u16> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
        Value::String(s) => s.rsplit(':').next().and_then(|p| p.trim().parse().ok()),
        _ => None,
    }
}

fn web_port(plain: bool) -> String {
    store().edit.draft.with(|d| {
        let web = ServerConfig::WEB.get(d.draft());
        let (scheme, field) = if plain {
            ("HTTP", web.and_then(|w| WebConfig::HTTP.get(w)))
        } else {
            ("HTTPS", web.and_then(|w| WebConfig::HTTPS.get(w)))
        };
        let port = shown_port(field);
        if port.is_empty() {
            String::new()
        } else {
            format!("{scheme} {port}")
        }
    })
}

fn ports() -> Memo<Ports> {
    let e = store().edit;
    Memo::new(move |_| {
        e.draft.with(|d| {
            let web = ServerConfig::WEB.get(d.draft()).filter(|w| !w.is_null());
            let has = |w: Option<&Value>, f: fn(&Value) -> Option<&Value>| {
                w.and_then(f).is_some_and(|v| !v.is_null())
            };
            Ports {
                on: web.is_some(),
                https: has(web, |w| WebConfig::HTTPS.get(w)),
                http: has(web, |w| WebConfig::HTTP.get(w)),
                numbers: CheckPorts {
                    https: web.and_then(|w| WebConfig::HTTPS.get(w)).and_then(port_of),
                    http: web.and_then(|w| WebConfig::HTTP.get(w)).and_then(port_of),
                },
            }
        })
    })
}

fn path(names: &[String], client: &str, target: &str, server: &str) -> PathData {
    let hop = |icon: &'static str, head: &str, value: String, current: bool| Hop {
        icon,
        head: head.into(),
        value: if value.is_empty() {
            "not set".into()
        } else {
            value
        },
        mono: true,
        dot: Some(Tone::Neutral),
        state: if current {
            HopState::Current
        } else {
            HopState::default()
        },
        ..Hop::default()
    };
    let edge = || Edge {
        above: String::new(),
        below: String::new(),
        state: EdgeState::Added,
    };
    PathData {
        hops: vec![
            hop("globe", "Visitors", "anyone".into(), false),
            hop(
                "globe",
                "Name",
                names.first().cloned().unwrap_or_default(),
                true,
            ),
            hop("box", "Server", server.to_string(), false),
            hop("monitor", "Client", client.to_string(), false),
            hop("server", "App", target.to_string(), false),
        ],
        edges: vec![edge(), edge(), edge(), edge()],
        ..PathData::default()
    }
}

fn tls_options(p: Ports) -> Vec<(String, String, bool)> {
    vec![
        (AUTO.into(), "Automatic".into(), !p.https),
        (
            PASSTHROUGH.into(),
            "The app does its own HTTPS".into(),
            !p.https,
        ),
        (OFF.into(), "Off: plain HTTP".into(), !p.http),
    ]
}

fn clients() -> Vec<ComboOption> {
    taken(ClientEntry::section())
        .into_iter()
        .map(|c| ComboOption {
            value: c.clone(),
            label: c,
            facet: String::new(),
            dot: None,
        })
        .collect()
}

fn site_value(names: &[String], client: &str, target: &str, tls: &str) -> Value {
    let target = target
        .trim()
        .parse::<u16>()
        .map_or_else(|_| json!(target.trim()), |p| json!(p));
    let tls = (tls != AUTO).then(|| json!(tls));
    let mut v = json!({});
    ServiceEntry::CLIENT.set(&mut v, json!(client));
    ServiceEntry::TARGET.set(&mut v, target);
    if let Some(web) = website(None, names, tls) {
        ServiceEntry::WEB.set(&mut v, web);
    }
    v
}

#[component]
pub fn NewWebsite(open: RwSignal<bool>, draft_name: RwSignal<String>) -> impl IntoView {
    let s = store();
    let e = s.edit;
    let p = ports();
    let names: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let client: RwSignal<Vec<String>> =
        RwSignal::new(clients().into_iter().take(1).map(|c| c.value).collect());
    let target = RwSignal::new("8080".to_string());
    let tls = RwSignal::new(if p.get_untracked().https { AUTO } else { OFF }.to_string());
    let email = RwSignal::new(String::new());
    let plain = Signal::derive(move || tls.get() == OFF);
    let check_name = Callback::new(move |t: String| {
        let single = if plain.get_untracked() {
            SingleLabel::Allowed
        } else {
            SingleLabel::Refused
        };
        host_name(&t, single).err()
    });
    let service = Memo::new(move |_| service_name(&names.get(), &taken(ServiceEntry::section())));
    Effect::new(move |_| {
        let n = if names.with(Vec::is_empty) {
            String::new()
        } else {
            service.get()
        };
        draft_name.set(n);
    });
    let asks_email = Memo::new(move |_| {
        tls.get() == AUTO
            && e.draft.with(|d| {
                ServerConfig::WEB
                    .get(d.draft())
                    .is_some_and(|w| needs_letsencrypt(&names.get(), w))
            })
    });
    let problem = Memo::new(move |_| {
        if !p.get().on {
            return Some("Turn the web proxy on in server.yaml first".to_string());
        }
        if names.with(Vec::is_empty) {
            return Some("Add the website's name".to_string());
        }
        if client.with(Vec::is_empty) {
            return Some("Pick the client that runs the app".to_string());
        }
        if target.with(|t| t.trim().is_empty()) {
            return Some("Give the app's port".to_string());
        }
        if asks_email.get()
            && let Some(n) = names
                .get()
                .into_iter()
                .find(|n| checks_for(n, p.get().numbers, false, false).is_err())
        {
            return Some(format!(
                "Let's Encrypt cannot check {n}: its checks reach only ports 443 and 80; open port 80 in server.yaml, or choose another HTTPS option"
            ));
        }
        if asks_email.get() && !email.with(|m| m.contains('@')) {
            return Some("Give Let's Encrypt's contact email".to_string());
        }
        name_problem(&service.get(), ServiceEntry::section(), None)
    });
    let finish = move |_| {
        if problem.get_untracked().is_some() {
            return;
        }
        let name = service.get_untracked();
        let c = client.get_untracked().first().cloned().unwrap_or_default();
        let value = site_value(
            &names.get_untracked(),
            &c,
            &target.get_untracked(),
            &tls.get_untracked(),
        );
        e.set(
            &[ServiceEntry::section().to_string(), name.clone()],
            Some(value),
        );
        if asks_email.get_untracked() {
            let le = ServerConfig::WEB.then(WebConfig::LETSENCRYPT);
            e.set(
                &le.path(),
                Some(json!({ LetsEncrypt::EMAIL.key(): email.get_untracked().trim() })),
            );
        }
        open.set(false);
        draft_name.set(String::new());
        go(Route::Detail(ServiceEntry::section().into(), name));
    };
    let diagram = Signal::derive(move || {
        let c = client.get().first().cloned().unwrap_or_default();
        path(&names.get(), &c, &target.get(), &web_port(plain.get()))
    });
    view! {
        <div data-testid="new-website">
            <nav class="mb-1.5 flex gap-1.5 text-sm text-text-muted" aria-label="Breadcrumb">
                <crate::route::Link to=Route::List(ServiceEntry::section().into()) class="text-text-muted no-underline hover:text-text">"Services"</crate::route::Link>
                <span aria-hidden="true">"/"</span>
                <span class="font-semibold text-text">"New website"</span>
            </nav>
            <header class="mb-[22px]">
                <h1 class="text-[25px] font-bold tracking-[-0.015em]">"New website"</h1>
                <p class="mt-1 text-base text-text-muted">"A name visitors type, and the app on one of your clients that answers it."</p>
            </header>
            {move || (!p.get().on).then(|| view! {
                <div class="mb-4" data-testid="web-off-new">
                    <Callout tone=Tone::Warning title="The web proxy is off">
                        <p class="mb-2">"Its ports open listeners, so they are set in server.yaml (burrow edit). Add these lines, save, and come back:"</p>
                        <CodeBlock text=Signal::stored(TURN_ON.to_string())/>
                    </Callout>
                </div>
            })}
            <PathDiagram data=diagram testid="new-website-path"/>
            {steps(names, check_name, client, target, tls, p)}
            {move || asks_email.get().then(|| email_step(email))}
            {move || problem.get().filter(|_| p.get().on && !names.with(Vec::is_empty)).map(|m| view! {
                <p class="mb-2 flex items-center gap-1.5 text-sm text-danger" data-testid="new-website-problem">
                    <burrow_ui_kit::Icon name="alert-circle" small=true/>
                    {m}
                </p>
            })}
            <div class="sticky bottom-4 z-20 mt-6 flex flex-wrap items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay" data-sticky-bar="">
                <span class="text-sm text-text-muted" data-testid="new-website-service">{move || format!("Service \"{}\"", service.get())}</span>
                <span class="ml-auto"></span>
                <Button label="Cancel" variant="ghost" testid="create-cancel" on_click=move |_| { open.set(false); draft_name.set(String::new()); }/>
                <Button
                    label="Add to changes"
                    variant="primary"
                    testid="create"
                    disabled=Signal::derive(move || problem.get().is_some())
                    reason=Signal::derive(move || problem.get().unwrap_or_default())
                    on_click=finish
                />
            </div>
        </div>
    }
}

fn step(n: u8, title: &'static str, sub: &'static str) -> impl IntoView {
    view! {
        <div class="flex items-baseline gap-2.5 border-b border-border pb-2.5 md:col-span-2">
            <h3 class="text-[14px] font-bold">{format!("{n}. {title}")}</h3>
            <span class="text-[12.5px] text-text-muted">{sub}</span>
        </div>
    }
}

fn steps(
    names: RwSignal<Vec<String>>,
    check_name: Callback<String, Option<String>>,
    client: RwSignal<Vec<String>>,
    target: RwSignal<String>,
    tls: RwSignal<String>,
    p: Memo<Ports>,
) -> impl IntoView {
    let options = Signal::derive(clients);
    let on_names = Callback::new(move |l: Vec<String>| names.set(l));
    let on_client =
        Callback::new(move |c: Vec<String>| client.set(c.into_iter().last().into_iter().collect()));
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
            {step(1, "Address", "The names visitors type.")}
            <div class="md:col-span-2">
                <ChipInput
                    id="new-website-names"
                    items=Signal::derive(move || names.get())
                    on_change=on_names
                    check=check_name
                    placeholder="Type a name like blog.example.com and press Enter"
                />
                <p class="mt-1.5 text-xs text-text-muted">"For example blog.example.com, *.apps.example.com or 203.0.113.10."</p>
            </div>
            {step(2, "Client", "The machine that reaches the app.")}
            <div>
                <Combobox
                    id="new-website-client"
                    options=options
                    selected=Signal::derive(move || client.get())
                    on_change=on_client
                    header="Clients"
                    placeholder="Pick a client…"
                    label="Client"
                />
            </div>
            <div></div>
            {step(3, "App on it", "Where the client finds the app.")}
            <div>
                <TextInput id="new-website-target" value=Signal::derive(move || target.get()) on_input=move |t: String| target.set(t) mono=true/>
                <p class="mt-1.5 text-xs text-text-muted">"For example 8080, 127.0.0.1:8080 or nas.lan:5000; a port alone means the client itself."</p>
            </div>
            <div></div>
            {step(4, "HTTPS", "Who does HTTPS for this website.")}
            <div class="md:col-span-2" data-testid="new-website-tls">
                {move || view! {
                    <Segmented name="new-website-tls" options=tls_options(p.get()) value=Signal::derive(move || tls.get()) label="HTTPS" on_change=move |m: String| tls.set(m)/>
                }}
                <p class="mt-1.5 text-xs text-text-muted">"Automatic gets a certificate for every name. Your own certificate files are set in server.yaml (burrow edit). Each choice needs the web proxy's HTTPS or HTTP port to be on."</p>
            </div>
        </section>
    }
}

fn email_step(email: RwSignal<String>) -> impl IntoView {
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="new-website-email">
            {step(5, "Let's Encrypt", "Automatic certificates for this server.")}
            <div>
                <TextInput id="new-website-email" value=Signal::derive(move || email.get()) on_input=move |t: String| email.set(t) placeholder="ops@example.com"/>
                <p class="mt-1.5 text-xs text-text-muted">
                    "Let's Encrypt's contact, for example ops@example.com. Giving it accepts "
                    <a href=TERMS target="_blank" rel="noopener noreferrer" class="font-semibold text-accent-ink">"Let's Encrypt's terms"</a>
                    "."
                </p>
            </div>
        </section>
    }
}
