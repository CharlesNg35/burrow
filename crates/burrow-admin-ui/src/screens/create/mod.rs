use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_config::{Limits, ServerConfig};
use burrow_syntax::name::check_name;
use burrow_ui_kit::{
    Button, ComboOption, Dialog, Edge, EdgeState, Hop, HopState, PathData, TextInput, Tone,
};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::{Store, store};

pub fn singular(section: &str) -> &'static str {
    match entity_of_section(section) {
        Some(Entity::Client) => ClientEntry::ENTITY,
        Some(Entity::Visitor) => VisitorEntry::ENTITY,
        _ => ServiceEntry::ENTITY,
    }
}

pub fn taken(section: &str) -> Vec<String> {
    store().edit.draft.with(|d| {
        d.draft()
            .get(section)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    })
}

pub fn name_problem(name: &str, section: &str, except: Option<&str>) -> Option<String> {
    if let Err(e) = check_name(name) {
        return Some(e);
    }
    if section == ClientEntry::section()
        && crate::domain::reserved_clients().iter().any(|r| r == name)
    {
        return Some(format!(
            "{name} is reserved for this server; give the client another name"
        ));
    }
    (taken(section).iter().any(|t| t == name) && except != Some(name))
        .then(|| format!("a {} named {name} already exists", singular(section)))
}

fn template(section: &str) -> Value {
    let mut v = json!({});
    if section == ServiceEntry::section() {
        if let Some(c) = taken(ClientEntry::section()).into_iter().next() {
            ServiceEntry::CLIENT.set(&mut v, Value::String(c));
        }
        ServiceEntry::TARGET.set(&mut v, Value::from(8080));
    }
    v
}

pub fn create_with(
    section: &str,
    name: &str,
    secret: Option<(String, String)>,
    max: Option<u64>,
    links: &[String],
) {
    let e = store().edit;
    let path = vec![section.to_string(), name.to_string()];
    let mut value = template(section);
    if let Some(m) = max
        && value.is_object()
    {
        if section == ClientEntry::section() {
            ClientEntry::MAX_CONNECTIONS.set(&mut value, Value::from(m));
        } else {
            VisitorEntry::MAX_CONNECTIONS.set(&mut value, Value::from(m));
        }
    }
    e.set(&path, Some(value));
    let doc = e.doc();
    for svc in links {
        let Some(v) = ServiceEntry::entry(&doc, svc) else {
            continue;
        };
        if section == ClientEntry::section() {
            let mut names: Vec<String> =
                match (ServiceEntry::CLIENT.get(v), ServiceEntry::CLIENTS.get(v)) {
                    (Some(Value::String(c)), _) => vec![c.clone()],
                    (_, Some(Value::Array(a))) => a
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                };
            names.push(name.to_string());
            let client = ServiceEntry::CLIENT.of(svc);
            let clients = ServiceEntry::CLIENTS.of(svc);
            if names.len() == 1 {
                e.set(&client, Some(Value::String(names.remove(0))));
            } else {
                e.set(&client, None);
                e.set(
                    &clients,
                    Some(Value::Array(names.into_iter().map(Value::String).collect())),
                );
            }
        } else {
            let visitors = ServiceEntry::VISITORS.of(svc);
            let mut list: Vec<Value> = ServiceEntry::VISITORS
                .get(v)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(Value::String(name.to_string()));
            e.set(&visitors, Some(Value::Array(list)));
        }
    }
    if section != ServiceEntry::section() {
        let mut secret_path = vec![section.to_string(), name.to_string()];
        secret_path.extend(if section == ClientEntry::section() {
            ClientEntry::SECRET.rel()
        } else {
            VisitorEntry::SECRET.rel()
        });
        match secret {
            Some((handle, fp)) => e.add_secret(&secret_path, &handle, &fp),
            None => api::request(msg::req::GENERATE_SECRET, json!({}), move |r| {
                if let Ok(Ok(g)) = r.map(serde_json::from_value::<GeneratedSecret>) {
                    e.add_secret(&secret_path, &g.handle, &g.fp);
                }
            }),
        }
    }
}

