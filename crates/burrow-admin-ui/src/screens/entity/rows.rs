use burrow_admin_logic::web::SiteKind;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Tone, tag_class};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{Link, Route};
use crate::screens::Row;

use super::index::Index;

pub(crate) fn protocol_tag(svc: &Value) -> (String, Tone) {
    let p = ServiceEntry::PROTOCOL
        .get(svc)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into());
    let tone = if Protocol::from_wire(&p) == Some(Protocol::Udp) {
        Tone::Violet
    } else {
        Tone::Accent
    };
    (p.to_uppercase(), tone)
}

pub(crate) fn live_card(section: &'static str, key: &str) -> Option<AnyView> {
    let row = untrack(|| super::row_for(section, key))?;
    let key = key.to_string();
    let live = Memo::new(move |_| super::live::live_text(section, &key));
    Some(view! { <crate::screens::ListCard row=row selected=false live=live/> }.into_any())
}

pub(crate) fn protocol_view(svc: &Value) -> impl IntoView + use<> {
    let (text, tone) = protocol_tag(svc);
    view! { <span class=tag_class(tone)>{text}</span> }
}

pub(crate) fn service_tr(name: &str, svc: &Value, rate: bool) -> impl IntoView + use<> {
    let s = crate::store::store();
    let target = ServiceEntry::TARGET
        .get(svc)
        .map(burrow_admin_logic::web::target_value_shown)
        .unwrap_or_default();
    let name = name.to_string();
    let to = Route::Detail(ServiceEntry::section().into(), name.clone());
    let label = name.clone();
    let exposure = match burrow_admin_logic::web::exposure(svc) {
        Some(b) => view! { <span class="font-mono text-[13px]">{b}</span> }.into_any(),
        None => {
            let w = burrow_admin_logic::web::ways(svc);
            let none = !(w.website || w.visitors);
            let text = burrow_admin_logic::web::without_exposure(w);
            view! { <span class=if none { "text-[13px] text-warning" } else { "text-[13px] text-text-muted" }>{text}</span> }.into_any()
        }
    };
    let open = {
        let name = name.clone();
        move || {
            s.status.with(|st| {
                st.as_ref()
                    .and_then(|st| st.services.iter().find(|y| y.name == name))
                    .map(|x| format::count(x.open))
                    .unwrap_or_default()
            })
        }
    };
    let rate = rate.then(|| {
        let name = name.clone();
        move || {
            let (v, u) =
                burrow_admin_logic::rates::human_bits(s.rates.with(|r| r.service(&name).bytes_in));
            format!("{v} {u}")
        }
    });
    let health = {
        let (name, svc) = (name.clone(), svc.clone());
        move || {
            s.status.with(|st| {
                let status = st.as_deref();
                super::health_tag(
                    status.map(|_| crate::screens::paths::health_count(&name, &svc, status, &[])),
                )
            })
        }
    };
    view! {
        <tr data-row=name>
            <th scope="row"><Link to=to class="font-semibold text-text no-underline hover:underline">{label}</Link></th>
            <td>{protocol_view(svc)}</td>
            <td class="whitespace-normal! break-words">{exposure}</td>
            <td class="max-w-[260px] truncate font-mono text-[13px] text-text-muted" title=target.clone() data-target=target.clone()>{format!("→ {target}")}</td>
            <td class="text-right tabular-nums" data-cell="open">{open}</td>
            {rate.map(|r| view! { <td class="text-right tabular-nums" data-cell="rate">{r}</td> })}
            <td>{health}</td>
        </tr>
    }
}

pub(super) fn client_row(name: &str, ix: &Index, pending: Option<&'static str>, to: Route) -> Row {
    Row {
        key: name.to_string(),
        to,
        icon: burrow_admin_logic::entity::CLIENT_ICON,
        title: name.to_string(),
        line2: if pending.is_some() {
            burrow_admin_logic::wording::NOT_APPLIED.into()
        } else {
            String::new()
        },
        terms: ix.served(name).join(" "),
        pending,
        moment: Some(format::Moment::Client),
        ..Row::default()
    }
}

pub(super) fn visitor_row(
    name: &str,
    v: &Value,
    ix: &Index,
    pending: Option<&'static str>,
    to: Route,
) -> Row {
    let admin = VisitorEntry::ADMIN.get(v).map(plain);
    let reach: Vec<String> = VisitorEntry::REACH
        .get(v)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    Row {
        key: name.to_string(),
        to,
        icon: if admin.is_some() { "shield" } else { "person" },
        title: name.to_string(),
        filters: admin.is_some().then_some("admins").into_iter().collect(),
        title_tag: admin.map(|a| {
            let tone = if a == "write" {
                Tone::Danger
            } else {
                Tone::Accent
            };
            (a, tone)
        }),
        terms: ix
            .may_use(name)
            .iter()
            .chain(&reach)
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        pending,
        moment: Some(format::Moment::Visitor),
        ..Row::default()
    }
}

pub(super) fn service_row(
    name: &str,
    (v, https_on): (&Value, bool),
    pending: Option<&'static str>,
    to: Route,
) -> Row {
    let public = burrow_admin_logic::web::is_public(v);
    let ways = burrow_admin_logic::web::ways(v);
    let protocol = ServiceEntry::PROTOCOL
        .get(v)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into());
    let udp = Protocol::from_wire(&protocol) == Some(Protocol::Udp);
    let mut tags = vec![protocol_tag(v)];
    if ways.public {
        tags.push(("public".into(), Tone::Neutral));
    }
    if ways.website {
        tags.push(("website".into(), Tone::Neutral));
    }
    if burrow_admin_logic::web::site_kind(v, https_on) == Some(SiteKind::Plain) {
        tags.push(("unencrypted".into(), Tone::Warning));
    }
    if ways.visitors {
        tags.push((ServiceEntry::VISITORS.key().into(), Tone::Neutral));
    }
    if !(ways.public || ways.website || ways.visitors) {
        tags.push((burrow_admin_logic::web::NO_WAY_IN.into(), Tone::Warning));
    }
    Row {
        key: name.to_string(),
        to,
        icon: if public { "globe" } else { "lock" },
        dot: Some(Tone::Success),
        title: name.to_string(),
        line2: super::live::service_line(v),
        tags,
        group: "Up".into(),
        pending,
        filters: [
            ways.public.then_some(crate::route::PUBLIC_FILTER),
            Some(if udp { Protocol::Udp } else { Protocol::Tcp }.wire()),
            ways.website.then_some(crate::route::WEBSITES_FILTER),
            ways.visitors.then_some(ServiceEntry::VISITORS.key()),
        ]
        .into_iter()
        .flatten()
        .collect(),
        actions: Vec::new(),
        ..Row::default()
    }
}
