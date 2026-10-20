use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_syntax::name::check_name;
use burrow_ui_kit::{Button, Dialog, TextInput};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::store;

pub fn singular(section: &str) -> &'static str {
    match section {
        "clients" => "client",
        "visitors" => "visitor",
        _ => "service",
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
    (taken(section).iter().any(|t| t == name) && except != Some(name))
        .then(|| format!("a {} named {name} already exists", singular(section)))
}

fn template(section: &str) -> Value {
    match section {
        "services" => {
            let first = taken("clients").into_iter().next();
            match first {
                Some(c) => json!({ "client": c, "target": 8080 }),
                None => json!({ "target": 8080 }),
            }
        }
        _ => json!({}),
    }
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
    if let (Some(m), Some(obj)) = (max, value.as_object_mut()) {
        obj.insert("max_connections".into(), Value::from(m));
    }
    e.set(&path, Some(value));
    let doc = e.doc();
    for svc in links {
        let entry = vec!["services".to_string(), svc.clone()];
        let Some(v) = doc.get("services").and_then(|m| m.get(svc)) else {
            continue;
        };
        if section == "clients" {
            let mut names: Vec<String> = match (v.get("client"), v.get("clients")) {
                (Some(Value::String(c)), _) => vec![c.clone()],
                (_, Some(Value::Array(a))) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            names.push(name.to_string());
            let mut client = entry.clone();
            client.push("client".into());
            let mut clients = entry.clone();
            clients.push("clients".into());
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
            let mut visitors = entry.clone();
            visitors.push("visitors".into());
            let mut list: Vec<Value> = v
                .get("visitors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(Value::String(name.to_string()));
            e.set(&visitors, Some(Value::Array(list)));
        }
    }
    if section != "services" {
        let secret_path = vec![section.to_string(), name.to_string(), "secret".to_string()];
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
    doc.get("services")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| section == "clients" || v.get("bind").is_none_or(Value::is_null))
                .map(|(n, v)| {
                    let up = status.as_ref().map(|st| {
                        crate::screens::paths::clients_of(v)
                            .iter()
                            .any(|c| crate::screens::paths::online(Some(st), c))
                    });
                    burrow_ui_kit::ComboOption {
                        value: n.clone(),
                        label: n.clone(),
                        facet: v
                            .get("bind")
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
    if section != "services" {
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
        // Applying at once is only for the new entry itself: edits that
        // were already pending still need their review.
        let clean = !untrack(|| e.dirty());
        create_with(&sec, &n, secret.get_untracked(), m, &links.get_untracked());
        open.set(false);
        name.set(String::new());
        go(Route::Detail(sec.clone(), n));
        if apply {
            e.auto_apply.set(clean);
            go(Route::Review);
        }
    };
    let finish2 = finish;
    let sec4 = section.clone();
    let path_data = Signal::derive(move || {
        let n = name.get();
        let host = crate::app::host_line(s);
        let first = links.with(|l| l.first().cloned());
        let shown = if n.is_empty() {
            format!("new {}", singular(&sec4))
        } else {
            n.clone()
        };
        burrow_ui_kit::PathData {
            hops: vec![
                burrow_ui_kit::Hop {
                    icon: if sec4 == "clients" {
                        "monitor"
                    } else if sec4 == "visitors" {
                        "person"
                    } else {
                        "services"
                    },
                    head: singular(&sec4).to_uppercase(),
                    value: shown,
                    mono: true,
                    dot: Some(burrow_ui_kit::Tone::Neutral),
                    unit: "not connected yet".into(),
                    state: burrow_ui_kit::HopState::Current,
                    badge: None,
                },
                burrow_ui_kit::Hop {
                    icon: "box",
                    head: "Server".into(),
                    value: host,
                    mono: true,
                    dot: Some(burrow_ui_kit::Tone::Success),
                    unit: "this server".into(),
                    ..burrow_ui_kit::Hop::default()
                },
                burrow_ui_kit::Hop {
                    icon: "services",
                    head: "Services".into(),
                    value: first.clone().unwrap_or_else(|| "none".into()),
                    unit: if first.is_some() {
                        "added below".into()
                    } else {
                        "pick below".into()
                    },
                    ..burrow_ui_kit::Hop::default()
                },
            ],
            edges: vec![
                burrow_ui_kit::Edge {
                    above: if sec4 == "visitors" {
                        "will visit".into()
                    } else {
                        "will dial".into()
                    },
                    below: String::new(),
                    state: burrow_ui_kit::EdgeState::Added,
                },
                burrow_ui_kit::Edge {
                    above: if sec4 == "visitors" {
                        "may use".into()
                    } else {
                        "serves".into()
                    },
                    below: String::new(),
                    state: burrow_ui_kit::EdgeState::Added,
                },
            ],
            summary: format!("A new {} for this server", singular(&sec4)),
        }
    });
    let intro = match what {
        "client" => {
            "A machine that dials out to this server and reaches targets for your services."
        }
        "visitor" => "A person or machine that reaches private services with burrow visit.",
        _ => "A port on this server, or a private target, reached through one of your clients.",
    };
    let link_label = if what == "client" {
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
        if what == "client" {
            format!("Adds {who} to the service's clients.")
        } else {
            format!("Adds {who} to the service's visitors.")
        }
    };
    let sec5 = section.clone();
    let options = Signal::derive(move || services_for(&sec5));
    let placeholder_max = s.state.with_untracked(|st| {
        st.as_ref()
            .and_then(|st| st.values.get("limits"))
            .and_then(|l| {
                l.get(if what == "client" {
                    "max_connections_per_client"
                } else {
                    "max_connections_per_visitor"
                })
            })
            .and_then(Value::as_u64)
            .unwrap_or(8192)
            .to_string()
    });
    let crumb = Route::List(section.clone());
    let plural_s = match what {
        "client" => "Clients",
        "visitor" => "Visitors",
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
                {(what != "service").then(|| view! {
                    <div>
                        <span class="mb-1.5 block text-sm font-semibold">"Secret"</span>
                        <div class="flex h-[38px] items-center gap-2 rounded-input border border-border bg-surface-2 px-2.5 text-[13px] dark:bg-surface-3" data-testid="new-secret">
                            <burrow_ui_kit::Icon name="key" small=true/>
                            {move || match secret.get() {
                                Some((_, fp)) => view! { <span>"new secret "<span class="font-mono font-semibold">{fp}</span></span> }.into_any(),
                                None => view! { <span class="text-text-muted">"generating…"</span> }.into_any(),
                            }}
                            <span class=format!("ml-auto {}", burrow_ui_kit::tag_class(burrow_ui_kit::Tone::Accent))>"server-generated"</span>
                        </div>
                        <p class="mt-1.5 text-xs text-text-muted">"The value is shown once, right after you apply."</p>
                    </div>
                })}
            </section>
            {(what != "service").then(|| {
                view! {
                    <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
                        <div class="flex items-baseline gap-2.5 border-b border-border pb-2.5 md:col-span-2">
                            <h3 class="text-[14px] font-bold">{if what == "client" { "Limits and services" } else { "Access and limits" }}</h3>
                            <span class="text-[12.5px] text-text-muted">"Both optional."</span>
                        </div>
                        <div>
                            <span class="mb-1.5 flex items-center gap-2 text-sm font-semibold">"Max connections"<span class="font-normal text-text-faint">"· optional"</span></span>
                            <div class="flex items-center gap-2">
                                <div class="w-36">
                                    <TextInput id="new-max" value=max on_input=move |t: String| max.set(t) numeric=true placeholder=placeholder_max.clone() suffix="conn."/>
                                </div>
                                <span class="text-xs text-text-muted">"default from limits"</span>
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
            })}
            <div class="sticky bottom-4 z-20 mt-6 flex flex-wrap items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay" data-sticky-bar="" on:keydown=move |ev| if ev.key() == "Enter" { finish(false) }>
                <span class="flex items-center gap-1.5 text-sm text-text-muted">
                    <burrow_ui_kit::Icon name="info" small=true/>
                    {move || {
                        let n = name.get();
                        if what == "service" {
                            "You set the client and target next.".to_string()
                        } else {
                            format!("The review shows \"New {what} {} (1 secret)\".", if n.is_empty() { "…".to_string() } else { n })
                        }
                    }}
                </span>
                <span class="ml-auto"></span>
                <Button label="Cancel" variant="ghost" testid="create-cancel" on_click=move |_| { open.set(false); name.set(String::new()); }/>
                <Button
                    label=if what == "service" { "Add and edit".to_string() } else { "Add to changes".to_string() }
                    variant=if what == "service" { "primary" } else { "secondary" }
                    testid="create"
                    disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some())
                    on_click=move |_| finish2(false)
                />
                {(what != "service").then(|| view! {
                    <Button
                        label="Create and apply"
                        variant="primary"
                        testid="create-apply"
                        disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some() || secret.get().is_none())
                        on_click=move |_| finish2(true)
                    />
                })}
            </div>
        </div>
    }
}

#[component]
pub fn RenameEntity(section: String, name: String, open: RwSignal<bool>) -> impl IntoView {
    let what = singular(&section);
    let to = RwSignal::new(name.clone());
    let (sec, old) = (section.clone(), name.clone());
    let problem = Signal::derive(move || {
        let n = to.get();
        if n == old {
            None
        } else {
            name_problem(&n, &sec, None)
        }
    });
    let note = match what {
        "client" => {
            "The client.yaml on that machine must use the new name, or the client cannot connect."
        }
        "visitor" => {
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
            <p class="mb-3 text-sm text-warning">{note}" References in server.yaml are updated too."</p>
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label="Rename"
                    variant="primary"
                    testid="rename"
                    disabled=Signal::derive(move || problem.get().is_some() || to.get().is_empty())
                    on_click=move |_| rename_now.run(())
                />
            </div>
        </Dialog>
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
}
