use std::collections::VecDeque;

use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_ui_kit::{
    EmptyState, Icon, Kbd, Kpi, LineChart, PillTabs, Segmented, Select, StatusDot, Table, Tone,
    button_class, tag_class,
};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens::paths::clients_of;
use crate::store::store;

use super::attention::Attention;
use super::{chart, healthy_detail, names, service_problems};

#[component]
fn Traffic() -> impl IntoView {
    let s = store();
    let service = RwSignal::new(String::new());
    let window = RwSignal::new(900usize);
    let series = Memo::new(move |_| {
        let svc = service.get();
        s.recorder.with(|r| {
            r.series((!svc.is_empty()).then_some(svc.as_str()))
                .map(|(a, b)| {
                    (
                        a.points().collect::<Vec<_>>(),
                        b.points().collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let data = Signal::derive(move || series.with(|(a, b)| chart(a, b, window.get())));
    let options = Signal::derive(move || {
        let mut o = vec![(String::new(), "All services".to_string())];
        s.status.with(|st| {
            if let Some(st) = st {
                o.extend(st.services.iter().map(|x| (x.name.clone(), x.name.clone())));
            }
        });
        o
    });
    let now = move |out: bool| {
        let svc = service.get();
        let r = s.rates.get();
        let rate = if svc.is_empty() {
            r.total
        } else {
            r.service(&svc)
        };
        human_bits(if out { rate.bytes_out } else { rate.bytes_in })
    };
    let label = Signal::derive(move || {
        let (i, iu) = now(false);
        let (o, ou) = now(true);
        format!(
            "Traffic over the last {} minutes; now {i} {iu} in and {o} {ou} out",
            window.get() / 60
        )
    });
    view! {
        <section class="flex flex-col rounded-card border border-border bg-surface min-[1200px]:col-span-8" data-testid="traffic">
            <div class="flex flex-wrap items-center gap-3 px-4 pt-3.5">
                <h2 class="text-[13.5px] font-bold">"Traffic"</h2>
                <span class="text-xs text-text-muted">"live · recorded while this page is open"</span>
                <span class="ml-auto"></span>
                <div class="w-36">
                    <Select id="traffic-service" small=true label="Service" options=options value=service on_change=Callback::new(move |v: String| service.set(v))/>
                </div>
                <Segmented
                    name="traffic-window"
                    label="Window"
                    options=vec![("60".to_string(), "1m".to_string(), false), ("300".to_string(), "5m".to_string(), false), ("900".to_string(), "15m".to_string(), false)]
                    value=Signal::derive(move || window.get().to_string())
                    on_change=Callback::new(move |v: String| window.set(v.parse().unwrap_or(900)))
                />
            </div>
            <div class="flex flex-wrap items-end gap-6 px-4 pt-3">
                <div data-testid="traffic-in">
                    <div class="flex items-center gap-1.5 text-xs text-text-muted"><span class="h-0.5 w-3.5 rounded bg-accent"></span>"In"</div>
                    <div class="text-2xl font-bold tabular-nums">{move || now(false).0}<small class="ml-1 text-[13px] font-medium text-text-muted">{move || now(false).1}</small></div>
                </div>
                <div data-testid="traffic-out">
                    <div class="flex items-center gap-1.5 text-xs text-text-muted"><span class="h-0.5 w-3.5 rounded bg-udp"></span>"Out"</div>
                    <div class="text-2xl font-bold tabular-nums">{move || now(true).0}<small class="ml-1 text-[13px] font-medium text-text-muted">{move || now(true).1}</small></div>
                </div>
                <div class="ml-auto text-right text-[11.5px] text-text-muted" data-testid="traffic-note">
                    <div>{move || format!("{} · Mbit/s", if service.get().is_empty() { "all services".to_string() } else { service.get() })}</div>
                </div>
            </div>
            <div class="flex h-[240px] px-4 pt-3">
                <LineChart data=data label=label testid="traffic-chart"/>
            </div>
            <p class="mt-auto flex items-center gap-1.5 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                <Icon name="info" small=true/>
                "Recorded by your browser from the live status (1 s), kept in memory for 15 min. The server keeps no history; reloading the page starts over."
            </p>
        </section>
    }
}

fn mobile_cards(section: &'static str) -> impl IntoView {
    let rows = crate::screens::entity::rows_for(section);
    view! {
        <div class="pt-2 pb-1 sm:hidden" data-testid=format!("mobile-{section}")>
            {rows.into_iter().map(|r| view! { <crate::screens::ListCard row=r selected=false/> }).collect_view()}
        </div>
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Refused(f64, u64);

#[component]
pub fn Overview() -> impl IntoView {
    let s = store();
    let refused_log: RwSignal<VecDeque<Refused>> = RwSignal::new(VecDeque::new());
    Effect::new(move |_| {
        let at = s.status_at.get();
        let n = s
            .status
            .with(|st| st.as_ref().map(|st| st.handshakes_refused));
        if let Some(n) = n {
            refused_log.update(|l| {
                l.push_back(Refused(at, n));
                while l.front().is_some_and(|r| at - r.0 > 60_000.0) {
                    l.pop_front();
                }
            });
        }
    });
    let values = Memo::new(move |_| s.running());
    let online = move |name: &str| {
        s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == name))
                .is_some_and(|c| c.connected)
        })
    };
    let clients_now = Signal::derive(move || {
        let n = names(&values.get(), ClientEntry::section());
        let up = n.iter().filter(|c| online(c)).count();
        (up, n.len())
    });
    let counts = Signal::derive(move || {
        let v = values.get();
        let (up, n) = clients_now.get();
        let vis = names(&v, VisitorEntry::section()).len();
        let active = s.status.with(|st| {
            st.as_ref()
                .map(|st| st.visitors.iter().filter(|x| s.in_use(x)).count())
                .unwrap_or(0)
        });
        vec![
            names(&v, ServiceEntry::section()).len().to_string(),
            format!("{up}/{n}"),
            format!("{active}/{vis}"),
        ]
    });
    let down = Signal::derive(move || {
        let v = values.get();
        let st = s.status.get();
        (
            names(&v, ServiceEntry::section()).len(),
            service_problems(&v, st.as_deref()),
        )
    });
    let sub = move || {
        let st = s.status.get();
        let w = s.welcome.get();
        let mut parts: Vec<String> = Vec::new();
        if let Some(w) = &w {
            parts.push(format!("v{}", w.server.version));
        }
        if let Some(st) = &st {
            parts.push(format!("up {}", format::span(st.uptime_secs)));
        }
        let rev = s.short_rev();
        if !rev.is_empty() {
            parts.push(format!("rev {rev}"));
        }
        parts.join(" · ")
    };
    let tab = RwSignal::new(
        match route::query("tab").as_deref().and_then(entity_of_section) {
            Some(Entity::Client) => ClientEntry::section(),
            Some(Entity::Visitor) => VisitorEntry::section(),
            _ => ServiceEntry::section(),
        },
    );
    let rate = move |name: &str| {
        let (v, u) = human_bits(s.rates.with(|r| r.service(name).bytes_in));
        format!("{v} {u}")
    };
    let services_table = move || {
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
    };
    let clients_table = move || {
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
    };
    let visitors_table = move || {
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
    };
    let kpi_link = "rounded-card no-underline text-text hover:[&>div]:border-border-strong";
    view! {
        <div class="px-8 pt-7 pb-12 max-sm:px-4" data-testid="overview">
            <header class="mb-[22px] flex flex-wrap items-start gap-3">
                <div class="min-w-0 flex-1">
                    <h1 class="text-2xl font-bold tracking-[-0.015em]">"Overview"</h1>
                    <p class="mt-1 text-sm text-text-muted" data-testid="overview-sub">{sub}</p>
                </div>
                <button type="button" class=button_class("secondary", false) data-testid="flow-open" on:click=move |_| s.flow_open.set(true)>
                    <Icon name="network"/>
                    "Flow chart"
                    <Kbd text="F"/>
                </button>
            </header>
            <div class="grid grid-cols-2 gap-4 md:grid-cols-6 xl:grid-cols-12">
                <Link to=Route::List(ClientEntry::section().into()) class=format!("md:col-span-2 {kpi_link}") testid="kpi-clients-link">
                    <Kpi
                        label="Clients online"
                        icon="monitor"
                        value=Signal::derive(move || clients_now.get().0.to_string())
                        unit=Signal::derive(move || format!("/ {}", clients_now.get().1))
                        detail=Signal::derive(move || { let (u, n) = clients_now.get(); if u == n { "all connected".to_string() } else { format!("{} away", n - u) } })
                        testid="kpi-clients"
                    />
                </Link>
                <Link to=Route::List(ServiceEntry::section().into()) class=format!("md:col-span-2 {kpi_link}") testid="kpi-services-link">
                    <Kpi
                        label="Services healthy"
                        icon="stack"
                        value=Signal::derive(move || { let (n, bad) = down.get(); (n - bad.len().min(n)).to_string() })
                        unit=Signal::derive(move || format!("/ {}", down.get().0))
                        detail=Signal::derive(move || healthy_detail(&down.get().1))
                        testid="kpi-services"
                    />
                </Link>
                <div class="md:col-span-2">
                    <Kpi
                        label="Open connections"
                        icon="network"
                        value=Signal::derive(move || s.status.with(|st| format::count(st.as_ref().map(|st| st.services.iter().map(|x| x.open).sum()).unwrap_or(0))))
                        detail=Signal::derive(move || s.status.with(|st| format!("{} since start", format::count(st.as_ref().map(|st| st.services.iter().map(|x| x.total).sum()).unwrap_or(0)))))
                        testid="kpi-open"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Traffic now"
                        icon="activity"
                        value=Signal::derive(move || human_bits(s.rates.with(|r| r.total.bytes_in)).0)
                        unit=Signal::derive(move || human_bits(s.rates.with(|r| r.total.bytes_in)).1.to_string())
                        detail=Signal::derive(move || { let (o, u) = human_bits(s.rates.with(|r| r.total.bytes_out)); format!("in · {o} {u} out") })
                        testid="kpi-traffic"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Visitors in use"
                        icon="person"
                        value=Signal::derive(move || {
                            let active = s.status.with(|st| st.as_ref().map(|st| st.visitors.iter().filter(|v| s.in_use(v)).count()).unwrap_or(0));
                            active.to_string()
                        })
                        unit=Signal::derive(move || format!("/ {}", names(&values.get(), VisitorEntry::section()).len()))
                        detail=Signal::derive(move || {
                            let seen = s.status.with(|st| st.as_ref().map(|st| st.visitors.iter().filter(|v| s.in_use(v) || v.last_seen_secs.is_some()).count()).unwrap_or(0));
                            format!("{seen} used since start")
                        })
                        testid="kpi-visitors"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Refused handshakes"
                        icon="shield"
                        value=Signal::derive(move || refused_log.with(|l| match (l.front(), l.back()) {
                            (Some(a), Some(b)) => b.1.saturating_sub(a.1).to_string(),
                            _ => "0".into(),
                        }))
                        unit="/min"
                        detail=Signal::derive(move || s.status.with(|st| format!("{} since start", format::count(st.as_ref().map(|st| st.handshakes_refused).unwrap_or(0)))))
                        testid="kpi-refused"
                    />
                </div>
                <div class="col-span-2 grid gap-4 md:col-span-6 xl:col-span-12 min-[1200px]:grid-cols-12" data-testid="overview-row3">
                    <Traffic/>
                    <Attention/>
                </div>
                <section class="col-span-2 rounded-card border border-border bg-surface md:col-span-6 xl:col-span-12" data-testid="overview-tabs">
                    <div class="flex flex-wrap items-center gap-3 border-b border-border px-3 py-2.5">
                        <PillTabs
                            tabs=vec![(ServiceEntry::section(), "Services".into()), (ClientEntry::section(), "Clients".into()), (VisitorEntry::section(), "Visitors".into())]
                            active=tab
                            counts=counts
                            on_change=Callback::new(move |t: &'static str| {
                                tab.set(t);
                                route::set_query("tab", t);
                            })
                        />
                        <span class="ml-auto text-xs text-text-muted">
                            "current rates · "
                            {move || {
                                let t = tab.get();
                                view! { <Link to=Route::List(t.to_string()) class="font-semibold text-accent-ink no-underline hover:underline">{format!("Open {t} →")}</Link> }
                            }}
                        </span>
                    </div>
                    <div role="tabpanel">
                        {move || match entity_of_section(tab.get()) {
                            Some(Entity::Client) => clients_table().into_any(),
                            Some(Entity::Visitor) => visitors_table().into_any(),
                            _ => services_table().into_any(),
                        }}
                    </div>
                </section>
            </div>
        </div>
    }
}
