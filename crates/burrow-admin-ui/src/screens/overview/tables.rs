use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{EmptyState, StatusDot, Table, Tone, tag_class};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{Link, Route};
use crate::screens::paths::clients_of;
use crate::store::Store;

use super::names;
use super::page::mobile_cards;

pub(super) fn services_table(s: Store, values: Memo<Value>) -> AnyView {
    let online = move |name: &str| {
        s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == name))
                .is_some_and(|c| c.connected)
        })
    };
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
    let rows = m
        .iter()
        .map(|(name, svc)| {
            let x = st.as_ref().and_then(|st| st.services.iter().find(|y| &y.name == name)).cloned();
            let protocol = ServiceEntry::PROTOCOL.get(svc).map(plain).unwrap_or_else(|| Protocol::default().wire().into());
            let cs = clients_of(svc);
            let up = cs.iter().filter(|c| online(c)).count();
            let health = crate::screens::entity::health_tag(st.is_some().then_some(up), cs.len(), x.as_ref().map(|x| x.health.as_slice()).unwrap_or(&[]));
            let label = name.clone();
            view! {
                <tr data-row=name.clone()>
                    <th scope="row"><Link to=Route::Detail(ServiceEntry::section().into(), name.clone()) class="text-text no-underline hover:underline">{label}</Link></th>
                    <td><span class=tag_class(if protocol == Protocol::Udp.wire() { Tone::Violet } else { Tone::Accent })>{protocol.to_uppercase()}</span></td>
                    <td>{match ServiceEntry::BIND.get(svc).filter(|b| !b.is_null()) {
                        Some(b) => view! { <span class="font-mono text-[13px]">{plain(b)}</span> }.into_any(),
                        None => view! { <span class="text-text-faint">"private"</span> }.into_any(),
                    }}</td>
                    <td class="font-mono text-[13px] text-text-muted">{ServiceEntry::TARGET.get(svc).map(plain).unwrap_or_default()}</td>
                    <td class="text-right tabular-nums" data-cell="open">{x.as_ref().map(|x| format::count(x.open)).unwrap_or_default()}</td>
                    <td class="text-right tabular-nums" data-cell="rate">{rate(name)}</td>
                    <td>{health}</td>
                </tr>
            }
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Service", "_", "Public", "Target", ">Open", ">In", "_Health"] testid="overview-services">{rows}</Table></div> {mobile_cards(ServiceEntry::section())} }.into_any()
}

pub(super) fn clients_table(s: Store, values: Memo<Value>) -> AnyView {
    let v = values.get();
    let n = names(&v, ClientEntry::section());
    if n.is_empty() {
        return view! { <EmptyState title="No clients yet" message="A client is a machine that connects out to this server." testid="clients-empty"/> }.into_any();
    }
    let st = s.status.get();
    let rows = n
        .into_iter()
        .map(|name| {
            let c = st.as_ref().and_then(|st| st.clients.iter().find(|c| c.name == name)).cloned();
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
                    <td class="text-text-muted">{serves.join(", ")}</td>
                </tr>
            }
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Client", "From", ">Open", ">Idle channels", "Since", "Serves"] testid="overview-clients">{rows}</Table></div> {mobile_cards(ClientEntry::section())} }.into_any()
}

pub(super) fn visitors_table(s: Store, values: Memo<Value>) -> AnyView {
    let v = values.get();
    let n = names(&v, VisitorEntry::section());
    if n.is_empty() {
        return view! { <EmptyState title="No visitors yet" message="A visitor reaches private services with burrow visit." testid="visitors-empty"/> }.into_any();
    }
    let st = s.status.get();
    let rows = n
        .into_iter()
        .map(|name| {
            let x = st.as_ref().and_then(|st| st.visitors.iter().find(|c| c.name == name)).cloned();
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
                    <td class="text-text-muted">{may.join(", ")}</td>
                    <td class="text-right tabular-nums">{open}</td>
                    <td class="text-right tabular-nums">{x.as_ref().map(|x| format::count(x.total)).unwrap_or_default()}</td>
                    <td class="text-text-muted" data-last-used="">{x.as_ref().map(|x| if s.in_use(x) { "now".to_string() } else { x.last_seen_secs.map(format::ago).unwrap_or_else(|| "not seen".into()) }).unwrap_or_default()}</td>
                </tr>
            }
        })
        .collect_view();
    view! { <div class="max-sm:hidden"><Table head=vec!["Visitor", "Admin", "Services it may use", ">Open", ">Total", "Last seen"] testid="overview-visitors">{rows}</Table></div> {mobile_cards(VisitorEntry::section())} }.into_any()
}