fn services_for(section: &str) -> Vec<burrow_ui_kit::ComboOption> {
    let s = store();
    let doc = s.edit.doc();
    let status = s.status.get();
    ServiceEntry::entries(&doc)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| {
                    section == ClientEntry::section()
                        || ServiceEntry::BIND.get(v).is_none_or(Value::is_null)
                })
                .map(|(n, v)| {
                    let up = status.as_ref().map(|st| {
                        crate::screens::paths::clients_of(v)
                            .iter()
                            .any(|c| crate::screens::paths::online(Some(st), c))
                    });
                    burrow_ui_kit::ComboOption {
                        value: n.clone(),
                        label: n.clone(),
                        facet: ServiceEntry::BIND
                            .get(v)
                            .filter(|b| !b.is_null())
                            .map(crate::form::plain)
                            .unwrap_or_else(|| "private".into()),
                        dot: up.map(|u| {
                            if u {
                                burrow_ui_kit::Tone::Success
                            } else {
                                burrow_ui_kit::Tone::Neutral
                            }
                        }),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[component]
pub fn NewEntity(
    section: String,
    open: RwSignal<bool>,
    #[prop(optional, into)] draft_name: Option<RwSignal<String>>,
) -> impl IntoView {
    let s = store();
    let e = s.edit;
    let what = singular(&section);
    let name = draft_name.unwrap_or_else(|| RwSignal::new(String::new()));
    let max = RwSignal::new(String::new());
    let links: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let secret: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    if section != ServiceEntry::section() {
        api::request(msg::req::GENERATE_SECRET, json!({}), move |r| {
            if let Ok(Ok(g)) = r.map(serde_json::from_value::<GeneratedSecret>) {
                let _ = secret.try_set(Some((g.handle, g.fp)));
            }
        });
    }
    let sec = section.clone();
    let problem = Signal::derive(move || {
        let n = name.get();
        if n.is_empty() {
            None
        } else {
            name_problem(&n, &sec, None)
        }
    });
    let sec3 = StoredValue::new(section.clone());
    let finish = move |apply: bool| {
        let n = name.get_untracked();
        let sec = sec3.get_value();
        if n.is_empty() || name_problem(&n, &sec, None).is_some() {
            return;
        }
        let m = max.get_untracked().trim().parse::<u64>().ok();
        let clean = !untrack(|| e.dirty());
        create_with(&sec, &n, secret.get_untracked(), m, &links.get_untracked());
        open.set(false);
        name.set(String::new());
        go(Route::Detail(sec.clone(), n));
        if apply {
            e.review.set(None);
            e.auto_apply.set(clean);
            go(Route::Review);
        }
    };
    let finish2 = finish;
    let sec4 = section.clone();
    let path_data = Signal::derive(move || {
        new_path(&sec4, &name.get(), &s.server_name(), links.with(Vec::len))
    });
    let intro = match entity_of_section(&section) {
        Some(Entity::Client) => {
            "A machine that dials out to this server and reaches targets for your services."
        }
        Some(Entity::Visitor) => {
            "A person or machine that reaches private services with burrow visit."
        }
        _ => "A port on this server, or a private target, reached through one of your clients.",
    };
    let sec5 = section.clone();
    let options = Signal::derive(move || services_for(&sec5));
    let client = what == ClientEntry::ENTITY;
    let placeholder_max = placeholder_max(s, client);
    let crumb = Route::List(section.clone());
    let plural_s = match entity_of_section(&section) {
        Some(Entity::Client) => "Clients",
        Some(Entity::Visitor) => "Visitors",
        _ => "Services",
    };
    view! {
        <div data-testid="new-entity">
            <nav class="mb-1.5 flex gap-1.5 text-sm text-text-muted" aria-label="Breadcrumb">
                <crate::route::Link to=crumb class="text-text-muted no-underline hover:text-text">{plural_s}</crate::route::Link>
                <span aria-hidden="true">"/"</span>
                <span class="font-semibold text-text">{format!("New {what}")}</span>
            </nav>
            <header class="mb-[22px]">
                <h1 class="text-[25px] font-bold tracking-[-0.015em]">{format!("New {what}")}</h1>
                <p class="mt-1 text-base text-text-muted">{intro}</p>
            </header>
            <burrow_ui_kit::PathDiagram data=path_data testid="new-path"/>
            <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
                <div class="flex items-baseline gap-2.5 border-b border-border pb-2.5 md:col-span-2">
                    <h3 class="text-[14px] font-bold">"Identity"</h3>
                    <span class="text-[12.5px] text-text-muted">{if what == "service" { "The name is the key in server.yaml." } else { "The name goes into the config on that machine." }}</span>
                </div>
                {name_field(name, problem)}
                {(what != "service").then(|| secret_field(secret))}
            </section>
            {(what != "service").then(|| limits_section(what, name, max, placeholder_max, options, links))}
            <div class="sticky bottom-4 z-20 mt-6 flex flex-wrap items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay" data-sticky-bar="" on:keydown=move |ev| if ev.key() == "Enter" { finish(false) }>
                <span class="flex items-center gap-1.5 text-sm text-text-muted">
                    <burrow_ui_kit::Icon name="info" small=true/>
                    {if what == ServiceEntry::ENTITY {
                        "You set the client and target next."
                    } else {
                        "The secret is shown once, after apply."
                    }}
                </span>
                <span class="ml-auto"></span>
                <Button label="Cancel" variant="ghost" testid="create-cancel" on_click=move |_| { open.set(false); name.set(String::new()); }/>
                <Button
                    label=if what == "service" { "Add and edit".to_string() } else { "Add to changes".to_string() }
                    variant=if what == "service" { "primary" } else { "secondary" }
                    testid="create"
                    disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some())
                    reason=Signal::derive(move || if name.get().is_empty() { "Type a name first".to_string() } else if problem.get().is_some() { "Fix the name first".to_string() } else { String::new() })
                    on_click=move |_| finish2(false)
                />
                {(what != "service").then(|| view! {
                    <Button
                        label="Create and apply"
                        variant="primary"
                        testid="create-apply"
                        disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some() || secret.get().is_none())
                        reason=Signal::derive(move || if name.get().is_empty() { "Type a name first".to_string() } else if problem.get().is_some() { "Fix the name first".to_string() } else if secret.get().is_none() { "Waiting for the server to make a secret".to_string() } else { String::new() })
                        on_click=move |_| finish2(true)
                    />
                })}
            </div>
        </div>
    }
}

fn placeholder_max(s: Store, client: bool) -> String {
    if client {
        let n = s
            .state
            .with_untracked(|st| {
                st.as_ref().and_then(|st| {
                    ServerConfig::LIMITS
                        .then(Limits::MAX_CONNECTIONS_PER_CLIENT)
                        .get(&st.values)
                        .and_then(Value::as_u64)
                })
            })
            .unwrap_or(u64::from(Limits::default().max_connections_per_client));
        format!("Default: {}", crate::format::count(n))
    } else {
        "Unlimited".to_string()
    }
}

fn name_field(name: RwSignal<String>, problem: Signal<Option<String>>) -> impl IntoView {
    view! {
    <div>
        <label class="mb-1.5 block text-sm font-semibold" for="new-name">"Name"</label>
        <div class="relative">
            <TextInput
                id="new-name"
                value=name
                on_input=move |t: String| name.set(t)
                invalid=Signal::derive(move || problem.get().is_some())
                described_by="new-name-msg"
                mono=true
            />
            {move || (!name.get().is_empty() && problem.get().is_none()).then(|| view! {
                <span class="pointer-events-none absolute top-1/2 right-2.5 flex -translate-y-1/2 items-center gap-1 text-xs font-semibold text-success"><burrow_ui_kit::Icon name="check" small=true/>"available"</span>
            })}
        </div>
        <p id="new-name-msg" class="mt-1.5 text-xs text-danger">{move || problem.get()}</p>
        <p class="text-xs text-text-muted">
            {move || (!name.get().is_empty() && problem.get().is_none()).then(|| view! { <span class="sr-only" data-testid="name-available">"available"</span> })}
            "Letters, digits, dot, dash, underscore; up to 64 characters."
        </p>
    </div>
    }
}

fn secret_field(secret: RwSignal<Option<(String, String)>>) -> impl IntoView {
    view! {
    <div>
        <span class="mb-1.5 block text-sm font-semibold">"Secret"</span>
        <div class="flex h-[38px] items-center gap-2 rounded-input border border-border bg-surface-2 px-2.5 text-[13px] dark:bg-surface-3" data-testid="new-secret">
            <burrow_ui_kit::Icon name="passkey" small=true/>
            {move || match secret.get() {
                Some((_, fp)) => view! { <span>"new secret "<span class="font-mono font-semibold">{fp}</span></span> }.into_any(),
                None => view! { <span class="text-text-muted">"generating…"</span> }.into_any(),
            }}
            <span class=format!("ml-auto {}", burrow_ui_kit::tag_class(burrow_ui_kit::Tone::Accent))>"server-generated"</span>
        </div>
        <p class="mt-1.5 text-xs text-text-muted">"The value is shown once, right after you apply."</p>
    </div>
    }
}

fn limits_section(
    what: &'static str,
    name: RwSignal<String>,
    max: RwSignal<String>,
    placeholder_max: String,
    options: Signal<Vec<ComboOption>>,
    links: RwSignal<Vec<String>>,
) -> impl IntoView {
    let link_label = if what == ClientEntry::ENTITY {
        "Serves"
    } else {
        "Services it may use"
    };
    let link_help = move || {
        let n = name.get();
        let who = if n.is_empty() {
            format!("the new {what}")
        } else {
            n
        };
        if what == ClientEntry::ENTITY {
            format!("Adds {who} to the service's clients.")
        } else {
            format!("Adds {who} to the service's visitors.")
        }
    };
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
            <div class="flex items-baseline gap-2.5 border-b border-border pb-2.5 md:col-span-2">
                <h3 class="text-[14px] font-bold">{if what == ClientEntry::ENTITY { "Limits and services" } else { "Access and limits" }}</h3>
                <span class="text-[12.5px] text-text-muted">"Both optional."</span>
            </div>
            <div>
                <span class="mb-1.5 flex items-center gap-2 text-sm font-semibold">{if what == ClientEntry::ENTITY { "Max connections on this server" } else { "Max connections" }}<span class="font-normal text-text-faint">"· optional"</span></span>
                <div class="flex items-center gap-2">
                    <div class="w-56">
                        <TextInput id="new-max" value=max on_input=move |t: String| max.set(t) numeric=true placeholder=placeholder_max.clone() suffix="connections"/>
                    </div>
                </div>
            </div>
            <div>
                <span class="mb-1.5 flex items-center gap-2 text-sm font-semibold">{link_label}<span class="font-normal text-text-faint">"· optional"</span></span>
                <burrow_ui_kit::Combobox
                    id="new-links"
                    options=options
                    selected=links
                    on_change=Callback::new(move |l: Vec<String>| links.set(l))
                    multi=true
                    header="Services"
                    placeholder="Add a service…"
                    label=link_label
                />
                <p class="mt-1.5 text-xs text-text-muted">{link_help}</p>
            </div>
        </section>
    }
}

