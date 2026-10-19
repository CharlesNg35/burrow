use burrow_admin_logic::rates::human_bytes;
use burrow_admin_logic::tree::{entity_of, referrers};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::as_secret;
use burrow_ui_kit::{
    Badge, Button, Card, EmptyState, Fact, Facts, PageHeader, StatusDot, Table, Tone, button_class,
};
use leptos::prelude::*;

use crate::form::{object_view, plain};
use crate::format;
use crate::route::{Link, Route};
use crate::screens::create::{NewEntity, RenameEntity, singular};
use crate::store::store;

fn entity(section: &str) -> Entity {
    match section {
        "clients" => Entity::Client,
        "visitors" => Entity::Visitor,
        _ => Entity::Service,
    }
}

fn titles(section: &str) -> (&'static str, &'static str, &'static str) {
    match section {
        "clients" => (
            "Clients",
            "Machines that connect out to this server and serve services",
            "No clients yet",
        ),
        "visitors" => (
            "Visitors",
            "People and machines that may use private services or administer the server",
            "No visitors yet",
        ),
        _ => (
            "Services",
            "Ports of clients, published on this server or kept private for visitors",
            "No services yet",
        ),
    }
}

fn secrets(v: &Value) -> usize {
    match v.get("secret") {
        Some(Value::Array(a)) => a.len(),
        Some(x) if as_secret(x).is_some() => 1,
        _ => 0,
    }
}

fn clients_of(svc: &Value) -> Vec<String> {
    match (svc.get("client"), svc.get("clients")) {
        (Some(Value::String(c)), _) => vec![c.clone()],
        (_, Some(Value::Array(a))) => a.iter().map(plain).collect(),
        (_, Some(Value::String(c))) => vec![c.clone()],
        _ => Vec::new(),
    }
}

