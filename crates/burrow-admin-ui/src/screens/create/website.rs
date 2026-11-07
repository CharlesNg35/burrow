use burrow_admin_logic::web::{needs_letsencrypt, service_name, website};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::serde_json::{Value, json};
use burrow_config::{Entry, LetsEncrypt, ServerConfig, ServiceEntry, TlsMode, WebConfig};
use burrow_ui_kit::{
    Button, ChipInput, Edge, EdgeState, Hop, HopState, PathData, PathDiagram, TextInput, Tone,
};
use leptos::prelude::*;

use crate::route::{Route, go};
use crate::store::store;

use crate::entities::{name_problem, taken};

use super::https_choice::{
    Ports, https_problem, planned, port_problem, ports_of, tls_options, web_now, web_port,
};

const TERMS: &str = "https://letsencrypt.org/repository/";

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
            hop(
                burrow_admin_logic::entity::CLIENT_ICON,
                "Client",
                burrow_admin_logic::choices::label_of(client, &crate::domain::reserved_clients()),
                false,
            ),
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

#[cfg(test)]
const OWN_CONTROLS: [&str; 1] = [ServiceEntry::WEB.key()];

fn site_rows() -> Vec<Path> {
    vec![ServiceEntry::CLIENT.schema(), ServiceEntry::TARGET.schema()]
}

fn email_path() -> Path {
    ServerConfig::WEB
        .then(WebConfig::LETSENCRYPT)
        .then(LetsEncrypt::EMAIL)
        .path()
}

