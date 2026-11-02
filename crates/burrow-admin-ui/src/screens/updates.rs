use burrow_admin_logic::presence::{ClientInfo, Mood, of, ready_names, rollout_line};
use burrow_admin_proto::features::CLIENT_UPDATES;
use burrow_admin_proto::msg::req::UPDATE_CLIENTS;
use burrow_admin_proto::updates::{UpdateClients, UpdateClientsReply};
use burrow_ui_kit::{Button, ButtonLook, ButtonSize, Card, Dialog, Fact, Facts, Tone, VersionTag};
use leptos::prelude::*;

use crate::store::{Store, store};

pub fn tone(m: Mood) -> Tone {
    match m {
        Mood::Quiet => Tone::Neutral,
        Mood::Good => Tone::Success,
        Mood::Busy => Tone::Info,
        Mood::Offer => Tone::Warning,
        Mood::Bad => Tone::Danger,
    }
}

fn offered(s: Store) -> bool {
    s.can_write()
        && s.welcome
            .with(|w| w.as_ref().is_some_and(|w| w.has(CLIENT_UPDATES)))
}

pub fn info_of(s: Store, name: &str) -> Option<ClientInfo> {
    s.status.with(|st| of(st.as_deref(), name))
}

pub fn version_cell(c: Option<ClientInfo>) -> AnyView {
    match c {
        Some(c) => {
            let tag = c.tag().map(|(t, m)| (t, tone(m)));
            view! { <VersionTag text=c.version.clone() muted=c.older tag=tag/> }.into_any()
        }
        None => ().into_any(),
    }
}

fn ask(s: Store, clients: Vec<String>, all: bool, version: Option<String>) {
    let body = crate::api::body(&UpdateClients {
        clients,
        all,
        version,
        dry_run: false,
    });
    crate::api::request(UPDATE_CLIENTS, body, move |r| match r {
        Ok(v) => {
            let reply: UpdateClientsReply =
                burrow_admin_proto::serde_json::from_value(v).unwrap_or_default();
            if reply.asked.is_empty() {
                let why = reply
                    .skipped
                    .first()
                    .map(|k| k.why.clone())
                    .unwrap_or_default();
                s.toast(Tone::Warning, format!("Nothing to update: {why}"), None);
            } else {
                s.toast(
                    Tone::Success,
                    format!("Asked {} to update to {}", reply.asked.join(", "), reply.to),
                    None,
                );
            }
        }
        Err(e) => s.toast(Tone::Danger, e.message, None),
    });
}

fn facts(c: ClientInfo, latest_known: bool) -> AnyView {
    if let Some(note) = c.older_note() {
        return view! { <p class="text-sm text-text-muted" data-testid="client-older">{note}</p> }
            .into_any();
    }
    if c.system.is_none() {
        return view! { <p class="text-sm text-text-muted">"Shown once it connects."</p> }
            .into_any();
    }
    let last = c.last_update();
    let newer = match (&c.available, latest_known) {
        (Some(v), _) => format!("{v} is available"),
        (None, true) => "this is the latest release".into(),
        (None, false) => "not known yet".into(),
    };
    view! {
        <Facts testid="client-facts">
            <Fact label="Version" mono=true>{c.version}</Fact>
            <Fact label="System">{c.system.unwrap_or_default()}</Fact>
            <Fact label="Runs as">{c.run.unwrap_or_default()}</Fact>
            <Fact label="Remote updates">{c.remote.unwrap_or_default()}</Fact>
            <Fact label="Newer release">{newer}</Fact>
        </Facts>
        {last.map(|(l, m)| view! { <p class=format!("mt-3 text-sm {}", tone(m).text()) data-testid="client-last-update">{l}</p> })}
    }
    .into_any()
}

