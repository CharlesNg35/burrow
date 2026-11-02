use burrow_admin_logic::allow;
use burrow_admin_logic::health::EntryHealth;
use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::EntryStatus;
use burrow_config::Mode;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_config::{Limits, Pool, ServerConfig};
use burrow_ui_kit::{Card, EmptyState, Icon, Stat, Stats, StatusDot, Table, Tone, tag_class};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{Link, Route};
use crate::screens::paths::clients_of;
use crate::store::store;

use super::list::rows_for;
use super::{CLIENTS, VISITORS};

fn pool_range(doc: &Value) -> String {
    let d = Pool::default();
    let min = ServerConfig::POOL
        .then(Pool::MIN_IDLE)
        .get(doc)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(d.min_idle));
    let max = ServerConfig::POOL
        .then(Pool::MAX_IDLE)
        .get(doc)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(d.max_idle));
    format!("keeps {min} to {max}")
}

fn mode_line(svc: &Value) -> String {
    match (
        burrow_admin_logic::entries::rows(ServiceEntry::CLIENTS.get(svc)).len() > 1,
        ServiceEntry::MODE
            .get(svc)
            .map(plain)
            .as_deref()
            .and_then(Mode::from_wire),
    ) {
        (true, Some(Mode::Balance)) => {
            format!(
                "{}: new connections spread over healthy clients by weight",
                mode_label(Mode::Balance)
            )
        }
        (true, Some(Mode::LeastConnections)) => {
            format!(
                "{}: the healthy client with the fewest open connections takes the next one",
                mode_label(Mode::LeastConnections)
            )
        }
        (true, Some(Mode::IpHash)) => {
            format!(
                "{}: each visitor address stays on one healthy client, chosen by weight",
                mode_label(Mode::IpHash)
            )
        }
        (true, _) => {
            format!(
                "{}: the first healthy client takes new connections",
                mode_label(Mode::Failover)
            )
        }
        _ => String::new(),
    }
}

fn mode_label(m: Mode) -> String {
    burrow_config::labels::option_label::<Mode>(m.wire()).unwrap_or_else(|| m.wire().to_string())
}

fn since_date(secs: u64) -> String {
    let at = js_sys::Date::now() - secs as f64 * 1000.0;
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at));
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "since {} {:02}, {:02}:{:02}",
        months[d.get_month() as usize % 12],
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

fn entry_rows(
    x: Option<&burrow_admin_proto::status::ServiceStatus>,
    st: Option<&burrow_admin_proto::status::Status>,
    svc: &Value,
) -> Vec<EntryStatus> {
    match x.map(|x| x.entries.clone()).filter(|e| !e.is_empty()) {
        Some(listed) => listed,
        None => clients_of(svc)
            .into_iter()
            .map(|c| EntryStatus {
                open: st
                    .and_then(|x| x.clients.iter().find(|y| y.name == c))
                    .map_or(0, |y| y.open),
                client: c,
                target: None,
            })
            .collect(),
    }
}

fn client_stats(name: String) -> impl IntoView {
    let s = store();
    let update = crate::screens::updates::ClientUpdate::new(name.clone());
    let key = StoredValue::new(name);
    let c = Memo::new(move |_| {
        key.with_value(|n| {
            s.status.with(|st| {
                st.as_ref()
                    .and_then(|x| x.clients.iter().find(|c| &c.name == n))
                    .cloned()
            })
        })
    });
    let max = move || {
        let doc = s.edit.doc();
        key.with_value(|name| {
            ClientEntry::MAX_CONNECTIONS
                .at(&doc, name)
                .and_then(Value::as_u64)
                .or_else(|| {
                    ServerConfig::LIMITS
                        .then(Limits::MAX_CONNECTIONS_PER_CLIENT)
                        .get(&doc)
                        .and_then(Value::as_u64)
                })
                .unwrap_or(u64::from(Limits::default().max_connections_per_client))
        })
    };
    let connected = move || c.with(|c| c.as_ref().and_then(|c| c.connected_secs));
    let uptime = move || s.status.with(|st| st.as_ref().map_or(0, |s| s.uptime_secs));
    let ready_label =
        <burrow_config::Pool as burrow_config::admin_schema::AdminSchema>::node().label;
    let open = move || {
        c.with(|c| {
            c.as_ref()
                .map(|c| format::count(c.open))
                .unwrap_or_else(|| "0".into())
        })
    };
    let open_detail = move || {
        if c.with(|c| c.as_ref().is_some_and(|c| c.at_limit)) {
            "at the client's own limit".to_string()
        } else {
            format!("max {}", format::count(max()))
        }
    };
    let ready = move || {
        c.with(|c| {
            c.as_ref()
                .map(|c| c.idle_channels.to_string())
                .unwrap_or_else(|| "0".into())
        })
    };
    let since = move || match connected() {
        Some(secs) => since_date(secs),
        None => c.with(|c| {
            c.as_ref()
                .map(|c| key.with_value(|n| format::left_of(n, c.left_secs_ago, uptime())))
                .unwrap_or_default()
        }),
    };
    view! {
        <Stats testid="live">
            <Stat label="Open" value=Signal::derive(open) detail=Signal::derive(open_detail)/>
            <Stat label=ready_label value=Signal::derive(ready) detail=Signal::derive(move || pool_range(&s.edit.doc()))/>
            <Stat label="Connected" value=Signal::derive(move || connected().map(format::span).unwrap_or_else(|| "—".into())) detail=Signal::derive(since)/>
            {update.stat()}
        </Stats>
        {update.below()}
    }
}