fn site_value(names: &[String], site: &Value, tls: &str) -> Value {
    let mut v = json!({});
    for r in site_rows() {
        if let Some((k, x)) = r.last().and_then(|k| site.get(k).map(|x| (k, x))) {
            v[k] = x.clone();
        }
    }
    let tls = if tls == crate::domain::website::files_choice() {
        Some(json!({}))
    } else {
        (tls != TlsMode::Auto.wire()).then(|| json!(tls))
    };
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

struct Inputs {
    web_on: bool,
    https_in: String,
    http_in: String,
    ports: Ports,
    names: Vec<String>,
    site: Value,
    asks_email: bool,
    email: String,
    service: String,
    name_issue: Option<String>,
    local: Option<String>,
    https: Option<String>,
}

fn missing(site: &Value, key: &str) -> bool {
    site.get(key)
        .is_none_or(|v| v.is_null() || v.as_str() == Some(""))
}

fn email_problem(email: &str) -> Option<String> {
    let m = email.trim();
    if m.is_empty() {
        Some("Give Let's Encrypt's contact email".to_string())
    } else {
        (!burrow_syntax::web::is_email(m))
            .then(|| format!("{m} is not an email address; give one like ops@example.com"))
    }
}

fn website_problem(i: &Inputs, root: &Node, doc: &Value) -> Option<String> {
    if !i.web_on {
        let found = port_problem("HTTPS port", &i.https_in)
            .or_else(|| port_problem("HTTP port", &i.http_in));
        if found.is_some() {
            return found;
        }
        if !i.ports.https && !i.ports.http {
            return Some("Give the web proxy an HTTPS port, an HTTP port or both".to_string());
        }
    }
    if i.names.is_empty() {
        return Some("Add the website's name".to_string());
    }
    if missing(&i.site, ServiceEntry::CLIENT.key()) {
        return Some("Pick the client that runs the app".to_string());
    }
    if missing(&i.site, ServiceEntry::TARGET.key()) {
        return Some("Give the app's port".to_string());
    }
    if let Some(m) = i.local.clone().or_else(|| {
        site_rows().iter().find_map(|r| {
            let v = r.last().and_then(|k| i.site.get(k));
            crate::field::problem_at(root, r, v, doc)
        })
    }) {
        return Some(m);
    }
    if i.asks_email
        && let Some(m) = email_problem(&i.email)
    {
        return Some(m);
    }
    if let Some(m) = i.https.clone() {
        return Some(m);
    }
    if i.service.is_empty() {
        return Some("Give the service a name".to_string());
    }
    i.name_issue.clone()
}

fn start_scratch(e: crate::edit::Edit) -> (StoredValue<Path>, StoredValue<Path>) {
    let svc_at = vec![
        ServiceEntry::section().to_string(),
        crate::edit::NEW.to_string(),
    ];
    let mut start = json!({});
    if let Some(c) =
        burrow_admin_logic::choices::default_client(&e.doc(), &crate::format::reserved_clients())
    {
        ServiceEntry::CLIENT.set(&mut start, json!(c));
    }
    ServiceEntry::TARGET.set(&mut start, json!(8080));
    e.start_scratch(svc_at.clone(), start);
    let le_at = ServerConfig::WEB.then(WebConfig::LETSENCRYPT).path();
    e.start_scratch(le_at.clone(), e.peek(&le_at).unwrap_or_else(|| json!({})));
    let (svc_at, le_at) = (StoredValue::new(svc_at), StoredValue::new(le_at));
    on_cleanup(move || {
        let _ = e.take_scratch(&svc_at.get_value());
        let _ = e.take_scratch(&le_at.get_value());
    });
    (svc_at, le_at)
}

#[derive(Clone, Copy)]
struct Fields {
    web_on: Memo<bool>,
    https_in: RwSignal<String>,
    http_in: RwSignal<String>,
    p: Memo<Ports>,
    names: RwSignal<Vec<String>>,
    site: Memo<Value>,
    asks_email: Memo<bool>,
    email: Memo<String>,
    service: Memo<String>,
    name_issue: Signal<Option<String>>,
    https: Memo<Option<String>>,
    at: [StoredValue<Path>; 2],
}

fn problem_of(f: Fields) -> (Memo<Option<String>>, Memo<bool>) {
    let s = store();
    let e = s.edit;
    let local = Memo::new(move |_| {
        e.local.with(|l| {
            l.iter()
                .find(|(p, _)| f.at.iter().any(|a| p.starts_with(&a.get_value())))
                .map(|(_, m)| m.clone())
        })
    });
    let problem = Memo::new(move |_| {
        let i = Inputs {
            web_on: f.web_on.get(),
            https_in: f.https_in.get(),
            http_in: f.http_in.get(),
            ports: f.p.get(),
            names: f.names.get(),
            site: f.site.get(),
            asks_email: f.asks_email.get(),
            email: f.email.get(),
            service: f.service.get(),
            name_issue: f.name_issue.get(),
            local: local.get(),
            https: f.https.get(),
        };
        s.schema.with(|sc| {
            sc.as_ref()
                .and_then(|sc| website_problem(&i, &sc.root, &e.doc()))
        })
    });
    let at_field = Memo::new(move |_| {
        problem.get().is_some() && (problem.get() == local.get() || problem.get() == f.https.get())
    });
    (problem, at_field)
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
    let (svc_at, le_at) = start_scratch(e);
    let site = Memo::new(move |_| e.value(&svc_at.get_value()).unwrap_or_default());
    let client = Memo::new(move |_| {
        ServiceEntry::CLIENT
            .get(&site.get())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let email = Memo::new(move |_| {
        e.value(&email_path())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    });
    let tls = RwSignal::new(
        if p.get_untracked().https {
            TlsMode::Auto
        } else {
            TlsMode::Off
        }
        .wire()
        .to_string(),
    );
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
    Effect::new(move |_| {
        let m = asks_email
            .get()
            .then(|| email_problem(&email.get()))
            .flatten();
        e.local_error(&email_path(), m);
    });
    let https = Memo::new(move |_| https_problem(&names.get(), &tls.get(), &web.get(), p.get()));
    let (problem, at_field) = problem_of(Fields {
        web_on,
        https_in,
        http_in,
        p,
        names,
        site,
        asks_email,
        email,
        service,
        name_issue,
        https,
        at: [svc_at, le_at],
    });
    let finish = move |_| {
        if problem.get_untracked().is_some() {
            return;
        }
        let name = service.get_untracked();
        let scratch = e.take_scratch(&svc_at.get_value()).unwrap_or_default();
        let _ = e.take_scratch(&le_at.get_value());
        if !web_on.get_untracked() {
            e.set(&ServerConfig::WEB.path(), Some(web.get_untracked()));
        }
        let value = site_value(&names.get_untracked(), &scratch, &tls.get_untracked());
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
        go(Route::Detail(ServiceEntry::section().into(), name.clone()));
        if tls.get_untracked() == crate::domain::website::files_choice() {
            let at = ServiceEntry::WEB
                .of(&name)
                .into_iter()
                .chain([burrow_config::WebSite::TLS.key().to_string()])
                .collect::<Vec<_>>();
            set_timeout(
                move || crate::form::go_to_field(&at),
                std::time::Duration::from_millis(150),
            );
        }
    };
    let diagram = Signal::derive(move || {
        let target = ServiceEntry::TARGET
            .get(&site.get())
            .map(crate::field::plain)
            .unwrap_or_default();
        path(&names.get(), &client.get(), &target, &web_port(plain.get()))
    });
    view! {
        <div data-testid="new-website">
            {crate::screens::crumbs(Route::List(ServiceEntry::section().into()), crate::screens::plural(ServiceEntry::section()).to_string(), "New website".into(), Signal::stored(false), None)}
            <burrow_ui_kit::PageHeader sub="A name visitors type, and the app on one of your clients that answers it.">"New website"</burrow_ui_kit::PageHeader>
            {move || (!web_on.get()).then(|| web_step(https_in, http_in))}
            <PathDiagram data=diagram testid="new-website-path"/>
            {steps(names, check_name, tls, p, https)}
            {move || asks_email.get().then(email_step)}
            <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="new-website-name">
                {move || step(if asks_email.get() { 6 } else { 5 }, "Service name", "A short name for lists, commands and Review.")}
                {crate::rename::name_input("new-website-service", "Service name", svc_name, name_issue, true, "blog or shop")}
                {super::save_in::save_in_field(svc_name, save_in)}
            </section>
            {move || problem.get().filter(|_| !names.with(Vec::is_empty) && !at_field.get()).map(|m| view! {
                <p class="mb-2 flex items-center gap-1.5 text-sm text-danger" data-testid="new-website-problem">
                    <burrow_ui_kit::Icon name="alert-circle" small=true/>
                    {m}
                </p>
            })}
            <burrow_ui_kit::StickyBar>
                <span data-testid="new-website-service">{burrow_ui_kit::bar_status(Signal::derive(move || problem.get().unwrap_or_else(|| format!("Adds service {} to your changes", service.get()))))}</span>
                <span class="ml-auto"></span>
                <Button label="Cancel" variant=burrow_ui_kit::ButtonLook::Ghost testid="create-cancel" on_click=move |_| { open.set(false); draft_name.set(String::new()); }/>
                <Button
                    label="Add to changes"
                    variant=burrow_ui_kit::ButtonLook::Primary
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
    tls: RwSignal<String>,
    p: Memo<Ports>,
    https: Memo<Option<String>>,
) -> impl IntoView {
    let on_names = Callback::new(move |l: Vec<String>| names.set(l));
    let row = |f: Path| crate::form::field_row(crate::field::concrete(&f, crate::edit::NEW), f);
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
            {step(1, "Names", "The names visitors type.")}
            <div class="md:col-span-2">
                <ChipInput
                    id="new-website-names"
                    items=Signal::derive(move || names.get())
                    on_change=on_names
                    check=check_name
                    placeholder="Type a name and press Enter"
                />
                {move || names.with(|l| l.iter().find_map(|t| check_name.run(t.clone()).map(|m| format!("{t}: {m}")))).map(|m| view! { <div data-testid="names-problem">{burrow_ui_kit::field_message(burrow_ui_kit::Tone::Danger, m)}</div> })}
                <p class="mt-1.5 text-xs text-text-muted">"For example blog.example.com, *.apps.example.com or 203.0.113.10."</p>
            </div>
            {step(2, "Client", "The machine that reaches the app.")}
            <div data-testid="new-website-client">{row(ServiceEntry::CLIENT.schema())}</div>
            <div></div>
            {step(3, "App on it", "Where the client finds the app.")}
            <div data-testid="new-website-target">{row(ServiceEntry::TARGET.schema())}</div>
            <div></div>
            {step(4, "HTTPS", "Who does HTTPS for this website.")}
            <div class="md:col-span-2" data-testid="new-website-tls">
                {move || view! {
                    <burrow_ui_kit::RadioList name="new-website-tls" options=Signal::derive(move || tls_options(p.get())) value=Signal::derive(move || tls.get()) label="HTTPS" on_change=move |m: String| tls.set(m)/>
                }}
                {move || https.get().filter(|_| !names.with(Vec::is_empty)).map(|m| view! { <div data-testid="new-website-tls-problem">{burrow_ui_kit::field_message(burrow_ui_kit::Tone::Danger, m)}</div> })}
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

fn email_step() -> impl IntoView {
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-testid="new-website-email">
            {step(5, "Let's Encrypt", "Automatic certificates for this server.")}
            <div>
                {crate::form::field_row(email_path(), email_path())}
                <p class="mt-1.5 text-xs text-text-muted">
                    <a href=TERMS target="_blank" rel="noopener noreferrer" class="font-semibold text-accent-ink">"Let's Encrypt's terms"</a>
                </p>
            </div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_config::admin_schema::AdminSchema;

    fn inputs() -> Inputs {
        Inputs {
            web_on: true,
            https_in: String::new(),
            http_in: String::new(),
            ports: ports_of(Some(&json!({ "https": 443, "http": 80 }))),
            names: vec!["blog.example.com".into()],
            site: json!({ "client": "home", "target": 8080 }),
            asks_email: true,
            email: "ops@example.com".into(),
            service: "blog".into(),
            name_issue: None,
            local: None,
            https: None,
        }
    }

    fn problem(i: &Inputs) -> Option<String> {
        website_problem(i, &ServerConfig::node(), &json!({}))
    }

    #[test]
    fn a_complete_website_has_no_problem() {
        assert_eq!(problem(&inputs()), None);
    }

    #[test]
    fn the_email_is_checked_like_validate_checks_it() {
        let mut i = inputs();
        i.email = "a@b".into();
        assert!(problem(&i).is_some());
        i.email = "ops@example.com".into();
        i.asks_email = false;
        i.email = String::new();
        assert_eq!(problem(&i), None);
    }

    #[test]
    fn a_certificate_problem_blocks_the_website_and_local_ca_takes_only_lan_names() {
        let mut i = inputs();
        i.https = Some("Let's Encrypt cannot check it".into());
        assert_eq!(
            problem(&i).as_deref(),
            Some("Let's Encrypt cannot check it")
        );
        let p = ports_of(Some(&json!({ "https": 443, "http": 80 })));
        let names = ["blog.example.com".to_string()];
        assert!(https_problem(&names, "internal", &json!({}), p).is_some());
        assert_eq!(
            https_problem(&["nas.lan".into()], "internal", &json!({}), p),
            None
        );
        assert_eq!(https_problem(&names, "auto", &json!({}), p), None);
        let closed = ports_of(Some(&json!({ "https": 8443 })));
        assert!(https_problem(&names, "auto", &json!({}), closed).is_some());
    }

    #[test]
    fn my_own_certificate_writes_an_empty_choice_the_site_page_fills() {
        let v = site_value(&["blog.example.com".to_string()], &inputs().site, "files");
        assert_eq!(v["web"]["tls"], json!({}));
    }

    #[test]
    fn the_target_is_checked_like_validate_checks_it() {
        for bad in [json!("x:y:z"), json!(70_000), json!("")] {
            let mut i = inputs();
            i.site = json!({ "client": "home", "target": bad });
            assert!(problem(&i).is_some(), "{bad}");
        }
    }

    #[test]
    fn every_field_the_wizard_writes_is_a_field_row_or_named_as_its_own() {
        let v = site_value(
            &["blog.example.com".to_string()],
            &inputs().site,
            TlsMode::Auto.wire(),
        );
        for key in v.as_object().unwrap().keys() {
            let row = site_rows().iter().any(|r| r.last() == Some(key));
            let own = OWN_CONTROLS.contains(&key.as_str());
            assert!(
                row || own,
                "{key} is written but is neither a field row nor a named wizard control"
            );
        }
    }
}
