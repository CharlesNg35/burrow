use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ClientEntry, Entry, ServiceEntry, entity_of_section};
use burrow_config::{Limits, ServerConfig};
use burrow_ui_kit::{Button, ComboOption, TextInput};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::{Store, store};

use super::{create_with, name_problem, new_path, services_for, singular};

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
        Some(Entity::Visitor) => "A person or machine that reaches services with burrow visit.",
        _ => {
            "A port on this server, a website, or a service for your visitors, reached through one of your clients."
        }
    };
    let sec5 = section.clone();
    let options = Signal::derive(move || services_for(&sec5));
    let client = what == ClientEntry::ENTITY;
    let placeholder_max = placeholder_max(s, client);
    let crumb = Route::List(section.clone());
    let plural_s = crate::screens::plural(&section);
    view! {
        <div data-testid="new-entity">
            {crate::screens::crumbs(crumb, plural_s.to_string(), format!("New {what}"), Signal::stored(false), None)}
            <burrow_ui_kit::PageHeader sub=intro>{format!("New {what}")}</burrow_ui_kit::PageHeader>
            <burrow_ui_kit::PathDiagram data=path_data testid="new-path"/>
            <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2">
                <burrow_ui_kit::SectionHead title="Identity" sub={if what == "service" { "The name is the key in server.yaml." } else { "The name goes into the config on that machine." }}/>
                {super::name_input("new-name", "Name", name, problem, true)}
                {(what != "service").then(|| secret_field(secret))}
            </section>
            {(what != "service").then(|| limits_section(what, name, max, placeholder_max, options, links))}
            <div class="contents" on:keydown=move |ev| if ev.key() == "Enter" { finish(false) }>
            <burrow_ui_kit::StickyBar>
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
            </burrow_ui_kit::StickyBar>
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
            <burrow_ui_kit::SectionHead title={if what == ClientEntry::ENTITY { "Limits and services" } else { "Access and limits" }} sub="Both optional."/>
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
