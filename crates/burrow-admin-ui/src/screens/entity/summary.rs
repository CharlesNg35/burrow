use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::serde_json::Value;
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
    format!("{min} to {max} kept ready")
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

pub(super) fn overview(section: &str, name: &str, stats: bool) -> impl IntoView {
    let s = store();
    let (section, name) = (section.to_string(), name.to_string());
    move || {
        let (section, name) = (section.clone(), name.clone());
        let st = s.status.get();
        let doc = s.edit.doc();
        match section.as_str() {
            CLIENTS => {
                if !stats {
                    return ().into_any();
                }
                let c = st
                    .as_ref()
                    .and_then(|x| x.clients.iter().find(|c| c.name == name))
                    .cloned();
                let max = ClientEntry::MAX_CONNECTIONS
                    .at(&doc, &name)
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        ServerConfig::LIMITS
                            .then(Limits::MAX_CONNECTIONS_PER_CLIENT)
                            .get(&doc)
                            .and_then(Value::as_u64)
                    })
                    .unwrap_or(u64::from(Limits::default().max_connections_per_client));
                let connected = c.as_ref().and_then(|c| c.connected_secs);
                view! {
                    <Stats testid="live">
                        <Stat label="Open" value=c.as_ref().map(|c| format::count(c.open)).unwrap_or_else(|| "0".into()) detail=format!("max {}", format::count(max))/>
                        <Stat label="Idle channels" value=c.as_ref().map(|c| c.idle_channels.to_string()).unwrap_or_else(|| "0".into()) detail=pool_range(&doc)/>
                        <Stat
                            label="Connected"
                            value=connected.map(format::span).unwrap_or_else(|| "—".into())
                            detail=connected.map(since_date).unwrap_or_else(|| c.as_ref().map(|c| format::left_of(&name, c.left_secs_ago, st.as_ref().map(|s| s.uptime_secs).unwrap_or(0))).unwrap_or_default())
                        />

                    </Stats>
                }
                .into_any()
            }
            VISITORS => ().into_any(),
            _ => {
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
                let rows = clients_of(&svc)
                    .into_iter()
                    .map(|c| {
                        let cs = st.as_ref().and_then(|x| x.clients.iter().find(|y| y.name == c)).cloned();
                        let on = cs.as_ref().is_some_and(|y| y.connected);
                        let h = health.iter().find(|h| h.client == c).map(|h| h.up);
                        let why = health.iter().find(|h| h.client == c).and_then(|h| h.reason.clone());
                        let set = aside.iter().find(|a| a.client == c).map(|a| if a.refused { "set aside (refused)" } else { "set aside" }).unwrap_or("not set aside");
                        let label = c.clone();
                        view! {
                            <tr data-row=c.clone()>
                                <th scope="row"><StatusDot on=on text=label/></th>
                                <td class="font-mono text-[13px] text-text-muted max-sm:hidden">{cs.and_then(|y| y.from).unwrap_or_else(|| "offline".into())}</td>
                                <td>{match h {
                                    Some(true) => view! { <span class=tag_class(Tone::Success)>"up"</span> }.into_any(),
                                    Some(false) => view! {
                                        <span class=tag_class(Tone::Danger)>"down"</span>
                                        {why.map(|w| view! { <span class="ml-2 text-xs text-text-muted" data-testid="health-reason">{w}</span> })}
                                    }.into_any(),
                                    None => view! { <span class="text-text-faint">"not checked"</span> }.into_any(),
                                }}</td>
                                <td class="max-sm:hidden">{set}</td>
                            </tr>
                        }
                    })
                    .collect_view();
                let mode = match (
                    clients_of(&svc).len() > 1,
                    ServiceEntry::MODE
                        .get(&svc)
                        .map(plain)
                        .as_deref()
                        .and_then(Mode::from_wire),
                ) {
                    (true, Some(Mode::Balance)) => {
                        "balance · new connections spread over healthy clients".to_string()
                    }
                    (true, _) => {
                        "failover · the first healthy client takes new connections".to_string()
                    }
                    _ => String::new(),
                };
                view! {
                    <div class="mb-4">
                        <Card title="Health per client" sub=mode flush=true testid="health">
                            <Table head=vec!["Client", "~Connected from", "Health", "~Set aside"]>{rows}</Table>
                        </Card>
                    </div>
                }
                .into_any()
            }
        }
    }
}

pub fn health_tag(
    up: Option<usize>,
    clients: usize,
    health: &[burrow_admin_proto::status::HealthStatus],
) -> AnyView {
    let failing = health.iter().filter(|h| !h.up).count();
    match up {
        None => view! { <span class="text-text-faint">"—"</span> }.into_any(),
        Some(0) => view! { <span class=tag_class(Tone::Danger)><Icon name="x" small=true/>"Down"</span> }.into_any(),
        Some(_) if failing > 0 => {
            let n = health.len();
            view! { <span class=tag_class(Tone::Warning)><Icon name="alert" small=true/>{format!("{} of {n} up", n - failing)}</span> }.into_any()
        }
        Some(u) if u < clients => view! { <span class=tag_class(Tone::Warning)><Icon name="alert" small=true/>{format!("{u} of {clients} up")}</span> }.into_any(),
        Some(_) if !health.is_empty() => view! { <span class=tag_class(Tone::Success) title="Health checks pass"><Icon name="check" small=true/>"Healthy"</span> }.into_any(),
        Some(_) => view! { <span class=tag_class(Tone::Success) title="A client is online; no health check is set"><Icon name="check" small=true/>"Up"</span> }.into_any(),
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
            view! { <p class="px-4 py-3 text-sm text-text-muted">"No private service names this visitor. Pick it in a service's Visitors field."</p> }.into_any()
        } else {
            view! {
                <div class="flex flex-wrap gap-2 px-4 py-3">
                    {may.into_iter().map(|(n, up, udp)| {
                        let (dot, state) = match up {
                            Some(true) => ("bg-success", "a client is online"),
                            Some(false) => ("bg-transparent ring-[1.5px] ring-text-faint ring-inset", "no client online"),
                            None => ("bg-text-faint", "status not known yet"),
                        };
                        view! {
                            <Link to=Route::Detail(ServiceEntry::section().into(), n.clone()) class="inline-flex h-[26px] items-center gap-1.5 rounded-sm bg-surface-3 px-2 text-[12.5px] font-semibold text-text no-underline ring-1 ring-border ring-inset hover:ring-border-strong">
                                <span class=format!("size-2 rounded-full {dot}") aria-hidden="true"></span>
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

pub(super) fn served(name: &str) -> impl IntoView {
    let s = store();
    let name = name.to_string();
    move || {
        let doc = s.edit.doc();
        let st = s.status.get();
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
                    ServiceEntry::TARGET.get(v).map(plain).unwrap_or_default(),
                )
            })
            .collect();
        let cards = rows_for(ServiceEntry::section())
            .into_iter()
            .filter(|r| names.contains(&r.key))
            .map(|r| {
                let t = targets.get(&r.key).cloned().unwrap_or_default();
                view! { <div data-target=t><crate::screens::ListCard row=r selected=false/></div> }
            })
            .collect_view();
        view! {
            <div class="mb-4">
                <Card title="Services it serves" flush=true testid="served">
                    <div class="pt-2 pb-1 sm:hidden" data-testid="served-cards">{cards}</div>
                    <div class="max-sm:hidden">
                    <Table head=vec!["Service", "_", "Public", "Target", ">Open", "_Health"]>
                        {rows.iter().map(|(n, v)| super::service_tr(n, v, st.as_deref(), None)).collect_view()}
                    </Table>
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
