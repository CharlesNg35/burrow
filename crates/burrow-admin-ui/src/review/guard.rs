use burrow_admin_logic::wording::capital;
use burrow_admin_proto::plan::Refusal;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::get;
use burrow_config::{Entry, VisitorEntry};
use burrow_syntax::grant::{SELF_DEMOTED, SELF_REMOVED, SELF_SECRET, SELF_SOURCE, is_grant_code};
use burrow_ui_kit::{
    Callout, Edge, EdgeState, Hop, HopState, Icon, PathData, PathDiagram, Tone, button_class,
};
use leptos::prelude::*;

use crate::store::store;

use super::session_ip;

#[derive(Clone)]
pub(super) struct Guard {
    me: String,
    ip: String,
    code: String,
    path: Vec<String>,
    base: Vec<String>,
    now: Vec<String>,
}

fn list_at(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(crate::form::plain).collect())
        .unwrap_or_default()
}

pub(super) fn guard(refusals: &[Refusal]) -> Guard {
    let s = store();
    let e = s.edit;
    let (me, from) = s.welcome.with_untracked(|w| {
        w.as_ref()
            .map(|w| (w.you.visitor.clone(), w.you.from.clone()))
            .unwrap_or_default()
    });
    let code = [SELF_REMOVED, SELF_DEMOTED, SELF_SOURCE, SELF_SECRET]
        .into_iter()
        .find(|c| refusals.iter().any(|r| r.code == *c))
        .map(str::to_string)
        .or_else(|| refusals.first().map(|r| r.code.clone()))
        .unwrap_or_default();
    let path = match code.as_str() {
        SELF_DEMOTED => VisitorEntry::ADMIN.of(&me),
        SELF_SOURCE => VisitorEntry::ADMIN_FROM.of(&me),
        _ => VisitorEntry::path(&me),
    };
    let base = e.draft.with_untracked(|d| list_at(get(d.base(), &path)));
    let now = list_at(e.peek(&path).as_ref());
    Guard {
        me,
        ip: session_ip(&from),
        code,
        path,
        base,
        now,
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
        SELF_DEMOTED => (
            VisitorEntry::ADMIN.key().to_string(),
            "not write".to_string(),
        ),
        SELF_SOURCE => (
            VisitorEntry::ADMIN_FROM.key().to_string(),
            if g.now.is_empty() {
                "empty".to_string()
            } else {
                g.now.join(", ")
            },
        ),
        SELF_SECRET => (
            VisitorEntry::SECRET.key().to_string(),
            "removed".to_string(),
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
        SELF_SOURCE if !g.base.is_empty() => format!(
            "Keep {} in Admin only from, or apply from an allowed address.",
            g.base.join(", ")
        ),
        SELF_SOURCE => "Undo the Admin only from change, or apply from an allowed address.".into(),
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
fn Granted(refusals: Vec<Refusal>) -> impl IntoView {
    let code = refusals.first().map(|r| r.code.clone()).unwrap_or_default();
    let codes = refusals
        .iter()
        .map(|r| r.code.clone())
        .collect::<Vec<_>>()
        .join(", ");
    view! {
        <div class="mb-4" data-testid="granted" data-refusal=code>
            <Callout tone=Tone::Danger title="Admin access is granted in the config file">
                {refusals.iter().map(|r| view! { <p>{format!("{}.", capital(&r.message))}</p> }).collect_view()}
                <p class="mt-1 font-mono text-xs">{format!("guard: {codes}")}</p>
            </Callout>
        </div>
    }
}

#[component]
pub(super) fn Refused(refusals: Vec<Refusal>) -> impl IntoView {
    let (granted, lockouts): (Vec<Refusal>, Vec<Refusal>) =
        refusals.into_iter().partition(|r| is_grant_code(&r.code));
    view! {
        {(!lockouts.is_empty()).then(|| view! { <Lockout refusals=lockouts/> })}
        {(!granted.is_empty()).then(|| view! { <Granted refusals=granted/> })}
    }
}

pub(super) fn lockout_fixes(g: Guard) -> impl IntoView {
    let e = store().edit;
    let (p1, p2, ip2) = (g.path.clone(), g.path.clone(), g.ip.clone());
    let fixable = matches!(
        g.code.as_str(),
        SELF_REMOVED | SELF_DEMOTED | SELF_SOURCE | SELF_SECRET
    );
    let can_allow =
        g.code == SELF_SOURCE && !g.ip.is_empty() && e.peek(&g.path).is_none_or(|v| v.is_array());
    let keep = match g.code.as_str() {
        SELF_REMOVED => format!("Keep visitor {}", g.me),
        SELF_DEMOTED => "Keep write admin".to_string(),
        SELF_SOURCE if !g.base.is_empty() => format!("Keep {}", g.base.join(", ")),
        _ => "Undo this change".to_string(),
    };
    let ip = g.ip.clone();
    view! {
        {can_allow.then(|| view! {
            <button type="button" class=format!("{} whitespace-nowrap", button_class("tinted", false)) data-testid="fix-allow" on:click=move |_| {
                let mut list = e.peek(&p1).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                list.push(Value::String(ip2.clone()));
                e.set(&p1, Some(Value::Array(list)));
                e.outcome.set(None);
                e.revalidate_now();
            }>
                <Icon name="plus" small=true/>
                {format!("Also allow {ip}")}
            </button>
        })}
        {fixable.then(|| view! {
            <button type="button" class=format!("{} whitespace-nowrap", button_class("tinted", false)) data-testid="fix-undo" on:click=move |_| {
                let base = e.draft.with_untracked(|d| get(d.base(), &p2).cloned());
                e.set(&p2, base);
                e.outcome.set(None);
                e.revalidate_now();
            }>
                <Icon name="revert" small=true/>
                {keep.clone()}
            </button>
        })}
    }
}
