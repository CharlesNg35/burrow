use burrow_admin_proto::Path;
use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Button, ComboOption};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::store;

use super::{new_path, services_for};
use crate::entities::{create_with, name_problem, singular};

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
    let at = vec![section.clone(), crate::edit::NEW.to_string()];
    e.start_scratch(at.clone(), json!({}));
    let at = StoredValue::new(at);
    on_cleanup(move || {
        let _ = e.take_scratch(&at.get_value());
    });
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
    let sec2 = sec.clone();
    let name_issue = Signal::derive(move || {
        let n = name.get();
        (!n.is_empty())
            .then(|| name_problem(&n, &sec, None))
            .flatten()
    });
    let problem = Signal::derive(move || {
        name_issue
            .get()
            .or_else(|| fields_problem(&sec2, at.get_value()))
    });
    let why = move || match (name_issue.get(), problem.get()) {
        (Some(_), _) => "Fix the name first".to_string(),
        (None, Some(p)) => p,
        (None, None) => String::new(),
    };
    let sec3 = StoredValue::new(section.clone());
    let finish = move |apply: bool| {
        let n = name.get_untracked();
        let sec = sec3.get_value();
        if n.is_empty() || untrack(|| problem.get()).is_some() {
            return;
        }
        let scratch = e.take_scratch(&at.get_value()).unwrap_or_default();
        let m = new_value(&sec, &scratch)
            .get(ClientEntry::MAX_CONNECTIONS.key())
            .and_then(Value::as_u64);
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
    let intro = burrow_admin_logic::entity::of_section(&section).intro;
    let sec5 = section.clone();
    let options = Signal::derive(move || services_for(&sec5));
    let sec6 = section.clone();
    let crumb = Route::List(section.clone());
    let plural_s = crate::screens::plural(&section);
    view! {
        <div data-testid="new-entity">
            {crate::screens::crumbs(crumb, plural_s.to_string(), format!("New {what}"), Signal::stored(false), None)}
            <burrow_ui_kit::PageHeader sub=intro>{format!("New {what}")}</burrow_ui_kit::PageHeader>
            <burrow_ui_kit::PathDiagram data=path_data testid="new-path"/>
            <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
                <burrow_ui_kit::SectionHead title="Identity" sub={if what == ServiceEntry::ENTITY { "A short name you'll see in lists and commands." } else { "The name goes into the config on that machine." }}/>
                {crate::rename::name_input("new-name", "Name", name, name_issue, true, burrow_admin_logic::entity::of_section(&section).name_example)}
                {(what != ServiceEntry::ENTITY).then(|| secret_field(secret))}
            </section>
            {(what != ServiceEntry::ENTITY).then(move || limits_section(&sec6, what, name, options, links))}
            <div class="contents" on:keydown=move |ev| if ev.key() == "Enter" { finish(false) }>
            <burrow_ui_kit::StickyBar>
                {burrow_ui_kit::bar_status(Signal::derive(move || {
                    if name.with(String::is_empty) {
                        burrow_admin_logic::wording::TYPE_NAME_FIRST.to_string()
                    } else if let Some(p) = problem.get() {
                        p
                    } else if what == ServiceEntry::ENTITY {
                        "You set the client and target next.".to_string()
                    } else {
                        "The secret is shown once, after apply.".to_string()
                    }
                }))}
                <span class="ml-auto"></span>
                <Button label="Cancel" variant=burrow_ui_kit::ButtonLook::Ghost testid="create-cancel" on_click=move |_| { open.set(false); name.set(String::new()); }/>
                <Button
                    label=if what == ServiceEntry::ENTITY { "Add and edit".to_string() } else { "Add to changes".to_string() }
                    variant=if what == ServiceEntry::ENTITY { burrow_ui_kit::ButtonLook::Primary } else { burrow_ui_kit::ButtonLook::Secondary }
                    testid="create"
                    disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some())
                    reason=Signal::derive(move || if name.get().is_empty() { burrow_admin_logic::wording::TYPE_NAME_FIRST.to_string() } else { why() })
                    on_click=move |_| finish2(false)
                />
                {(what != ServiceEntry::ENTITY).then(|| view! {
                    <Button
                        label="Create and apply"
                        variant=burrow_ui_kit::ButtonLook::Primary
                        testid="create-apply"
                        disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some() || secret.get().is_none())
                        reason=Signal::derive(move || if name.get().is_empty() { burrow_admin_logic::wording::TYPE_NAME_FIRST.to_string() } else if problem.get().is_some() { why() } else if secret.get().is_none() { "Waiting for the server to make a secret".to_string() } else { String::new() })
                        on_click=move |_| finish2(true)
                    />
                })}
            </burrow_ui_kit::StickyBar>
            </div>
        </div>
    }
}