pub fn client_card(name: String) -> impl IntoView {
    let s = store();
    let open = RwSignal::new(false);
    let key = StoredValue::new(name);
    let c = move || key.with_value(|n| info_of(s, n));
    let latest_known = move || {
        s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.latest.as_ref())
                .is_some_and(|l| l.version.is_some())
        })
    };
    let body = move || match c() {
        Some(c) => facts(c, latest_known()),
        None => {
            view! { <p class="text-sm text-text-muted">"Not seen since the server started."</p> }
                .into_any()
        }
    };
    let button = move || {
        let c = c()?;
        let to = c.available.clone()?;
        offered(s).then(|| {
            let reason = c.blocked.clone().unwrap_or_default();
            let blocked = c.blocked.is_some();
            view! {
                <div class="mt-4">
                    <Button label=format!("Update to {to}") variant=ButtonLook::Primary size=ButtonSize::Small icon="download" disabled=blocked reason=reason testid="client-update" on_click=move |_| open.set(true)/>
                </div>
            }
        })
    };
    let confirm = move || {
        let c = c().unwrap_or_default();
        let to = c.available.clone().unwrap_or_default();
        let (who, from, target) = (c.name.clone(), c.version.clone(), to.clone());
        view! {
            <p class="text-sm">{format!(
                "{who} downloads {to} from the official releases, checks it against the published checksums and restarts with it. If {to} does not connect within a minute, {who} goes back to {from}."
            )}</p>
            <div class="mt-4 flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button label=format!("Update to {target}") variant=ButtonLook::Primary testid="client-update-confirm" on_click=move |_| {
                    open.set(false);
                    ask(s, vec![who.clone()], false, Some(target.clone()));
                }/>
            </div>
        }
    };
    view! {
        <div class="mb-4">
            <Card title="Burrow on this client" testid="client-burrow">
                {body}
                {button}
            </Card>
            <Dialog open=open title=Signal::derive(move || format!("Update {}?", key.get_value())) icon="download" testid="client-update-dialog">
                {confirm}
            </Dialog>
        </div>
    }
}

pub fn update_all_bar() -> impl IntoView {
    let s = store();
    let open = RwSignal::new(false);
    let ready = move || {
        s.status
            .with(|st| st.as_deref().map(ready_names).unwrap_or_default())
    };
    let to = move || {
        s.status
            .with(|st| {
                st.as_ref()
                    .and_then(|st| st.latest.as_ref())
                    .and_then(|l| l.version.clone())
            })
            .unwrap_or_default()
    };
    let bar = move || {
        let line = s.status.with(|st| st.as_deref().and_then(rollout_line));
        let n = ready().len();
        let button = offered(s) && n > 0;
        (line.is_some() || button).then(|| view! {
            <div class="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-2.5" data-testid="updates-bar">
                {line.map(|(t, m)| view! { <p class=format!("text-sm {}", tone(m).text()) data-testid="rollout">{t}</p> })}
                {button.then(|| view! { <span class="ml-auto"><Button label=format!("Update all to {} ({n})", to()) variant=ButtonLook::Secondary size=ButtonSize::Small icon="download" testid="update-all" on_click=move |_| open.set(true)/></span> })}
            </div>
        })
    };
    view! {
        {bar}
        <Dialog open=open title=Signal::derive(move || {
            let n = ready().len();
            let what = burrow_admin_proto::schema::Entity::Client;
            format!("Update {} to {}?", burrow_admin_logic::wording::count_of(n, what.as_str()), to())
        }) icon="download" testid="update-all-dialog">
            <p class="text-sm">{move || format!(
                "One at a time: {}. Each downloads {} from the official releases, checks it and restarts. The next one starts only after the one before has connected again, and the update stops at the first client that does not come back.",
                ready().join(", "),
                to()
            )}</p>
            <div class="mt-4 flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button label="Update all" variant=ButtonLook::Primary testid="update-all-confirm" on_click=move |_| {
                    open.set(false);
                    ask(s, Vec::new(), true, Some(to()));
                }/>
            </div>
        </Dialog>
    }
}
