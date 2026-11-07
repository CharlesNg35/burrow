use burrow_admin_logic::wording::capital;
use burrow_admin_proto::codes::{LAST_WRITER, SELF_REMOVED};
use burrow_admin_proto::plan::Refusal;
use burrow_admin_proto::value::get;
use burrow_config::{Entry, VisitorEntry};
use burrow_ui_kit::{
    Callout, Edge, EdgeState, Hop, HopState, Icon, PathData, PathDiagram, Tone, button_class,
};
use leptos::prelude::*;

use crate::store::store;

use burrow_admin_logic::presence::from_ip;

#[derive(Clone)]
pub(super) struct Guard {
    me: String,
    ip: String,
    code: String,
    path: Vec<String>,
}

pub(super) fn guard(refusals: &[Refusal]) -> Guard {
    let s = store();
    let (me, from) = s.welcome.with_untracked(|w| {
        w.as_ref()
            .map(|w| (w.you.visitor.clone(), w.you.from.clone()))
            .unwrap_or_default()
    });
    let code = [SELF_REMOVED, LAST_WRITER]
        .into_iter()
        .find(|c| refusals.iter().any(|r| r.code == *c))
        .map(str::to_string)
        .or_else(|| refusals.first().map(|r| r.code.clone()))
        .unwrap_or_default();
    let path = match code.as_str() {
        LAST_WRITER => VisitorEntry::ADMIN.of(&me),
        _ => VisitorEntry::path(&me),
    };
    Guard {
        me,
        ip: from_ip(&from),
        code,
        path,
    }
}

fn lockout_path(g: &Guard) -> PathData {
    let session = if g.ip.is_empty() {
        "this session".to_string()
    } else {
        format!("this session · {}", g.ip)
    };
    let (above, below) = match g.code.as_str() {
        SELF_REMOVED => ("removed".to_string(), String::new()),
        LAST_WRITER => (
            VisitorEntry::ADMIN.key().to_string(),
            "no writer left".to_string(),
        ),
        other => (other.trim_start_matches("self-").to_string(), String::new()),
    };
    PathData {
        hops: vec![
            Hop {
                icon: "person",
                head: "Visitor".into(),
                value: g.me.clone(),
                mono: true,
                unit: session,
                state: HopState::Current,
                ..Hop::default()
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: "this server".into(),
                dot: Some(Tone::Danger),
                unit: "would refuse this session".into(),
                state: HopState::Bad,
                ..Hop::default()
            },
        ],
        edges: vec![Edge {
            above,
            below,
            state: EdgeState::Error,
        }],
        summary: format!(
            "Visitor {} would lose its admin session on this server",
            g.me
        ),
    }
}

pub(super) fn footer_text(g: &Guard) -> String {
    match g.code.as_str() {
        LAST_WRITER => "Keep at least one visitor with write access.".into(),
        _ => "Change it back to keep your admin access.".into(),
    }
}

#[component]
fn Lockout(refusals: Vec<Refusal>) -> impl IntoView {
    let g = guard(&refusals);
    let codes = refusals
        .iter()
        .map(|r| r.code.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let msg = refusals
        .iter()
        .map(|r| r.message.clone())
        .collect::<Vec<_>>()
        .join("; ");
    let msg = capital(&msg);
    view! {
        <PathDiagram data=lockout_path(&g) testid="lockout-path"/>
        <div class="mb-4" data-testid="lockout" data-refusal=g.code.clone()>
            <Callout tone=Tone::Danger title="This change would lock you out">
                <p>{format!("{msg}. The server refuses changes that remove the current admin session's access.")}</p>
                <p class="mt-1 font-mono text-xs">{format!("guard: {codes}")}</p>
            </Callout>
        </div>
    }
}

#[component]
pub(super) fn Refused(refusals: Vec<Refusal>) -> impl IntoView {
    (!refusals.is_empty()).then(|| view! { <Lockout refusals=refusals/> })
}

pub(super) fn lockout_fixes(g: Guard) -> impl IntoView {
    let e = store().edit;
    let path = g.path.clone();
    let keep = match g.code.as_str() {
        SELF_REMOVED => format!("Keep visitor {}", g.me),
        LAST_WRITER => "Keep write admin".to_string(),
        _ => return None,
    };
    Some(view! {
        <button type="button" class=format!("{} whitespace-nowrap", button_class(burrow_ui_kit::ButtonLook::Tinted, burrow_ui_kit::ButtonSize::Normal)) data-testid="fix-undo" on:click=move |_| {
            let base = e.draft.with_untracked(|d| get(d.base(), &path).cloned());
            e.set(&path, base);
            e.outcome.set(None);
            e.revalidate_now();
        }>
            <Icon name="revert" small=true/>
            {keep}
        </button>
    })
}
