use burrow_admin_logic::{addr, free_port};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_admin_proto::{Path, features, msg};
use burrow_config::{Protocol, ServiceEntry};
use burrow_ui_kit::{INPUT_H, Icon};
use leptos::prelude::*;

use super::Addr;
use crate::api;
use crate::store::store;

pub(super) fn offered(path: &Path) -> bool {
    let s = store();
    free_port::is_service_bind(path)
        && s.can_write()
        && s.welcome.with_untracked(|w| {
            w.as_ref()
                .is_some_and(|w| w.has(features::SUGGEST_FREE_PORT))
        })
}

fn sibling(path: &Path, field: &str) -> Path {
    let mut p = path.clone();
    if let Some(last) = p.last_mut() {
        *last = field.to_string();
    }
    p
}

fn ask(
    near: Option<u32>,
    count: u16,
    udp: bool,
    tries: usize,
    used: std::rc::Rc<std::collections::BTreeSet<u16>>,
    done: std::rc::Rc<dyn Fn(u16, u16)>,
) {
    let body = json!({
        "what": msg::suggest::FREE_PORT,
        "near": near,
        "count": count,
        "proto": if udp { Protocol::Udp } else { Protocol::Tcp }.wire(),
    });
    api::request(msg::req::SUGGEST, body, move |r| {
        let Ok(Ok(s)) = r.map(serde_json::from_value::<msg::Suggestions>) else {
            return;
        };
        let Some(start) = s
            .values
            .first()
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
        else {
            return;
        };
        match free_port::clash(start, count, &used) {
            Some(next) if tries > 1 && next <= 65535 => {
                ask(Some(next), count, udp, tries - 1, used, done)
            }
            Some(_) => {}
            None => done(start, count),
        }
    });
}

pub(super) fn button(
    path: Path,
    a: Addr,
    commit: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let e = store().edit;
    let name = path.get(1).cloned().unwrap_or_default();
    let target = sibling(&path, ServiceEntry::TARGET.key());
    let protocol = sibling(&path, ServiceEntry::PROTOCOL.key());
    let size = Memo::new(move |_| {
        let target = addr::count(&addr::split(e.value(&target).as_ref()));
        let own = a.parts.with(addr::count);
        free_port::wanted(a.range_on.get(), target, own)
    });
    let tip = Signal::derive(move || match size.get() {
        Some(n) => free_port::tip(n),
        None => free_port::NEEDS_SIZE.to_string(),
    });
    let class = format!(
        "flex {INPUT_H} w-[38px] shrink-0 cursor-pointer items-center justify-center rounded-input border border-border-strong bg-surface text-text-muted hover:bg-surface-3 hover:text-text disabled:cursor-not-allowed disabled:opacity-55"
    );
    let propose = move |_| {
        let Some(count) = size.get_untracked() else {
            return;
        };
        let udp = e
            .value(&protocol)
            .as_ref()
            .and_then(Value::as_str)
            .and_then(Protocol::from_wire)
            == Some(Protocol::Udp);
        let near = free_port::near_after(&a.parts.with_untracked(|p| p.from.clone()), count);
        let used = std::rc::Rc::new(e.with_saved(|d| free_port::draft_ports(d.draft(), &name)));
        let commit = commit.clone();
        let done: std::rc::Rc<dyn Fn(u16, u16)> =
            std::rc::Rc::new(move |start: u16, count: u16| {
                let first = start.to_string();
                let last = (u32::from(start) + u32::from(count) - 1).to_string();
                a.parts.update(|p| {
                    p.from = first.clone();
                    p.to = if count > 1 {
                        last.clone()
                    } else {
                        String::new()
                    };
                    p.unix = None;
                });
                a.port_text.set(first);
                commit();
            });
        ask(near, count, udp, free_port::TRIES, used, done);
    };
    view! {
        <button
            type="button"
            class=class
            title=tip
            aria-label=tip
            data-testid="free-port"
            disabled=move || size.get().is_none()
            on:click=propose
        >
            <Icon name="wand"/>
        </button>
    }
}
