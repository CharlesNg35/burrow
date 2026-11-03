use burrow_admin_logic::own_update::{ServerFigure, figure};
use burrow_admin_logic::presence::{ClientInfo, Mood, of, ready_names, rollout_line, skipped};
use burrow_admin_proto::features::{CLIENT_UPDATES, SERVER_UPDATES};
use burrow_admin_proto::msg::req::{UPDATE_CLIENTS, UPDATE_SERVER};
use burrow_admin_proto::updates::{
    FIRST_REPORTING, UpdateClients, UpdateClientsReply, UpdateServer, UpdateServerReply,
    rollback_window,
};
use burrow_ui_kit::{Button, ButtonLook, ButtonSize, Stat, Tone, VersionTag, tag_class};
use leptos::prelude::*;

use super::update_dialog::{Confirm, update_dialog};
use crate::store::{OwnJob, Store, store};

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
            let why = c.blocked.clone().unwrap_or_default();
            view! { <VersionTag text=c.version.clone() muted=c.older tag=tag why=why/> }.into_any()
        }
        None => ().into_any(),
    }
}

pub fn system_cell(c: Option<ClientInfo>) -> AnyView {
    let Some(c) = c else {
        return ().into_any();
    };
    view! {
        <span class="text-[13px] whitespace-nowrap text-text-muted" data-testid="client-system">{c.system.clone().unwrap_or_default()}</span>
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
        Some(c) => c.system_short(),
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
        let card = Memo::new(move |_| info.with(|c| c.as_ref().map(|c| c.card(offered(s)))));
        let side = move || {
            let card = card.get()?;
            let tag = card.tag.map(|(t, m, why)| view! { <span class=tag_class(tone(m)) title=why data-testid="client-update-tag">{t}</span> });
            let button = card.can_ask.then(|| view! {
                <Button label="Update" variant=ButtonLook::Primary size=ButtonSize::Small icon="download" testid="client-update" on_click=move |_| open.set(true)/>
            });
            Some(view! { {tag}{button} })
        };
        let hint = move || {
            info.with(|c| {
                c.as_ref()
                    .and_then(ClientInfo::older_note)
                    .unwrap_or_default()
            })
        };
        view! {
            <Stat label="Burrow" value=Signal::derive(move || info.with(|c| shown_version(c.as_ref()))) detail=Signal::derive(move || info.with(|c| shown_detail(c.as_ref()))) title=Signal::derive(hint)>
                {side}
            </Stat>
        }
    }

    pub fn below(self) -> impl IntoView {
        let s = store();
        let (info, open) = (self.info, self.open);
        let c = Signal::derive(move || {
            let c = info.get().unwrap_or_default();
            let window = rollback_window();
            Confirm {
                title: format!("Update {}?", c.name),
                from: Some(c.version.clone()),
                to: c.available.clone().unwrap_or_default(),
                names: None,
                notes: vec![
                    (
                        "warning",
                        format!("{} restarts; its services pause for a few seconds.", c.name),
                    ),
                    (
                        "revert",
                        format!(
                            "If it doesn't reconnect within {window}, it goes back to {}.",
                            c.version
                        ),
                    ),
                ],
                action: format!("Update {}", c.name),
            }
        });
        let go = move || {
            let c = info.get_untracked().unwrap_or_default();
            ask(s, vec![c.name], false, c.available);
        };
        update_dialog(open, "client-update-dialog", "client-update-confirm", c, go)
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
    let c = Signal::derive(move || {
        let names = ready();
        let skipped = s
            .status
            .with(|st| st.as_deref().map(skipped).unwrap_or_default());
        let one =
            burrow_admin_logic::entity::info(&burrow_admin_proto::schema::Entity::Client).singular;
        let count = burrow_admin_logic::wording::count_of(names.len(), one);
        let reasons: Vec<String> = skipped
            .iter()
            .map(|(n, why)| format!("{n}: {why}"))
            .collect();
        let (shown, more) = burrow_admin_logic::top::some_names(&reasons);
        let short = match (reasons.is_empty(), more) {
            (true, _) => String::new(),
            (false, Some(m)) => format!("({shown} {m}, skipped)"),
            (false, None) => format!("({shown}, skipped)"),
        };
        Confirm {
            title: format!("Update {count} to {}?", to()),
            from: None,
            to: to(),
            names: Some((names.join(", "), short, reasons.join("\n"))),
            notes: vec![
                (
                    "warning",
                    "One at a time; each pauses for a few seconds.".to_string(),
                ),
                (
                    "alert-circle",
                    "Stops at the first client that doesn't come back.".to_string(),
                ),
            ],
            action: format!("Update {count}"),
        }
    });
    let go = move || ask(s, Vec::new(), true, Some(to()));
    view! {
        {bar}
        {update_dialog(open, "update-all-dialog", "update-all-confirm", c, go)}
    }
}

fn server_offered(s: Store) -> bool {
    s.welcome.with(|w| {
        w.as_ref().is_some_and(|w| {
            w.has(SERVER_UPDATES) && w.you.level == burrow_admin_proto::msg::AdminLevel::Write
        })
    })
}

fn ask_server(s: Store) {
    let body = crate::api::body(&UpdateServer { dry_run: false });
    s.server_updating.set(OwnJob::Asked);
    crate::api::request(UPDATE_SERVER, body, move |r| match r {
        Ok(v) => {
            let reply: UpdateServerReply =
                burrow_admin_proto::serde_json::from_value(v).unwrap_or_default();
            s.toast(
                Tone::Info,
                format!(
                    "Updating the server to {}: it restarts, and this tab reconnects on its own",
                    reply.to
                ),
                None,
            );
        }
        Err(e) => {
            s.server_updating.set(OwnJob::Idle);
            s.toast(Tone::Danger, e.message, None);
        }
    });
}

#[derive(Clone, Copy)]
pub struct ServerUpdate {
    fig: Memo<Option<ServerFigure>>,
    open: RwSignal<bool>,
}

impl ServerUpdate {
    pub fn new() -> Self {
        let s = store();
        ServerUpdate {
            fig: Memo::new(move |_| s.status.with(|st| st.as_deref().and_then(figure))),
            open: RwSignal::new(false),
        }
    }

    pub fn line(self) -> impl IntoView {
        let s = store();
        let (fig, open) = (self.fig, self.open);
        move || {
            let f = fig.get().filter(ServerFigure::says_something)?;
            let state = f.state.clone().map(|(t, m)| view! {
                <span class=format!("whitespace-nowrap {}", tone(m).text()) title=f.detail.clone() data-testid="server-update-state">{t}</span>
            });
            let available = f.available.clone().map(|(t, m)| view! {
                <span class=format!("whitespace-nowrap {}", tone(m).text()) title=f.why_not.clone().unwrap_or_default() data-testid="server-update-tag">{t}</span>
                {f.to.as_deref().and_then(burrow_admin_logic::wording::release_notes).map(|(_, url)| view! {
                    <a href=url target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 whitespace-nowrap font-semibold text-accent-ink" data-testid="server-whats-new">"What's new"<burrow_ui_kit::Icon name="external" small=true/></a>
                })}
            });
            let button = (server_offered(s) && f.can_ask && !s.server_updating.get().busy()).then(|| view! {
                <Button label="Update" variant=ButtonLook::Primary size=ButtonSize::Inline icon="download" testid="server-update" on_click=move |_| open.set(true)/>
            });
            Some(view! {
                <div class="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1.5 text-sm text-text-muted" data-testid="server-burrow">{state}{available}{button}</div>
            })
        }
    }

    pub fn dialog(self) -> impl IntoView {
        let s = store();
        let (fig, open) = (self.fig, self.open);
        let c = Signal::derive(move || {
            let to = fig.with(|f| f.as_ref().and_then(|f| f.to.clone()).unwrap_or_default());
            let from = s
                .status
                .with(|st| st.as_ref().map(|st| st.version.clone()).unwrap_or_default());
            Confirm {
                title: "Update burrow on this server?".into(),
                notes: vec![
                    (
                        "warning",
                        "Clients, visitors and websites pause for a few seconds while it restarts."
                            .into(),
                    ),
                    (
                        "revert",
                        format!(
                            "If {to} isn't healthy within {}, {from} comes back.",
                            rollback_window()
                        ),
                    ),
                ],
                from: Some(from),
                to,
                names: None,
                action: "Update the server".into(),
            }
        });
        update_dialog(
            open,
            "server-update-dialog",
            "server-update-confirm",
            c,
            move || ask_server(s),
        )
    }
}

pub fn follow_server_update() {
    let s = store();
    Effect::new(move |_| {
        let job = s.server_updating.get();
        if !job.busy() {
            return;
        }
        let Some(last) = s.status.with(|st| {
            st.as_deref()
                .and_then(|st| st.server_update.as_ref())
                .and_then(|u| u.last.clone())
        }) else {
            return;
        };
        match (job, last.state.finished()) {
            (OwnJob::Asked, false) => s.server_updating.set(OwnJob::Running),
            (OwnJob::Running, true) => {
                s.server_updating.set(OwnJob::Idle);
                let t = if last.state == burrow_admin_proto::updates::UpdateState::Done {
                    Tone::Success
                } else {
                    Tone::Danger
                };
                s.toast(t, burrow_admin_proto::updates::job_text(&last), None);
            }
            _ => {}
        }
    });
}
