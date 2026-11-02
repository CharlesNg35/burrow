use burrow_admin_logic::web::{needs_letsencrypt, service_name, website};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{
    ClientEntry, Entry, LetsEncrypt, ServerConfig, ServiceEntry, TlsMode, WebConfig,
};
use burrow_syntax::web::{CheckPorts, checks_for};
use burrow_ui_kit::{
    Button, ChipInput, ComboOption, Combobox, Edge, EdgeState, Hop, HopState, PathData,
    PathDiagram, TextInput, Tone,
};
use leptos::prelude::*;

use crate::route::{Route, go};
use crate::store::store;

use super::{name_problem, taken};

const TERMS: &str = "https://letsencrypt.org/repository/";

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

fn web_now() -> Option<Value> {
    store().edit.draft.with(|d| {
        ServerConfig::WEB
            .get(d.draft())
            .filter(|w| !w.is_null())
            .cloned()
    })
}

fn port_value(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.parse::<u16>().map_or_else(|_| json!(t), |n| json!(n)))
}

fn port_problem(label: &str, text: &str) -> Option<String> {
    let v = port_value(text)?;
    let ok = serde_json::from_value::<burrow_config::Bind>(v)
        .ok()
        .and_then(|b| b.ranged([0, 0, 0, 0], label).ok())
        .is_some_and(|(_, n)| n == 1);
    (!ok).then(|| format!("{label}: give one port, like 443, or an address like 0.0.0.0:443"))
}

fn planned(https: &str, http: &str) -> Value {
    let mut web = serde_json::Map::new();
    if let Some(v) = port_value(https) {
        web.insert(WebConfig::HTTPS.key().into(), v);
    }
    if let Some(v) = port_value(http) {
        web.insert(WebConfig::HTTP.key().into(), v);
    }
    Value::Object(web)
}

fn ports_of(web: Option<&Value>) -> Ports {
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
}

fn path(names: &[String], client: &str, target: &str, server: &str) -> PathData {
    let hop = |icon: &'static str, head: &str, value: String, current: bool| Hop {
        icon,
        head: head.into(),
        value: if value.is_empty() {
            burrow_admin_logic::wording::NOT_SET.into()
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
            hop(
                "server",
                "App",
                burrow_admin_logic::web::target_shown(target),
                false,
            ),
        ],
        edges: vec![edge(), edge(), edge(), edge()],
        ..PathData::default()
    }
}

fn tls_options(p: Ports) -> Vec<burrow_ui_kit::RadioOption> {
    crate::domain::website::https_options(|v| {
        if v == crate::domain::website::files_choice() {
            return Some("set its certificate files on the website once it exists".to_string());
        }
        let blocked = if v == TlsMode::Off.wire() {
            !p.http
        } else {
            !p.https
        };
        blocked.then(|| {
            if v == TlsMode::Off.wire() {
                "needs the web proxy's HTTP port".to_string()
            } else {
                "needs the web proxy's HTTPS port".to_string()
            }
        })
    })
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
    let tls = (tls != TlsMode::Auto.wire()).then(|| json!(tls));
    let mut v = json!({});
    ServiceEntry::CLIENT.set(&mut v, json!(client));
    ServiceEntry::TARGET.set(&mut v, target);
    if let Some(web) = website(None, names, tls) {
        ServiceEntry::WEB.set(&mut v, web);
    }
    v
}

fn service_name_of(
    names: RwSignal<Vec<String>>,
) -> (RwSignal<String>, Memo<String>, Signal<Option<String>>) {
    let suggested = Memo::new(move |_| service_name(&names.get(), &taken(ServiceEntry::section())));
    let svc_name = RwSignal::new(String::new());
    let auto = StoredValue::new(String::new());
    Effect::new(move |_| {
        let s = suggested.get();
        if svc_name.get_untracked() == auto.get_value() {
            svc_name.set(s.clone());
        }
        auto.set_value(s);
    });
    let service = Memo::new(move |_| svc_name.get().trim().to_string());
    let name_issue = Signal::derive(move || {
        let n = service.get();
        (!n.is_empty())
            .then(|| name_problem(&n, ServiceEntry::section(), None))
            .flatten()
    });
    (svc_name, service, name_issue)
}

#[derive(Clone, Copy)]
struct Checks {
    web_on: Memo<bool>,
    https_in: RwSignal<String>,
    http_in: RwSignal<String>,
    p: Memo<Ports>,
    names: RwSignal<Vec<String>>,
    client: RwSignal<Vec<String>>,
    target: RwSignal<String>,
    asks_email: Memo<bool>,
    email: RwSignal<String>,
    service: Memo<String>,
    name_issue: Signal<Option<String>>,
}

