use burrow_admin_logic::certs::{Level, notices, rows};
use burrow_admin_logic::top::top;
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, ServerConfig, ServiceEntry};
use burrow_ui_kit::{Tone, tag_class};
use leptos::prelude::*;

use crate::route::{self, Route};

use super::Item;
use super::tables::view_all_link;

fn tone(l: Level) -> Tone {
    match l {
        Level::Good => Tone::Success,
        Level::Info => Tone::Info,
        Level::Warn => Tone::Warning,
        Level::Bad => Tone::Danger,
    }
}

pub(super) fn items(status: &Status, now: u64) -> Vec<Item> {
    notices(status, now)
        .into_iter()
        .map(|n| Item {
            key: n.key,
            tone: tone(n.level),
            title: n.title,
            detail: n.detail,
            to: Some(match (n.service, &n.field) {
                (_, Some(_)) => Route::Settings,
                (Some(s), None) => Route::Detail(ServiceEntry::section().into(), s),
                (None, None) => Route::Settings,
            }),
            field: n.field,
        })
        .collect()
}

fn table(status: &Status, now: u64) -> impl IntoView + use<> {
    let (kept, total) = top(rows(status, now), |r| {
        (
            r.problem.is_some()
                || matches!(r.state.1, Level::Warn | Level::Bad)
                || matches!(r.expires.1, Level::Warn | Level::Bad),
            0,
        )
    });
    let shown = kept.len();
    let body = kept
        .into_iter()
        .map(|r| {
            let (state, st) = r.state;
            let (expires, ex) = r.expires;
            let on = r.expires_on.clone();
            let key = r.name.clone();
            let problem = r.problem.map(|p| {
                let link = r.explanation.map(|u| {
                    view! { <a href=u class="ml-1 font-semibold text-accent-ink" target="_blank" rel="noopener noreferrer">"Why"</a> }
                });
                view! { <p class="mt-1 text-xs text-danger" data-testid="cert-problem">{p}{link}</p> }
            });
            view! {
                <tr class="border-b border-rowline align-top last:border-b-0" data-cert=key>
                    <td class="px-4 py-2.5">
                        <span class="font-mono text-[13px]">{r.name}</span>
                        {problem}
                    </td>
                    <td class="px-2 py-2.5 text-[13px]">{r.source}</td>
                    <td class="px-2 py-2.5 text-[13px]">{r.check}</td>
                    <td class="px-2 py-2.5"><span class=tag_class(tone(st))>{state}</span></td>
                    <td class=format!("px-2 py-2.5 text-[13px] {}", if ex == Level::Good || ex == Level::Info { "" } else { tone(ex).text() }) title=(!on.is_empty()).then_some(on)>{expires}</td>
                    <td class="px-2 py-2.5 pr-4 text-[13px]">{r.next}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="overflow-x-auto">
            <table class="w-full min-w-[640px] text-left">
                <thead class="text-xs text-text-muted">
                    <tr class="border-b border-border">
                        <th scope="col" class="px-4 py-2 font-semibold">"Name"</th>
                        <th scope="col" class="px-2 py-2 font-semibold">"Certificate"</th>
                        <th scope="col" class="px-2 py-2 font-semibold">"Check"</th>
                        <th scope="col" class="px-2 py-2 font-semibold">"State"</th>
                        <th scope="col" class="px-2 py-2 font-semibold">"Expires"</th>
                        <th scope="col" class="px-2 py-2 pr-4 font-semibold">"Next renewal"</th>
                    </tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </div>
        {view_all_link(Route::Settings, Some((route::CARD_QUERY, ServerConfig::WEB.key())), shown, total)}
    }
}

pub(super) fn certificates_panel(status: Option<&Status>, now: u64) -> impl IntoView + use<> {
    let Some(status) = status.filter(|st| !rows(st, now).is_empty()) else {
        return view! { <p class="px-4 py-3 text-[13px] text-text-muted" data-testid="certs-empty">"No certificates yet: they appear when a website uses HTTPS."</p> }.into_any();
    };
    let account = status.acme.clone().map(|a| {
        let who = a
            .account
            .map_or_else(|| "not created yet".to_string(), |u| u.clone());
        view! {
            <p class="border-t border-border px-4 py-2.5 text-xs text-text-muted" data-testid="acme-account">
                "Let's Encrypt account: "<span class="font-mono">{who}</span>
                {(!a.state_dir.is_empty()).then(|| view! { <span>" · kept in "<span class="font-mono">{a.state_dir.clone()}</span></span> })}
            </p>
        }
    });
    let queue = status
        .acme
        .as_ref()
        .map(|a| a.queue.clone())
        .unwrap_or_default();
    let orders = (!queue.is_empty()).then(|| {
        let items = queue
            .into_iter()
            .map(|o| {
                let when = if o.running {
                    "ordering now".to_string()
                } else if o.at <= now {
                    "next in line".to_string()
                } else {
                    format!("next try {}", crate::format::relative(o.at))
                };
                view! {
                    <li class="flex flex-wrap gap-x-2 px-4 py-1.5 text-[13px]" data-order=o.service.clone()>
                        <span class="font-semibold">{o.service.clone()}</span>
                        <span class="font-mono text-text-muted">{o.names.join(", ")}</span>
                        <span class="ml-auto">{when}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="border-t border-border py-2" data-testid="cert-orders">
                <p class="px-4 pb-1 text-xs font-semibold text-text-muted">"Certificate orders, one at a time"</p>
                <ul>{items}</ul>
            </div>
        }
    });
    view! {
        <div data-testid="certs-card">
            {table(status, now)}
            {orders}
            {account}
        </div>
    }
    .into_any()
}