#[component]
pub fn RenameEntity(section: String, name: String, open: RwSignal<bool>) -> impl IntoView {
    let what = singular(&section);
    let to = RwSignal::new(name.clone());
    let same = {
        let old = name.clone();
        Signal::derive(move || to.with(|t| t.trim() == old))
    };
    let (sec, old) = (section.clone(), name.clone());
    let problem = Signal::derive(move || {
        let n = to.get();
        if n == old {
            None
        } else {
            name_problem(&n, &sec, None)
        }
    });
    let note = match entity_of_section(&section) {
        Some(Entity::Client) => {
            "The client.yaml on that machine must use the new name, or the client cannot connect."
        }
        Some(Entity::Visitor) => {
            "The visitor.yaml on that machine must use the new name, or the visitor cannot connect."
        }
        _ => "Visitors that forward this service by name must use the new name.",
    };
    let target = StoredValue::new((section.clone(), name.clone()));
    let rename_now = Callback::new(move |_: ()| {
        let (sec2, old2) = target.get_value();
        let n = to.get_untracked();
        open.set(false);
        if n != old2 && store().edit.rename(&[sec2.clone(), old2.clone()], &n) {
            go(Route::Detail(sec2, n));
        }
    });
    view! {
        <Dialog open=open title=format!("Rename {what} {name}") testid="rename-entity">
            <label class="mb-1 block font-medium" for="rename-to">"New name"</label>
            <TextInput
                id="rename-to"
                value=to
                on_input=move |t: String| to.set(t)
                invalid=Signal::derive(move || problem.get().is_some())
                described_by="rename-msg"
                mono=true
            />
            <p id="rename-msg" class="mt-1 min-h-5 text-sm text-danger">{move || problem.get()}</p>
            <p class=move || if same.get() { "mb-3 text-sm text-text-muted" } else { "mb-3 text-sm text-warning" }>{note}" References in server.yaml are updated too."</p>
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label="Rename"
                    variant="primary"
                    testid="rename"
                    disabled=Signal::derive(move || problem.get().is_some() || to.get().is_empty() || same.get())
                    reason=Signal::derive(move || if same.get() || to.get().is_empty() { "Type a new name first".to_string() } else if problem.get().is_some() { "Fix the name first".to_string() } else { String::new() })
                    on_click=move |_| rename_now.run(())
                />
            </div>
        </Dialog>
    }
}