fn website_checks(c: Checks) -> Memo<Option<String>> {
    let Checks {
        web_on,
        https_in,
        http_in,
        p,
        names,
        client,
        target,
        asks_email,
        email,
        service,
        name_issue,
    } = c;
    Memo::new(move |_| {
        if !web_on.get() {
            let found = port_problem("HTTPS port", &https_in.get())
                .or_else(|| port_problem("HTTP port", &http_in.get()));
            if found.is_some() {
                return found;
            }
            if !p.get().https && !p.get().http {
                return Some("Give the web proxy an HTTPS port, an HTTP port or both".to_string());
            }
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
                .find(|n| checks_for(n, p.get().numbers, false, None).is_err())
        {
            return Some(burrow_admin_logic::wording::no_check(&n));
        }
        if asks_email.get() && !email.with(|m| m.contains('@')) {
            return Some("Give Let's Encrypt's contact email".to_string());
        }
        if service.with(String::is_empty) {
            return Some("Give the service a name".to_string());
        }
        name_issue.get()
    })
}

#[component]
pub fn NewWebsite(open: RwSignal<bool>, draft_name: RwSignal<String>) -> impl IntoView {
    let s = store();
    let e = s.edit;
    let web_on = Memo::new(move |_| web_now().is_some());
    let https_in = RwSignal::new("443".to_string());
    let http_in = RwSignal::new("80".to_string());
    let web =
        Memo::new(move |_| web_now().unwrap_or_else(|| planned(&https_in.get(), &http_in.get())));
    let p = Memo::new(move |_| ports_of(Some(&web.get())));
    let names: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let client: RwSignal<Vec<String>> =
        RwSignal::new(clients().into_iter().take(1).map(|c| c.value).collect());
    let target = RwSignal::new("8080".to_string());
    let tls = RwSignal::new(
        if p.get_untracked().https {
            TlsMode::Auto
        } else {
            TlsMode::Off
        }
        .wire()
        .to_string(),
    );
    let email = RwSignal::new(String::new());
    let plain = Signal::derive(move || {
        burrow_admin_logic::web::is_plain(Some(&Value::String(tls.get())), true)
    });
    let check_name = Callback::new(move |t: String| {
        burrow_admin_logic::web::site_name_problem(&t, plain.get_untracked())
    });
    let (svc_name, service, name_issue) = service_name_of(names);
    let save_in = RwSignal::new(String::new());
    Effect::new(move |_| {
        let n = if names.with(Vec::is_empty) {
            String::new()
        } else {
            service.get()
        };
        draft_name.set(n);
    });
    let asks_email = Memo::new(move |_| {
        tls.get() == TlsMode::Auto.wire() && needs_letsencrypt(&names.get(), &web.get())
    });
    let problem = website_checks(Checks {
        web_on,
        https_in,
        http_in,
        p,
        names,
        client,
        target,
        asks_email,
        email,
        service,
        name_issue,
    });
    let finish = move |_| {
        if problem.get_untracked().is_some() {
            return;
        }
        let name = service.get_untracked();
        let c = client.get_untracked().first().cloned().unwrap_or_default();
        if !web_on.get_untracked() {
            e.set(&ServerConfig::WEB.path(), Some(web.get_untracked()));
        }
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
        super::save_in::place_new(ServiceEntry::section(), &name, &save_in.get_untracked());
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
            {crate::screens::crumbs(Route::List(ServiceEntry::section().into()), crate::screens::plural(ServiceEntry::section()).to_string(), "New website".into(), Signal::stored(false), None)}
            <burrow_ui_kit::PageHeader sub="A name visitors type, and the app on one of your clients that answers it.">"New website"</burrow_ui_kit::PageHeader>
            {move || (!web_on.get()).then(|| web_step(https_in, http_in))}
            <PathDiagram data=diagram testid="new-website-path"/>
            {steps(names, check_name, client, target, tls, p)}
            <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="new-website-name">
                {step(5, "Service name", "A short name for lists, commands and Review.")}
                {super::name_input("new-website-service", "Service name", svc_name, name_issue, true, "blog or shop")}
                {super::save_in::save_in_field(svc_name, save_in)}
            </section>
            {move || asks_email.get().then(|| email_step(email))}
            {move || problem.get().filter(|_| !names.with(Vec::is_empty)).map(|m| view! {
                <p class="mb-2 flex items-center gap-1.5 text-sm text-danger" data-testid="new-website-problem">
                    <burrow_ui_kit::Icon name="alert-circle" small=true/>
                    {m}
                </p>
            })}
            <burrow_ui_kit::StickyBar>
                <span data-testid="new-website-service">{burrow_ui_kit::bar_status(Signal::derive(move || problem.get().unwrap_or_else(|| format!("Adds service {} to your changes", service.get()))))}</span>
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
            </burrow_ui_kit::StickyBar>
        </div>
    }
}

fn step(n: u8, title: &'static str, sub: &'static str) -> impl IntoView {
    view! {
        <burrow_ui_kit::SectionHead title={format!("{n}. {title}")} sub={sub}/>
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
            {step(1, "Names", "The names visitors type.")}
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
                    <burrow_ui_kit::RadioList name="new-website-tls" options=Signal::derive(move || tls_options(p.get())) value=Signal::derive(move || tls.get()) label="HTTPS" on_change=move |m: String| tls.set(m)/>
                }}
            </div>
        </section>
    }
}

fn web_step(https: RwSignal<String>, http: RwSignal<String>) -> impl IntoView {
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="web-off-new">
            <burrow_ui_kit::SectionHead title="Web proxy" sub="The web proxy is off; this website turns it on with these ports."/>
            <div>
                <TextInput id="new-website-https" value=Signal::derive(move || https.get()) on_input=move |t: String| https.set(t) label="HTTPS port" mono=true/>
                <p class="mt-1.5 text-xs text-text-muted">"For example 443; empty for no HTTPS."</p>
            </div>
            <div>
                <TextInput id="new-website-http" value=Signal::derive(move || http.get()) on_input=move |t: String| http.set(t) label="HTTP port" mono=true/>
                <p class="mt-1.5 text-xs text-text-muted">"For example 80, for http:// and Let's Encrypt's HTTP check; empty for none."</p>
            </div>
        </section>
    }
}

fn email_step(email: RwSignal<String>) -> impl IntoView {
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="new-website-email">
            {step(6, "Let's Encrypt", "Automatic certificates for this server.")}
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