fn rows_of(section: &str) -> Vec<Path> {
    if section == ClientEntry::section() {
        vec![ClientEntry::MAX_CONNECTIONS.schema()]
    } else if section == VisitorEntry::section() {
        vec![VisitorEntry::MAX_CONNECTIONS.schema()]
    } else {
        Vec::new()
    }
}

fn new_value(section: &str, scratch: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for r in rows_of(section) {
        if let (Some(k), Some(v)) = (r.last(), r.last().and_then(|k| scratch.get(k))) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn entry_problem(root: &Node, section: &str, scratch: &Value, doc: &Value) -> Option<String> {
    rows_of(section).iter().find_map(|r| {
        let v = r.last().and_then(|k| scratch.get(k));
        crate::field::problem_at(root, r, v, doc)
    })
}

fn fields_problem(section: &str, at: Path) -> Option<String> {
    let s = store();
    let e = s.edit;
    let local = e.local.with(|l| {
        l.iter()
            .find(|(p, _)| p.starts_with(&at))
            .map(|(_, m)| m.clone())
    });
    local.or_else(|| {
        let scratch = e.value(&at).unwrap_or_default();
        s.schema.with(|sc| {
            sc.as_ref()
                .and_then(|sc| entry_problem(&sc.root, section, &scratch, &e.doc()))
        })
    })
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
    section: &str,
    what: &'static str,
    name: RwSignal<String>,
    options: Signal<Vec<ComboOption>>,
    links: RwSignal<Vec<String>>,
) -> impl IntoView + use<> {
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
    let rows: Vec<AnyView> = rows_of(section)
        .into_iter()
        .map(|r| crate::form::field_row(crate::field::concrete(&r, crate::edit::NEW), r))
        .collect();
    view! {
        <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
            <burrow_ui_kit::SectionHead title={if what == ClientEntry::ENTITY { "Limits and services" } else { "Access and limits" }} sub="Both optional."/>
            <div data-testid="new-max">{rows}</div>
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

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_config::ServerConfig;
    use burrow_config::admin_schema::AdminSchema;

    fn problem(section: &str, scratch: Value) -> Option<String> {
        entry_problem(&ServerConfig::node(), section, &scratch, &json!({}))
    }

    #[test]
    fn maximum_connections_follow_the_schema_range_for_clients_and_visitors() {
        for (section, key) in [
            (ClientEntry::section(), ClientEntry::MAX_CONNECTIONS.key()),
            (VisitorEntry::section(), VisitorEntry::MAX_CONNECTIONS.key()),
        ] {
            assert_eq!(problem(section, json!({})), None, "{section}");
            assert_eq!(problem(section, json!({ key: 8 })), None, "{section}");
            assert!(
                problem(section, json!({ key: 0 })).is_some(),
                "{section}: 0"
            );
            assert!(
                problem(section, json!({ key: 4_294_967_296u64 })).is_some(),
                "{section}: above u32"
            );
            assert!(
                problem(section, json!({ key: "abc" })).is_some(),
                "{section}: abc"
            );
        }
    }

    #[test]
    fn every_field_the_wizard_writes_is_a_field_row() {
        for section in [ClientEntry::section(), VisitorEntry::section()] {
            let written = new_value(section, &json!({ "max_connections": 3 }));
            let rows = rows_of(section);
            for key in written.as_object().unwrap().keys() {
                assert!(
                    rows.iter().any(|r| r.last() == Some(key)),
                    "{section}.{key} is written but not rendered by the field editor"
                );
            }
        }
    }
}