pub(super) fn overview(section: &str, name: &str, stats: bool) -> AnyView {
    match section {
        CLIENTS if stats => client_stats(name.to_string()).into_any(),
        CLIENTS | VISITORS => ().into_any(),
        _ => entry_overview(name, stats).into_any(),
    }
}

fn entry_overview(name: &str, stats: bool) -> impl IntoView {
    let s = store();
    let name = name.to_string();
    move || {
        let name = name.clone();
        let st = s.status.get();
        let doc = s.edit.doc();
        let x = st
            .as_ref()
            .and_then(|x| x.services.iter().find(|c| c.name == name))
            .cloned();
        if stats {
            let r = s.rates.get().service(&name);
            let (i, iu) = human_bits(r.bytes_in);
            let (o, ou) = human_bits(r.bytes_out);
            let limit = ServiceEntry::BANDWIDTH
                .at(&doc, &name)
                .map(|b| format!("limit {}", plain(b)));
            return view! {
                <Stats testid="live">
                    <Stat label="Open" value=x.as_ref().map(|x| format::count(x.open)).unwrap_or_else(|| "0".into()) detail=x.as_ref().map(|x| format!("{} total", format::count(x.total))).unwrap_or_default()/>
                    <Stat label="In" value=i unit=iu detail=limit.clone().unwrap_or_else(|| x.as_ref().map(|x| format::bytes(x.bytes_in)).unwrap_or_default())/>
                    <Stat label="Out" value=o unit=ou detail=limit.unwrap_or_else(|| x.as_ref().map(|x| format::bytes(x.bytes_out)).unwrap_or_default())/>
                    <Stat label="Refused by filters" value=x.as_ref().map(|x| format::count(x.refused)).unwrap_or_else(|| "0".into()) detail="by the allow and deny lists"/>
                    {x.as_ref().and_then(|x| x.http.clone()).map(|h| {
                        let total: u64 = h.responses.iter().sum();
                        view! { <Stat label="Web answers" value=format::count(total) detail=by_class(&h.responses)/> }
                    })}
                </Stats>
            }
            .into_any();
        }
        let svc = ServiceEntry::entry(&doc, &name)
            .cloned()
            .unwrap_or(Value::Null);
        let health = x.as_ref().map(|x| x.health.clone()).unwrap_or_default();
        let aside = x.as_ref().map(|x| x.set_aside.clone()).unwrap_or_default();
        let reserved = crate::domain::reserved_clients();
        let listed = entry_rows(x.as_ref(), st.as_deref(), &svc);
        let default_target = ServiceEntry::TARGET
            .get(&svc)
            .map(burrow_admin_logic::web::target_value_shown)
            .unwrap_or_default();
        let rows = listed
            .into_iter()
            .map(|en| {
                let c = en.client.clone();
                let t = en.target.clone();
                let cs = st.as_ref().and_then(|x| x.clients.iter().find(|y| y.name == c)).cloned();
                let on = cs.as_ref().is_some_and(|y| y.connected);
                let h = burrow_admin_logic::health::entry(&health, &c, t.as_deref(), on);
                let set = format::skip_label(aside.iter().find(|a| a.client == c && a.target == t).map(|a| a.refused));
                let (to, main) = match &t {
                    Some(t) => (format!("→\u{a0}{}", burrow_admin_logic::web::target_shown(t)), false),
                    None => (format!("→\u{a0}{default_target}"), true),
                };
                let label = burrow_admin_logic::choices::label_of(&c, &reserved);
                let open = format::count(en.open);
                view! {
                    <tr data-row=c.clone()>
                        <th scope="row"><StatusDot on=on text=label/><span class="block pl-4 text-xs font-normal text-text-muted break-words" data-testid="entry-target"><span class="font-mono">{to}</span>{main.then(|| view! { <span class="ml-1 whitespace-nowrap">{format!("({})", burrow_admin_logic::wording::MAIN_TARGET)}</span> })}</span></th>
                        <td class="tabular-nums" data-testid="client-open">{open}</td>
                        <td class=burrow_ui_kit::WIDE_ONLY>{match cs.and_then(|y| y.from) {
                            Some(f) => view! { <span class="font-mono text-[13px] text-text-muted">{f}</span> }.into_any(),
                            None => view! { <span class="text-[13px] text-text-faint" aria-label=burrow_admin_logic::wording::OFFLINE>{format::NONE_MARK}</span> }.into_any(),
                        }}</td>
                        <td class="whitespace-normal!">{match h {
                            EntryHealth::Up => view! { <span class=tag_class(Tone::Success)>"up"</span> }.into_any(),
                            EntryHealth::Down(why) => view! {
                                <span class=tag_class(Tone::Danger)>"down"</span>
                                {why.map(|w| view! { <span class="ml-2 text-xs text-text-muted [overflow-wrap:anywhere] @max-[640px]:mt-1 @max-[640px]:ml-0 @max-[640px]:block" data-testid="health-reason">{w}</span> })}
                            }.into_any(),
                            EntryHealth::Offline => view! { <span class="text-[13px] whitespace-nowrap text-text-faint">{burrow_admin_logic::wording::OFFLINE}</span> }.into_any(),
                            EntryHealth::NoCheck => view! { <span class="text-[13px] whitespace-nowrap text-text-faint">{format::NO_CHECK}</span> }.into_any(),
                        }}</td>
                        <td class=burrow_ui_kit::WIDE_ONLY>{set}</td>
                    </tr>
                }
            })
            .collect_view();
        let mode = mode_line(&svc);
        view! {
            <div class="mb-4">
                <Card title="Health per client" sub=mode flush=true testid="health">
                    <Table head=vec!["Client and target", "Open", "~Connected from", "Health", "~Skipped"]>{rows}</Table>
                </Card>
            </div>
        }
        .into_any()
    }
}

