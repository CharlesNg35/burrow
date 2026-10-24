use burrow_admin_logic::certs::{Level, notices, rows};
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, ServiceEntry};
use burrow_ui_kit::{Tone, tag_class};
use leptos::prelude::*;

use crate::route::Route;
use crate::store::store;

use super::Item;

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
    let body = rows(status, now)
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
    }
}

#[component]
pub(super) fn Certificates() -> impl IntoView {
    let s = store();
    move || {
        let status = s.status.get()?;
        let now = (s.now.get() / 1000.0) as u64;
        if rows(&status, now).is_empty() {
            return None;
        }
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
        Some(view! {
            <section class="col-span-2 rounded-card border border-border bg-surface md:col-span-6 xl:col-span-12" data-testid="certs-card">
                <header class="border-b border-border px-4 py-3">
                    <h2 class="text-sm font-semibold">"Certificates"</h2>
                </header>
                {table(&status, now)}
                {account}
            </section>
        })
    }
}