pub fn new_path(section: &str, name: &str, host: &str, picked: usize) -> PathData {
    let shown = if name.is_empty() {
        format!("new {}", singular(section))
    } else {
        name.to_string()
    };
    let pending = |icon: &'static str, head: &str, value: &str| Hop {
        icon,
        head: head.into(),
        value: value.into(),
        dot: Some(Tone::Neutral),
        unit: "set next".into(),
        ..Hop::default()
    };
    let added = |above: &str| Edge {
        above: above.into(),
        below: String::new(),
        state: EdgeState::Added,
    };
    if section == ServiceEntry::section() {
        return PathData {
            hops: vec![
                pending("globe", "Who connects", "public or visitors"),
                Hop {
                    icon: "box",
                    head: "Server".into(),
                    value: shown,
                    mono: true,
                    dot: Some(Tone::Neutral),
                    unit: "on this server".into(),
                    state: HopState::Current,
                    badge: None,
                },
                pending("monitor", "Client", "not set"),
                pending("fork", "Target", "not set"),
            ],
            edges: vec![added(""), added(""), added("")],
            summary:
                "A new service: who connects, this server, its client and its target, set next"
                    .into(),
        };
    }
    let visitor = section == VisitorEntry::section();
    PathData {
        hops: vec![
            Hop {
                icon: if visitor { "person" } else { "monitor" },
                head: singular(section).to_uppercase(),
                value: shown,
                mono: true,
                dot: Some(Tone::Neutral),
                unit: "not connected yet".into(),
                state: HopState::Current,
                badge: None,
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: host.into(),
                mono: true,
                dot: Some(Tone::Success),
                unit: "this server".into(),
                ..Hop::default()
            },
            Hop {
                icon: "stack",
                head: "Services".into(),
                value: match picked {
                    0 => "none".into(),
                    1 => "1 service".into(),
                    n => format!("{n} services"),
                },
                dot: (picked == 0).then_some(Tone::Neutral),
                unit: if picked == 0 {
                    "pick below".into()
                } else {
                    String::new()
                },
                ..Hop::default()
            },
        ],
        edges: vec![
            added(if visitor { "will visit" } else { "will dial" }),
            added(if visitor { "may use" } else { "serves" }),
        ],
        summary: format!("A new {} for this server", singular(section)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singular_names() {
        assert_eq!(singular("clients"), "client");
        assert_eq!(singular("services"), "service");
    }

    fn heads(d: &burrow_ui_kit::PathData) -> Vec<(String, String)> {
        d.hops
            .iter()
            .map(|h| (h.head.clone(), h.value.clone()))
            .collect()
    }

    #[test]
    fn a_new_service_path_runs_from_who_connects_to_its_target() {
        let d = new_path("services", "", "127.0.0.1", 0);
        assert_eq!(
            heads(&d),
            [
                ("Who connects".to_string(), "public or visitors".to_string()),
                ("Server".to_string(), "new service".to_string()),
                ("Client".to_string(), "not set".to_string()),
                ("Target".to_string(), "not set".to_string()),
            ]
        );
        assert_eq!(d.edges.len(), 3);
        assert!(
            d.edges.iter().all(|e| e.above.is_empty()),
            "a service neither dials nor serves"
        );
        assert!(
            d.hops.iter().all(|h| h.unit != "pick below"),
            "the service form has no picker"
        );
        assert!(
            d.hops.iter().all(|h| h.dot == Some(Tone::Neutral)),
            "nothing on a new service is set or online yet"
        );
        assert_eq!(heads(&new_path("services", "web", "h", 0))[1].1, "web");
    }

    #[test]
    fn a_new_client_or_visitor_path_runs_through_this_server_to_its_services() {
        let c = new_path("clients", "", "127.0.0.1", 0);
        assert_eq!(
            heads(&c),
            [
                ("CLIENT".to_string(), "new client".to_string()),
                ("Server".to_string(), "127.0.0.1".to_string()),
                ("Services".to_string(), "none".to_string()),
            ]
        );
        assert_eq!(c.edges[0].above, "will dial");
        assert_eq!(c.edges[1].above, "serves");
        let v = new_path("visitors", "phone", "127.0.0.1", 2);
        assert_eq!(heads(&v)[0], ("VISITOR".to_string(), "phone".to_string()));
        assert_eq!(c.hops[2].dot, Some(Tone::Neutral), "no service picked yet");
        assert_eq!(heads(&v)[2].1, "2 services");
        assert_eq!(v.hops[2].dot, None);
        assert_eq!(v.edges[0].above, "will visit");
        assert_eq!(v.edges[1].above, "may use");
    }
}
