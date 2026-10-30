use burrow_admin_logic::rates::human_bits;
use burrow_admin_logic::top::{some_names, top, view_all};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{EmptyState, StatusDot, Table, Tone, tag_class};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{Link, Query, Route};
use crate::screens::paths::clients_of;
use crate::store::Store;

use super::page::mobile_cards;
use super::{names, service_problems};

pub(super) fn view_all_link(to: Route, query: Query, shown: usize, total: usize) -> impl IntoView {
    view_all(shown, total).map(|t| {
        view! {
            <p class="border-t border-border px-4 py-2.5 text-right text-[13px]">
                <Link to=to query=query class="font-semibold text-accent-ink no-underline hover:underline" testid="view-all">{t}</Link>
            </p>
        }
    })
}

fn names_cell(all: Vec<String>) -> impl IntoView {
    let (shown, more) = some_names(&all);
    let title = more.is_some().then(|| all.join(", "));
    view! {
        <td class="text-text-muted" title=title data-names=all.len().to_string()>
            {shown}
            {more.map(|m| view! { <span class="ml-1.5 whitespace-nowrap" data-testid="names-more">{m}</span> })}
        </td>
    }
}

fn footer(section: &'static str, kept: &[String], total: usize) -> impl IntoView {
    view! {
        {mobile_cards(section, kept)}
        {view_all_link(Route::List(section.into()), None, kept.len(), total)}
    }
}

