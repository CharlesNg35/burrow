use burrow_admin_logic::presence::{ClientInfo, Mood, of, ready_names, rollout_line};
use burrow_admin_proto::features::CLIENT_UPDATES;
use burrow_admin_proto::msg::req::UPDATE_CLIENTS;
use burrow_admin_proto::updates::{FIRST_REPORTING, UpdateClients, UpdateClientsReply};
use burrow_ui_kit::{Button, ButtonLook, ButtonSize, Dialog, Stat, Tone, VersionTag, tag_class};
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

pub fn system_cell(c: Option<ClientInfo>) -> AnyView {
    let Some(c) = c else {
        return ().into_any();
    };
    let marker = c.refuses().then(|| view! { " · " <span class="text-text-faint" data-testid="client-updates-off">"updates off"</span> });
    view! {
        <span class="text-[13px] whitespace-nowrap text-text-muted" title=c.summary() data-testid="client-system">{c.system_short()}{marker}</span>
    }
    .into_any()
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

fn shown_version(c: Option<&ClientInfo>) -> String {
    match c {
        Some(c) if !c.older && !c.version.is_empty() => c.version.clone(),
        _ => "—".into(),
    }
}

fn shown_detail(c: Option<&ClientInfo>) -> String {
    match c {
        None => "not seen since the server started".into(),
        Some(c) if c.local => "this server".into(),
        Some(c) if c.older => format!("older than {FIRST_REPORTING}"),
        Some(c) if c.system.is_none() => "shown once it connects".into(),
        Some(c) => c.summary(),
    }
}

#[derive(Clone, PartialEq)]
enum Action {
    Nothing,
    Tag(String, Mood),
    Offer {
        tag: String,
        mood: Mood,
        blocked: Option<String>,
    },
}

fn action(c: Option<&ClientInfo>, offered: bool) -> Action {
    let Some(c) = c else {
        return Action::Nothing;
    };
    match c.tag() {
        Some((t, Mood::Busy)) => Action::Tag(t, Mood::Busy),
        Some((tag, mood)) if offered && c.available.is_some() => Action::Offer {
            tag,
            mood,
            blocked: c.blocked.clone(),
        },
        Some((t, m)) => Action::Tag(t, m),
        None => Action::Nothing,
    }
}

#[derive(Clone, Copy)]
pub struct ClientUpdate {
    info: Memo<Option<ClientInfo>>,
    open: RwSignal<bool>,
}

impl ClientUpdate {
    pub fn new(name: String) -> Self {
        let s = store();
        let key = StoredValue::new(name);
        ClientUpdate {
            info: Memo::new(move |_| key.with_value(|n| info_of(s, n))),
            open: RwSignal::new(false),
        }
    }

    pub fn stat(self) -> impl IntoView {
        let s = store();
        let (info, open) = (self.info, self.open);
        let act = Memo::new(move |_| info.with(|c| action(c.as_ref(), offered(s))));
        let facts = move || info.with(|c| shown_detail(c.as_ref()));
        let side = move || {
            match act.get() {
            Action::Nothing => ().into_any(),
            Action::Tag(t, m) => view! { <span class=tag_class(tone(m)) data-testid="client-update-tag">{t}</span> }.into_any(),
            Action::Offer { tag, mood, blocked } => view! {
                <span class=tag_class(tone(mood)) data-testid="client-update-tag">{tag}</span>
                <Button label="Update" variant=ButtonLook::Primary size=ButtonSize::Small icon="download" disabled=blocked.is_some() reason=blocked.unwrap_or_default() testid="client-update" on_click=move |_| open.set(true)/>
            }.into_any(),
        }
        };
        view! {
            <Stat label="Burrow" value=Signal::derive(move || info.with(|c| shown_version(c.as_ref())))>
                <div class="text-xs text-text-muted break-words" data-testid="client-facts">{facts}</div>
                {move || (act.get() != Action::Nothing).then(|| view! { <div class="mt-2 flex flex-wrap items-center gap-2">{side}</div> })}
            </Stat>
        }
    }

    pub fn below(self) -> impl IntoView {
        let s = store();
        let (info, open) = (self.info, self.open);
        let line = move || {
            info.with(|c| {
                let c = c.as_ref()?;
                if let Some(note) = c.older_note() {
                    return Some(view! { <p class="-mt-3 mb-5 text-sm text-text-muted" data-testid="client-older">{note}</p> }.into_any());
                }
                let (l, m) = c.last_update()?;
                Some(view! { <p class=format!("-mt-3 mb-5 text-sm {}", tone(m).text()) data-testid="client-last-update">{l}</p> }.into_any())
            })
        };
        let confirm = move || {
            let c = info.get().unwrap_or_default();
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
            {line}
            <Dialog open=open title=Signal::derive(move || format!("Update {}?", info.with(|c| c.as_ref().map(|c| c.name.clone()).unwrap_or_default()))) icon="download" testid="client-update-dialog">
                {confirm}
            </Dialog>
        }
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
            <div class="flex flex-wrap items-center gap-2" data-testid="updates-bar">
                {line.map(|(t, m)| view! { <p class=format!("text-xs {}", tone(m).text()) data-testid="rollout">{t}</p> })}
                {button.then(|| view! { <span><Button label=format!("Update all to {} ({n})", to()) variant=ButtonLook::Secondary size=ButtonSize::Small icon="download" testid="update-all" on_click=move |_| open.set(true)/></span> })}
            </div>
        })
    };
    view! {
        {bar}
        <Dialog open=open title=Signal::derive(move || {
            let n = ready().len();
            let one = burrow_admin_logic::entity::info(&burrow_admin_proto::schema::Entity::Client).singular;
            format!("Update {} to {}?", burrow_admin_logic::wording::count_of(n, one), to())
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