pub type HealthMark = (String, Tone, &'static str);

pub fn health_mark(c: burrow_admin_logic::health::Count) -> HealthMark {
    use burrow_admin_logic::health::Level;
    match c.level() {
        Level::Down => (c.label(), Tone::Danger, "x"),
        Level::Partial => (c.label(), Tone::Warning, "alert"),
        Level::Up => (c.label(), Tone::Success, "check"),
    }
}

pub fn mark_view((text, tone, icon): HealthMark) -> AnyView {
    view! { <span class=tag_class(tone) data-health=tone.name()><Icon name=icon small=true/>{text}</span> }.into_any()
}

pub fn health_tag(c: Option<burrow_admin_logic::health::Count>) -> AnyView {
    match c {
        None => view! { <span class="text-text-faint">"—"</span> }.into_any(),
        Some(c) => mark_view(health_mark(c)),
    }
}

pub(super) fn visitor_overview(name: &str) -> impl IntoView {
    let s = store();
    let name = name.to_string();
    move || {
        let doc = s.edit.doc();
        let st = s.status.get();
        let may: Vec<(String, Option<bool>, bool)> = ServiceEntry::entries(&doc)
            .map(|m| {
                m.iter()
                    .filter(|(_, x)| {
                        ServiceEntry::VISITORS
                            .get(x)
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.iter().any(|y| y.as_str() == Some(name.as_str())))
                    })
                    .map(|(n, x)| {
                        let up = st.as_ref().map(|st| {
                            clients_of(x)
                                .iter()
                                .any(|c| st.clients.iter().any(|y| &y.name == c && y.connected))
                        });
                        (n.clone(), up, crate::screens::paths::udp(x))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let admin_only = may.is_empty()
            && VisitorEntry::ADMIN.at(&doc, &name).is_some()
            && VisitorEntry::REACH
                .at(&doc, &name)
                .and_then(Value::as_object)
                .is_none_or(|m| m.is_empty());
        let chips = if admin_only {
            view! { <p class="px-4 py-3 text-sm text-text-muted">"None: this visitor runs burrow admin, not burrow visit."</p> }.into_any()
        } else if may.is_empty() {
            view! { <p class="px-4 py-3 text-sm text-text-muted">"No service lists this visitor yet. Pick it in a service's Visitors field."</p> }.into_any()
        } else {
            view! {
                <div class="flex flex-wrap gap-2 px-4 py-3">
                    {may.into_iter().map(|(n, up, udp)| {
                        let (dot, state) = match up {
                            Some(true) => (Tone::Success, "a client is online"),
                            Some(false) => (Tone::Neutral, "no client online"),
                            None => (Tone::Neutral, "status not known yet"),
                        };
                        view! {
                            <Link to=Route::Detail(ServiceEntry::section().into(), n.clone()) class="inline-flex h-[26px] items-center gap-1.5 rounded-sm bg-surface-3 px-2 text-[12.5px] font-semibold text-text no-underline ring-1 ring-border ring-inset hover:ring-border-strong">
                                <burrow_ui_kit::Dot tone=dot/>
                                {n.clone()}
                                {udp.then(|| view! { <span class=tag_class(Tone::Violet)>"UDP"</span> })}
                                <span class="sr-only">{format!(", {state}")}</span>
                            </Link>
                        }
                    }).collect_view()}
                </div>
                <p class="border-t border-rowline px-4 py-2.5 text-[12.5px] text-text-muted">"Set on each service: its Visitors field lists who may use it."</p>
            }
            .into_any()
        };
        view! {
            <section class="mb-4 rounded-card border border-border bg-surface" data-testid="may-use">
                <div class="border-b border-border px-4 py-3"><h2 class="text-[13.5px] font-bold">"Services it may use"</h2></div>
                {chips}
            </section>
        }
        .into_any()
    }
}

fn allow_line(name: String) -> impl IntoView {
    let s = store();
    let text = move || {
        let feature = s.welcome.with(|w| {
            w.as_ref()
                .is_some_and(|w| w.has(burrow_admin_proto::features::CLIENT_ALLOW))
        });
        s.status.with(|st| {
            let c = st
                .as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == name));
            allow::shown(feature, c)
        })
    };
    view! {
        <p class="mb-4 flex flex-wrap items-baseline gap-x-2 gap-y-0.5 rounded-card border border-border bg-surface px-4 py-3 text-[13.5px]" data-testid="client-allow">
            <span class="font-semibold">"Allows:"</span>
            <span class="font-mono text-[13px] break-words" data-testid="client-allow-value">{text}</span>
            <span class="text-xs text-text-muted">{allow::SET_THERE}</span>
        </p>
    }
}

pub(super) fn served(name: &str) -> impl IntoView {
    view! {
        {allow_line(name.to_string())}
        {served_services(name)}
    }
}

fn served_services(name: &str) -> impl IntoView + use<> {
    let s = store();
    let name = name.to_string();
    move || {
        let doc = s.edit.doc();
        let rows = ServiceEntry::entries(&doc)
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| clients_of(v).contains(&name))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if rows.is_empty() {
            return view! { <div class="mb-4"><EmptyState title="Serves no services" message="Pick this client in a service to publish one of its ports."/></div> }.into_any();
        }
        let names: Vec<String> = rows.iter().map(|(n, _)| n.clone()).collect();
        let targets: std::collections::BTreeMap<String, String> = rows
            .iter()
            .map(|(n, v)| {
                (
                    n.clone(),
                    ServiceEntry::TARGET
                        .get(v)
                        .map(burrow_admin_logic::web::target_value_shown)
                        .unwrap_or_default(),
                )
            })
            .collect();
        let cards = untrack(|| rows_for(ServiceEntry::section()))
            .into_iter()
            .filter(|r| names.contains(&r.key))
            .map(|r| {
                let t = targets.get(&r.key).cloned().unwrap_or_default();
                let key = r.key.clone();
                let live = Signal::derive(move || {
                    super::list::row_for(ServiceEntry::section(), &key)
                        .map(|r| (r.meta, r.line2))
                        .unwrap_or_default()
                });
                view! { <div data-target=t><crate::screens::ListCard row=r selected=false live=live/></div> }
            })
            .collect_view();
        view! {
            <div class="mb-4">
                <Card title="Services it serves" flush=true testid="served">
                    <div class="@container">
                    <div class=format!("pt-2 pb-1 {}", burrow_ui_kit::CARD_VIEW) data-testid="served-cards">{cards}</div>
                    <div class=burrow_ui_kit::TABLE_VIEW>
                    <Table head=vec!["Service", "_", "Public", "Target", ">Open", "_Health"]>
                        {rows.iter().map(|(n, v)| super::service_tr(n, v, false)).collect_view()}
                    </Table>
                    </div>
                    </div>
                </Card>
            </div>
        }
        .into_any()
    }
}

fn by_class(responses: &[u64; 5]) -> String {
    let set: Vec<String> = responses
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(i, n)| format!("{}xx {}", i + 1, format::count(*n)))
        .collect();
    if set.is_empty() {
        "none yet".into()
    } else {
        set.join(" · ")
    }
}
