use burrow_admin_logic::rates::human_bytes;
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::{Card, EmptyState, Kpi, PageHeader, Sparkline, StatusDot, Table, Tone};
use leptos::prelude::*;

use crate::format;
use crate::route::{Link, Route};
use crate::store::store;

fn keys(v: &Value, section: &str) -> Vec<String> {
    v.get(section)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn service_clients(svc: &Value) -> Vec<String> {
    match (svc.get("client"), svc.get("clients")) {
        (Some(Value::String(c)), _) => vec![c.clone()],
        (_, Some(Value::Array(a))) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        (_, Some(Value::String(c))) => vec![c.clone()],
        _ => Vec::new(),
    }
}

#[component]
pub fn Overview() -> impl IntoView {
    let s = store();
    let values = move || {
        s.state.with(|st| {
            st.as_ref()
                .map(|st| st.values.clone())
                .unwrap_or(Value::Null)
        })
    };
    let online = move |name: &str| {
        s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == name))
                .is_some_and(|c| c.connected)
        })
    };
    let clients_kpi = Signal::derive(move || {
        let names = keys(&values(), "clients");
        let up = names.iter().filter(|n| online(n)).count();
        format!("{up} / {}", names.len())
    });
    let clients_tone = Signal::derive(move || {
        let names = keys(&values(), "clients");
        if names.iter().all(|n| online(n)) {
            Tone::Success
        } else {
            Tone::Warning
        }
    });
    let services_kpi = Signal::derive(move || keys(&values(), "services").len().to_string());
    let public = Signal::derive(move || {
        let v = values();
        let n = v
            .get("services")
            .and_then(Value::as_object)
            .map(|m| m.values().filter(|x| x.get("bind").is_some()).count())
            .unwrap_or(0);
        format!("{n} public")
    });
    let open = Signal::derive(move || {
        s.status.with(|st| {
            format::count(
                st.as_ref()
                    .map(|st| st.services.iter().map(|x| x.open).sum())
                    .unwrap_or(0),
            )
        })
    });
    let total = Signal::derive(move || {
        s.status.with(|st| {
            let t: u64 = st
                .as_ref()
                .map(|st| st.services.iter().map(|x| x.total).sum())
                .unwrap_or(0);
            format!("{} since start", format::count(t))
        })
    });
    let traffic = Signal::derive(move || {
        let r = s.rates.get();
        format!(
            "↓ {}  ↑ {}",
            human_bytes(r.total.bytes_in),
            human_bytes(r.total.bytes_out)
        )
    });
    let refused = Signal::derive(move || {
        s.status
            .with(|st| format::count(st.as_ref().map(|st| st.handshakes_refused).unwrap_or(0)))
    });
    let spark_in = Signal::derive(move || s.traffic_in.with(|h| h.points().collect::<Vec<_>>()));
    let spark_out = Signal::derive(move || s.traffic_out.with(|h| h.points().collect::<Vec<_>>()));
    let attention = move || {
        let v = values();
        let mut items: Vec<(Tone, String)> = Vec::new();
        if let Some(st) = s.state.get() {
            if !st.running.ok {
                items.push((
                    Tone::Danger,
                    "The config file has errors; the previous version is running".into(),
                ));
            }
            for p in &st.runtime.restart_pending {
                items.push((Tone::Warning, format!("{p} changes after a restart")));
            }
            for d in st.diags.iter().filter(|d| d.severity == Severity::Warning) {
                items.push((Tone::Warning, d.message.clone()));
            }
        }
        if s.status.with(Option::is_some) {
            let services = v
                .get("services")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut offline_used: Vec<String> = Vec::new();
            for (name, svc) in &services {
                let cs = service_clients(svc);
                if !cs.is_empty() && cs.iter().all(|c| !online(c)) {
                    items.push((Tone::Danger, format!("Service {name} has no client online")));
                }
                for c in cs {
                    if !online(&c) && !offline_used.contains(&c) {
                        offline_used.push(c);
                    }
                }
            }
            for c in offline_used {
                items.push((
                    Tone::Warning,
                    format!("Client {c} is offline and serves services"),
                ));
            }
        }
        if items.is_empty() {
            view! { <p class="text-text-muted" data-testid="attention-empty">"Nothing needs attention."</p> }.into_any()
        } else {
            view! {
                <ul class="space-y-2" data-testid="attention">
                    {items
                        .into_iter()
                        .map(|(tone, text)| view! { <li class=format!("rounded-input border border-l-4 px-3 py-2 text-sm {} {}", tone.soft(), tone.bar()) data-tone=tone.name()>{text}</li> })
                        .collect_view()}
                </ul>
            }
            .into_any()
        }
    };
    let services_table = move || {
        let v = values();
        let services = v
            .get("services")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if services.is_empty() {
            return view! { <EmptyState title="No services yet" message="Services publish a port of a client on this server." testid="services-empty"/> }.into_any();
        }
        let rates = s.rates.get();
        let status = s.status.get();
        let rows = services
            .iter()
            .map(|(name, svc)| {
                let st = status.as_ref().and_then(|st| st.services.iter().find(|x| &x.name == name)).cloned();
                let r = rates.service(name);
                let label = name.clone();
                let public = svc.get("bind").map(crate::form::plain).unwrap_or_else(|| "Private".into());
                let clients = service_clients(svc)
                    .into_iter()
                    .map(|c| {
                        let on = online(&c);
                        view! { <StatusDot on=on text=c/> }
                    })
                    .collect_view();
                view! {
                    <tr data-row=name.clone()>
                        <th scope="row"><Link to=Route::Detail("services".into(), name.clone()) class="font-mono text-sm">{label}</Link></th>
                        <td class="font-mono text-sm">{public}</td>
                        <td><span class="flex flex-wrap gap-x-3">{clients}</span></td>
                        <td class="tabular-nums" data-cell="open">{st.as_ref().map(|x| format::count(x.open)).unwrap_or_default()}</td>
                        <td class="tabular-nums" data-cell="rate">{format!("↓ {} ↑ {}", human_bytes(r.bytes_in), human_bytes(r.bytes_out))}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <Table head=vec!["Service", "Public", "Clients", "Open", "Traffic"] testid="overview-services">{rows}</Table>
        }
        .into_any()
    };
    let clients_table = move || {
        let names = keys(&values(), "clients");
        if names.is_empty() {
            return view! { <EmptyState title="No clients yet" message="A client is a machine that connects out to this server." testid="clients-empty"/> }.into_any();
        }
        let status = s.status.get();
        let rows = names
            .into_iter()
            .map(|name| {
                let st = status.as_ref().and_then(|st| st.clients.iter().find(|c| c.name == name)).cloned();
                let on = st.as_ref().is_some_and(|c| c.connected);
                let label = name.clone();
                let from = st.as_ref().and_then(|c| c.from.clone()).unwrap_or_default();
                let since = st
                    .as_ref()
                    .and_then(|c| match (c.connected_secs, c.left_secs_ago) {
                        (Some(s), _) => Some(format!("for {}", format::span(s))),
                        (None, Some(a)) => Some(format!("left {}", format::ago(a))),
                        _ => None,
                    })
                    .unwrap_or_else(|| "never connected".into());
                view! {
                    <tr data-row=name.clone()>
                        <th scope="row"><Link to=Route::Detail("clients".into(), name.clone()) class="font-mono text-sm">{label}</Link></th>
                        <td><StatusDot on=on text=if on { "online" } else { "offline" }/></td>
                        <td class="font-mono text-sm">{from}</td>
                        <td>{since}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <Table head=vec!["Client", "State", "From", "Since"] testid="overview-clients">{rows}</Table>
        }
        .into_any()
    };
    view! {
        <PageHeader title="Overview" subtitle="Live state of the server, updated every second"/>
        <div class="mb-4 grid grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] gap-3">
            <Kpi label="Clients online" value=clients_kpi tone=clients_tone testid="kpi-clients"/>
            <Kpi label="Services" value=services_kpi detail=public testid="kpi-services"/>
            <Kpi label="Open connections" value=open detail=total testid="kpi-open"/>
            <Kpi label="Traffic" value=traffic detail="per second, all services" testid="kpi-traffic"/>
            <Kpi label="Refused handshakes" value=refused detail="since start" testid="kpi-refused"/>
        </div>
        <div class="grid items-start gap-4 lg:grid-cols-2">
            <Card title="Throughput" testid="card-throughput">
                <Sparkline points=spark_in second=spark_out label="Traffic in and out over the last 10 minutes" testid="spark"/>
                <p class="mt-1 flex gap-4 text-xs"><span class="text-accent">"in"</span><span class="text-success">"out"</span></p>
            </Card>
            <Card title="Needs attention" testid="card-attention">{attention}</Card>
        </div>
        <Card title="Services">{services_table}</Card>
        <Card title="Clients">{clients_table}</Card>
    }
}
