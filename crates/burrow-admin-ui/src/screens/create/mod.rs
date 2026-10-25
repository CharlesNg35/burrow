mod new;
mod website;

use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_syntax::name::{MAX_NAME_LEN, check_name};
use burrow_ui_kit::{Button, Dialog, Edge, EdgeState, Hop, HopState, PathData, TextInput, Tone};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::store;

pub(crate) use new::NewEntity;
pub(crate) use website::NewWebsite;

pub fn singular(section: &str) -> &'static str {
    match entity_of_section(section) {
        Some(Entity::Client) => ClientEntry::ENTITY,
        Some(Entity::Visitor) => VisitorEntry::ENTITY,
        _ => ServiceEntry::ENTITY,
    }
}

pub(super) fn name_input(
    id: &'static str,
    label: &'static str,
    name: RwSignal<String>,
    problem: Signal<Option<String>>,
    available: bool,
) -> impl IntoView {
    let msg = format!("{id}-msg");
    let ok = move || available && !name.get().is_empty() && problem.get().is_none();
    view! {
    <div>
        <label class="mb-1.5 block text-sm font-semibold" for=id>{label}</label>
        <div class="relative">
            <TextInput
                id=id
                value=name
                on_input=move |t: String| name.set(t)
                invalid=Signal::derive(move || problem.get().is_some())
                described_by=msg.clone()
                mono=true
            />
            {move || ok().then(|| view! {
                <span class="pointer-events-none absolute top-1/2 right-2.5 flex -translate-y-1/2 items-center gap-1 text-xs font-semibold text-success"><burrow_ui_kit::Icon name="check" small=true/>"available"</span>
            })}
        </div>
        <p id=msg class="mt-1.5 text-xs text-danger">{move || problem.get()}</p>
        <p class="text-xs text-text-muted">
            {move || ok().then(|| view! { <span class="sr-only" data-testid="name-available">"available"</span> })}
            {format!("Letters, digits, dot, dash, underscore; up to {MAX_NAME_LEN} characters.")}
        </p>
    </div>
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
                        facet: burrow_admin_logic::web::exposure(v)
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
            {name_input("rename-to", "New name", to, problem, false)}
            <p class=move || if same.get() { "my-3 text-sm text-text-muted" } else { "my-3 text-sm text-warning" }>{note}" References in server.yaml are updated too."</p>
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