pub(super) fn services_table(s: Store, values: Memo<Value>) -> AnyView {
    let rate = move |name: &str| {
        let (v, u) = human_bits(s.rates.with(|r| r.service(name).bytes_in));
        format!("{v} {u}")
    };
    let v = values.get();
    let st = s.status.get();
    let m = ServiceEntry::entries(&v).cloned().unwrap_or_default();
    if m.is_empty() {
        return view! { <EmptyState title="No services yet" message="Services publish a port of a client on this server." testid="services-empty"/> }.into_any();
    }
    let bad: Vec<String> = service_problems(&v, st.as_deref())
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let open = |n: &str| {
        st.as_ref()
            .and_then(|st| st.services.iter().find(|x| x.name == n))
            .map_or(0, |x| x.open)
    };
    let (kept, total) = top(m.iter().collect(), |(n, _)| (bad.contains(*n), open(n)));
    let names: Vec<String> = kept.iter().map(|(n, _)| (*n).clone()).collect();
    let rows = kept
        .iter()
        .map(|(name, svc)| {
            crate::screens::entity::service_tr(name, svc, st.as_deref(), Some(rate(name)))
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Service", "_", "Public", "Target", ">Open", ">In", "_Health"] testid="overview-services">{rows}</Table></div> {footer(ServiceEntry::section(), &names, total)} }.into_any()
}

pub(super) fn clients_table(s: Store, values: Memo<Value>) -> AnyView {
    let v = values.get();
    let n = names(&v, ClientEntry::section());
    if n.is_empty() {
        return view! { <EmptyState title="No clients yet" message="A client is a machine that connects out to this server." testid="clients-empty"/> }.into_any();
    }
    let st = s.status.get();
    let of = |name: &str| {
        st.as_ref()
            .and_then(|st| st.clients.iter().find(|c| c.name == name))
            .cloned()
    };
    let serving = |name: &str| {
        ServiceEntry::entries(&v)
            .is_some_and(|m| m.values().any(|x| clients_of(x).iter().any(|c| c == name)))
    };
    let (kept, total) = top(n, |name| {
        let c = of(name);
        let away = !c.as_ref().is_some_and(|c| c.connected) && serving(name);
        (
            away || c.as_ref().is_some_and(|c| c.at_limit),
            c.map_or(0, |c| c.open),
        )
    });
    let rows = kept
        .clone()
        .into_iter()
        .map(|name| {
            let c = of(&name);
            let on = c.as_ref().is_some_and(|c| c.connected);
            let since = c
                .as_ref()
                .and_then(|c| match (c.connected_secs, c.left_secs_ago) {
                    (Some(x), _) => Some(format!("for {}", format::span(x))),
                    (None, Some(a)) => Some(format!("left {}", format::ago(a))),
                    _ => None,
                })
                .unwrap_or_else(|| format::left_of(&name, None, st.as_ref().map(|s| s.uptime_secs).unwrap_or(0)));
            let serves: Vec<String> = ServiceEntry::entries(&v)
                .map(|m| m.iter().filter(|(_, x)| clients_of(x).contains(&name)).map(|(k, _)| k.clone()).collect())
                .unwrap_or_default();
            let label = name.clone();
            view! {
                <tr data-row=name.clone() data-online=on.to_string()>
                    <th scope="row"><Link to=Route::Detail(ClientEntry::section().into(), name.clone()) class="text-text no-underline hover:underline"><StatusDot on=on text=label/></Link></th>
                    <td class="font-mono text-[13px]">{c.as_ref().and_then(|c| c.from.clone()).unwrap_or_default()}</td>
                    <td class="text-right tabular-nums">{c.as_ref().map(|c| c.open).unwrap_or(0)}</td>
                    <td class="text-right tabular-nums">{c.as_ref().map(|c| c.idle_channels).unwrap_or(0)}</td>
                    <td>{since}</td>
                    {names_cell(serves)}
                </tr>
            }
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Client", "From", ">Open", ">Idle channels", "Since", "Serves"] testid="overview-clients">{rows}</Table></div> {footer(ClientEntry::section(), &kept, total)} }.into_any()
}

pub(super) fn visitors_table(s: Store, values: Memo<Value>) -> AnyView {
    let v = values.get();
    let n = names(&v, VisitorEntry::section());
    if n.is_empty() {
        return view! { <EmptyState title="No visitors yet" message="A visitor reaches private services with burrow visit." testid="visitors-empty"/> }.into_any();
    }
    let st = s.status.get();
    let of = |name: &str| {
        st.as_ref()
            .and_then(|st| st.visitors.iter().find(|c| c.name == name))
            .cloned()
    };
    let (kept, total) = top(n, |name| {
        (
            false,
            of(name).map_or(0, |x| x.open * 2 + u64::from(s.in_use(&x))),
        )
    });
    let rows = kept
        .clone()
        .into_iter()
        .map(|name| {
            let x = of(&name);
            let open = x.as_ref().map(|x| x.open).unwrap_or(0);
            let admin = VisitorEntry::ADMIN.at(&v, &name).map(plain);
            let may: Vec<String> = ServiceEntry::entries(&v)
                .map(|m| m.iter().filter(|(_, s)| ServiceEntry::VISITORS.get(s).and_then(Value::as_array).is_some_and(|a| a.iter().any(|y| y.as_str() == Some(name.as_str())))).map(|(k, _)| k.clone()).collect())
                .unwrap_or_default();
            let label = name.clone();
            view! {
                <tr data-row=name.clone()>
                    <th scope="row">
                        <Link to=Route::Detail(VisitorEntry::section().into(), name.clone()) class="text-text no-underline hover:underline">{label}</Link>
                        {x.as_ref().is_some_and(|x| s.in_use(x)).then(|| view! { <span class=format!("ml-2 {}", tag_class(Tone::Success))>"in use now"</span> })}
                    </th>
                    <td>{admin.map(|a| view! { <span class=tag_class(if a == "write" { Tone::Danger } else { Tone::Accent })>{format!("admin · {a}")}</span> })}</td>
                    {names_cell(may)}
                    <td class="text-right tabular-nums">{open}</td>
                    <td class="text-right tabular-nums">{x.as_ref().map(|x| format::count(x.total)).unwrap_or_default()}</td>
                    <td class="text-text-muted" data-last-used="">{x.as_ref().map(|x| if s.in_use(x) { "now".to_string() } else { x.last_seen_secs.map(format::ago).unwrap_or_else(|| "not seen".into()) }).unwrap_or_default()}</td>
                </tr>
            }
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Visitor", "Admin", "Services it may use", ">Open", ">Total", "Last seen"] testid="overview-visitors">{rows}</Table></div> {footer(VisitorEntry::section(), &kept, total)} }.into_any()
}