#[component]
pub fn List(section: String) -> impl IntoView {
    let s = store();
    let (title, subtitle, empty) = titles(&section);
    let sec = section.clone();
    let body = move || {
        let values = s.edit.draft.with(|d| d.draft().clone());
        let base = s
            .edit
            .draft
            .with(|d| d.base().get(&sec).cloned().unwrap_or(Value::Null));
        let items = values
            .get(&sec)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let removed: Vec<String> = base
            .as_object()
            .map(|m| {
                m.keys()
                    .filter(|k| !items.contains_key(*k))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let pending = {
            let base = base.clone();
            move |name: &str| -> Option<&'static str> {
                if base.get(name).is_none() {
                    Some("new")
                } else if removed.iter().any(|r| r == name) {
                    Some("removed")
                } else {
                    None
                }
            }
        };
        if items.is_empty() {
            return view! { <EmptyState title=empty message="Nothing is configured here yet." testid="list-empty"/> }.into_any();
        }
        let status = s.status.get();
        let rates = s.rates.get();
        let (head, rows): (Vec<&str>, Vec<AnyView>) = match sec.as_str() {
            "clients" => (
                vec!["Name", "State", "From", "Open", "Secrets", "Services"],
                items
                    .iter()
                    .map(|(name, v)| {
                        let label = name.clone();
                        let mark = pending(name);
                        let st = status.as_ref().and_then(|st| st.clients.iter().find(|c| &c.name == name)).cloned();
                        let on = st.as_ref().is_some_and(|c| c.connected);
                        let used = values
                            .get("services")
                            .and_then(Value::as_object)
                            .map(|m| m.values().filter(|x| clients_of(x).contains(name)).count())
                            .unwrap_or(0);
                        view! {
                            <tr data-row=name.clone()>
                                <th scope="row"><Link to=Route::Detail("clients".into(), name.clone()) class="font-mono text-sm">{label}</Link>{mark.map(|m| view! { <span class="ml-2 text-xs text-accent" data-pending=m>{m}</span> })}</th>
                                <td><StatusDot on=on text=if on { "online" } else { "offline" }/></td>
                                <td class="font-mono text-sm">{st.as_ref().and_then(|c| c.from.clone()).unwrap_or_default()}</td>
                                <td class="tabular-nums">{st.as_ref().map(|c| format::count(c.open)).unwrap_or_default()}</td>
                                <td class="tabular-nums">{secrets(v)}</td>
                                <td class="tabular-nums">{used}</td>
                            </tr>
                        }
                        .into_any()
                    })
                    .collect(),
            ),
            "visitors" => (
                vec!["Name", "Admin", "Open", "Total", "Services"],
                items
                    .iter()
                    .map(|(name, v)| {
                        let label = name.clone();
                        let mark = pending(name);
                        let st = status.as_ref().and_then(|st| st.visitors.iter().find(|c| &c.name == name)).cloned();
                        let admin = v.get("admin").map(plain).unwrap_or_default();
                        let allowed = values
                            .get("services")
                            .and_then(Value::as_object)
                            .map(|m| {
                                m.values()
                                    .filter(|x| x.get("visitors").is_some_and(|l| l.as_array().is_some_and(|a| a.iter().any(|i| i.as_str() == Some(name)))))
                                    .count()
                            })
                            .unwrap_or(0);
                        view! {
                            <tr data-row=name.clone()>
                                <th scope="row"><Link to=Route::Detail("visitors".into(), name.clone()) class="font-mono text-sm">{label}</Link>{mark.map(|m| view! { <span class="ml-2 text-xs text-accent" data-pending=m>{m}</span> })}</th>
                                <td>{(!admin.is_empty()).then(|| view! { <Badge text=admin.clone() tone=Tone::Accent/> })}</td>
                                <td class="tabular-nums">{st.as_ref().map(|c| format::count(c.open)).unwrap_or_default()}</td>
                                <td class="tabular-nums">{st.as_ref().map(|c| format::count(c.total)).unwrap_or_default()}</td>
                                <td class="tabular-nums">{allowed}</td>
                            </tr>
                        }
                        .into_any()
                    })
                    .collect(),
            ),
            _ => (
                vec!["Name", "Protocol", "Public", "Target", "Clients", "Open", "Total", "Traffic"],
                items
                    .iter()
                    .map(|(name, v)| {
                        let label = name.clone();
                        let mark = pending(name);
                        let st = status.as_ref().and_then(|st| st.services.iter().find(|c| &c.name == name)).cloned();
                        let r = rates.service(name);
                        let protocol = v.get("protocol").map(plain).unwrap_or_else(|| "tcp".into());
                        let clients = clients_of(v)
                            .into_iter()
                            .map(|c| {
                                let on = status.as_ref().and_then(|st| st.clients.iter().find(|x| x.name == c)).is_some_and(|x| x.connected);
                                view! { <StatusDot on=on text=c/> }
                            })
                            .collect_view();
                        view! {
                            <tr data-row=name.clone()>
                                <th scope="row"><Link to=Route::Detail("services".into(), name.clone()) class="font-mono text-sm">{label}</Link>{mark.map(|m| view! { <span class="ml-2 text-xs text-accent" data-pending=m>{m}</span> })}</th>
                                <td><Badge text=protocol/></td>
                                <td class="font-mono text-sm">{v.get("bind").map(plain).unwrap_or_else(|| "Private".into())}</td>
                                <td class="font-mono text-sm">{v.get("target").map(plain).unwrap_or_default()}</td>
                                <td><span class="flex flex-wrap gap-x-3">{clients}</span></td>
                                <td class="tabular-nums" data-cell="open">{st.as_ref().map(|c| format::count(c.open)).unwrap_or_default()}</td>
                                <td class="tabular-nums">{st.as_ref().map(|c| format::count(c.total)).unwrap_or_default()}</td>
                                <td class="tabular-nums">{format!("↓ {} ↑ {}", human_bytes(r.bytes_in), human_bytes(r.bytes_out))}</td>
                            </tr>
                        }
                        .into_any()
                    })
                    .collect(),
            ),
        };
        let gone = base
            .as_object()
            .map(|m| {
                m.keys()
                    .filter(|k| !items.contains_key(*k))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let gone_rows = gone
            .into_iter()
            .map(|n| {
                let label = n.clone();
                let restore = vec![sec.clone(), n.clone()];
                view! {
                    <tr data-row=n.clone() data-pending="removed">
                        <th scope="row"><span class="font-mono text-sm line-through">{label}</span></th>
                        <td colspan="20">
                            <span class="text-sm text-danger">"Removed in your changes. "</span>
                            <button
                                type="button"
                                class=button_class("ghost", true)
                                on:click=move |_| {
                                    let v = s.edit.draft.with_untracked(|d| burrow_admin_logic::changeset::get(d.base(), &restore).cloned());
                                    s.edit.set(&restore, v);
                                }
                            >
                                "Undo"
                            </button>
                        </td>
                    </tr>
                }
            })
            .collect_view();
        let _ = &pending;
        view! {
            <Table head=head testid="entity-list">{rows}{gone_rows}</Table>
        }
        .into_any()
    };
    let adding = RwSignal::new(false);
    let what = singular(&section);
    let can = s.can_write_now();
    view! {
        <PageHeader title=title subtitle=subtitle>
            {can.then(|| view! {
                <Button label=format!("New {what}") variant="primary" testid="new" on_click=move |_| adding.set(true)/>
            })}
        </PageHeader>
        <NewEntity section=section.clone() open=adding/>
        <Card>{body}</Card>
    }
}

#[component]
pub fn Detail(section: String, name: String) -> impl IntoView {
    let s = store();
    let (plural, _, _) = titles(&section);
    let sec = section.clone();
    let nm = name.clone();
    let exists = {
        let path = vec![section.clone(), name.clone()];
        Memo::new(move |_| {
            s.edit
                .draft
                .with(|d| burrow_admin_logic::changeset::get(d.draft(), &path).is_some())
        })
    };
    let body = move || {
        let schema = s.schema.get()?;
        if !exists.get() {
            let back = Route::List(sec.clone());
            return Some(
                view! {
                    <EmptyState title=format!("No {} named {nm}", sec.trim_end_matches('s')) message="It may have been removed or renamed." testid="detail-missing">
                        <Link to=back class=button_class("secondary", false)>{format!("Back to {plural}")}</Link>
                    </EmptyState>
                }
                .into_any(),
            );
        };
        let path = vec![sec.clone(), nm.clone()];
        let node = schema.root.at(&path)?.clone();
        let spath = vec![sec.clone(), "*".to_string()];
        let settings = object_view(&node, path.clone(), spath, path.clone());
        let (sec2, nm2) = (sec.clone(), nm.clone());
        let related = move || {
            crate::form::ctx_now().map(|c| {
                s.edit.draft.track();
                related(&c, &sec2, &nm2)
            })
        };
        let live = live(&sec, &nm);
        Some(
            view! {
                {live}
                <Card title="Settings" testid="detail-settings">{settings}</Card>
                {related}
            }
            .into_any(),
        )
    };
    let crumb = Route::List(section.clone());
    let renaming = RwSignal::new(false);
    if section == "clients"
        && let Ok(h) = set_interval_with_handle(
            move || crate::api::load_state(s),
            std::time::Duration::from_secs(5),
        )
    {
        on_cleanup(move || h.clear());
    }
    let (sec_r, name_r, sec_d, name_d) =
        (section.clone(), name.clone(), section.clone(), name.clone());
    let kind = match entity(&section) {
        Entity::Client => "Client",
        Entity::Visitor => "Visitor",
        _ => "Service",
    };
    view! {
        <nav class="mb-1 flex gap-1 text-sm text-text-muted" aria-label="Breadcrumb">
            <Link to=crumb>{plural}</Link>
            <span aria-hidden="true">" / "</span>
            <span class="font-mono text-sm">{name.clone()}</span>
        </nav>
        <PageHeader title=name.clone() subtitle=kind>
            {(section != "services").then(|| view! { <crate::secrets::ConnectButton section=section.clone() name=name.clone()/> })}
            {(s.can_write_now()).then(|| {
                let path = vec![section.clone(), name.clone()];
                let back = Route::List(section.clone());
                let what = singular(&section);
                view! {
                    <Button label="Rename…" testid="rename-open" on_click=move |_| renaming.set(true)/>
                    <Button
                        label=format!("Delete {what}")
                        variant="danger"
                        testid="delete"
                        on_click=move |_| {
                            s.edit.set(&path, None);
                            crate::route::go(back.clone());
                        }
                    />
                }
            })}
        </PageHeader>
        <RenameEntity section=sec_r name=name_r open=renaming/>
        <div data-testid="detail" data-entity=sec_d data-name=name_d>{body}</div>
    }
}

fn related(c: &crate::form::Ctx, section: &str, name: &str) -> AnyView {
    let refs = referrers(&c.schema, &c.doc, &entity(section), name);
    let (title, empty) = match section {
        "clients" => ("Used by", "No service or visitor refers to this client."),
        "visitors" => ("Services it may use", "No service lists this visitor."),
        _ => ("Referred to by", "Nothing refers to this service."),
    };
    let items = refs
        .into_iter()
        .filter_map(|(owner, field)| {
            let (info, n) = entity_of(&c.schema, &owner)?;
            let section = info.path.first()?.clone();
            let label = format!("{} {n}", info.label.to_lowercase());
            Some(view! {
                <li>
                    <Link to=Route::Detail(section, n.clone()) class="font-mono text-sm">{label}</Link>
                    <span class="text-text-muted">{format!(" ({field})")}</span>
                </li>
            })
        })
        .collect::<Vec<_>>();
    let body = if items.is_empty() {
        view! { <p class="text-text-muted">{empty}</p> }.into_any()
    } else {
        view! { <ul class="list-disc space-y-1 pl-5">{items}</ul> }.into_any()
    };
    view! { <Card title=title testid="detail-related">{body}</Card> }.into_any()
}

fn live(section: &str, name: &str) -> AnyView {
    let s = store();
    let section = section.to_string();
    let name = name.to_string();
    let body = move || {
        let st = s.status.get()?;
        let view = match section.as_str() {
            "clients" => {
                let c = st.clients.iter().find(|c| c.name == name)?.clone();
                let since = match (c.connected_secs, c.left_secs_ago) {
                    (Some(x), _) => format!("connected for {}", format::span(x)),
                    (None, Some(a)) => format!("left {}", format::ago(a)),
                    _ => "never connected".into(),
                };
                view! {
                    <Facts testid="live">
                        <Fact label="State"><StatusDot on=c.connected text=if c.connected { "online" } else { "offline" }/></Fact>
                        <Fact label="From" mono=true>{c.from.clone().unwrap_or_else(|| "—".into())}</Fact>
                        <Fact label="Since">{since}</Fact>
                        <Fact label="Open connections">{format::count(c.open)}</Fact>
                        <Fact label="Idle channels">{format::count(c.idle_channels)}</Fact>
                    </Facts>
                }
                .into_any()
            }
            "visitors" => {
                let v = st.visitors.iter().find(|c| c.name == name)?.clone();
                view! {
                    <Facts testid="live">
                        <Fact label="Open visits">{format::count(v.open)}</Fact>
                        <Fact label="Total visits">{format::count(v.total)}</Fact>
                    </Facts>
                }
                .into_any()
            }
            _ => {
                let v = st.services.iter().find(|c| c.name == name)?.clone();
                let r = s.rates.get().service(&name);
                let health = if v.health.is_empty() {
                    view! { <span class="text-text-muted">"Not checked"</span> }.into_any()
                } else {
                    v.health
                        .iter()
                        .map(|h| view! { <StatusDot on=h.up text=format!("{} {}", h.client, if h.up { "healthy" } else { "down" })/> })
                        .collect_view()
                        .into_any()
                };
                view! {
                    <Facts testid="live">
                        <Fact label="Open"><span data-cell="open">{format::count(v.open)}</span></Fact>
                        <Fact label="Total">{format::count(v.total)}</Fact>
                        <Fact label="Refused">{format::count(v.refused)}</Fact>
                        <Fact label="In">{format!("{} ({})", human_bytes(r.bytes_in), format::bytes(v.bytes_in))}</Fact>
                        <Fact label="Out">{format!("{} ({})", human_bytes(r.bytes_out), format::bytes(v.bytes_out))}</Fact>
                        <Fact label="Health"><span class="flex flex-wrap gap-x-3">{health}</span></Fact>
                    </Facts>
                }
                .into_any()
            }
        };
        Some(view)
    };
    view! { <Card title="Live">{body}</Card> }.into_any()
}
