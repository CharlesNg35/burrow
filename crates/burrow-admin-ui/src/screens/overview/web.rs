use burrow_admin_logic::top::top;
use burrow_admin_proto::web::{RouteLine, WebSummary};
use burrow_config::{Entry, ServiceEntry};
use burrow_ui_kit::{Tone, tag_class};
use leptos::prelude::*;

use crate::route::{self, Link, Route};

use super::tables::view_all_link;

fn service_link(service: String, rest: String) -> impl IntoView {
    let to = Route::Detail(ServiceEntry::section().into(), service.clone());
    view! {
        <Link to=to class="relative font-semibold text-accent-ink no-underline after:absolute after:-inset-x-2 after:-inset-y-1 hover:underline">{service}</Link>
        {(!rest.is_empty()).then(|| view! { <span class="text-xs text-text-muted">" "{rest}</span> })}
    }
}

fn routes(list: Vec<RouteLine>) -> impl IntoView {
    let items = list
        .into_iter()
        .map(|r| {
            let to = if r.service.is_empty() {
                view! { <span class="text-[13px]">{r.to}</span> }.into_any()
            } else {
                let rest = r
                    .to
                    .strip_prefix(&format!("service {}", r.service))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                view! { <span class="text-[13px]">{service_link(r.service, rest)}</span> }.into_any()
            };
            let (key, path) = (r.path.clone(), r.path);
            view! {
                <li class="flex flex-wrap items-baseline gap-x-3" data-web-route=key>
                    <span class="w-16 shrink-0 font-mono text-[13px] text-text-muted sm:w-32">{path}</span>
                    <span>"→ "{to}</span>
                </li>
            }
        })
        .collect_view();
    view! { <ul class="mt-1 w-full space-y-0.5 pl-4" data-testid="web-routes">{items}</ul> }
}

fn plain_kind(served: &str) -> String {
    match served.split_once(" (tls: ") {
        Some((kind, rest)) => format!("{kind}{}", rest.split_once(')').map_or("", |(_, r)| r)),
        None => served.to_string(),
    }
}

fn ports_line(w: &WebSummary) -> String {
    let mut out = Vec::new();
    let shared = |on: bool| if on { " (shared with the tunnel)" } else { "" };
    if !w.https.is_empty() {
        out.push(format!(
            "HTTPS on {}{}",
            w.https,
            shared(w.shared_with_tunnel)
        ));
    }
    if !w.http.is_empty() {
        out.push(format!(
            "HTTP on {}{}",
            w.http,
            shared(w.http_shared_with_tunnel)
        ));
    }
    out.join(", ")
}

fn rows(web: WebSummary) -> impl IntoView {
    let tunnel = web.tunnel.clone().map(|t| {
        view! {
            <li class="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-4 py-2.5" data-web-row="tunnel">
                <span class="font-mono text-[13px]">{t.url}</span>
                <span class="text-xs text-text-muted">"the tunnel, for burrow clients"</span>
            </li>
        }
    });
    let empty = (web.hosts.is_empty() && web.tunnel.is_none()).then(|| {
        view! { <li class="px-4 py-2.5 text-[13px] text-text-muted">"No names yet: every connection to this port is closed."</li> }
    });
    let (kept, total) = top(web.hosts, |h| (h.unencrypted(), 0));
    let shown = kept.len();
    let hosts = kept
        .into_iter()
        .map(|h| {
            let (row, kind, plain) = (h.name.clone(), plain_kind(&h.served()), h.unencrypted());
            let single = h.routes.is_empty().then(|| service_link(h.service.clone(), h.also.clone()));
            let routes = (!h.routes.is_empty()).then(|| routes(h.routes.clone()));
            view! {
                <li class="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-4 py-2.5" data-web-row=row>
                    <span class="font-mono text-[13px]">{h.name}</span>
                    {plain.then(|| view! { <span class=tag_class(Tone::Warning) data-testid="unencrypted">"unencrypted"</span> })}
                    <span class="text-xs text-text-muted">{kind}</span>
                    {single.map(|l| view! { <span class="ml-auto text-[13px]">"→ "{l}</span> })}
                    {routes}
                </li>
            }
        })
        .collect_view();
    view! {
        <ul class="divide-y divide-border">{tunnel}{hosts}{empty}</ul>
        {view_all_link(Route::List(ServiceEntry::section().into()), Some((route::FILTER_QUERY, route::WEBSITES_FILTER)), shown, total)}
    }
}

pub(super) fn websites_panel(web: Option<WebSummary>) -> impl IntoView {
    let Some(w) = web else {
        return view! { <p class="px-4 py-3 text-[13px] text-text-muted" data-testid="websites">"The web proxy is not running yet."</p> }.into_any();
    };
    view! {
        <div data-testid="websites">
            <p class="border-b border-border px-4 py-2.5 text-xs text-text-muted" data-testid="websites-port">{ports_line(&w)}</p>
            {rows(w)}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_websites_tab_reads_without_config_keys() {
        assert_eq!(
            plain_kind("local CA, 12-hour certificates (tls: internal)"),
            "local CA, 12-hour certificates"
        );
        assert_eq!(
            plain_kind("shared file ./portal.pem"),
            "shared file ./portal.pem"
        );
        let w = WebSummary {
            https: "127.0.0.1:443".into(),
            http: "127.0.0.1:80".into(),
            addresses: "this address only".into(),
            ..WebSummary::default()
        };
        assert_eq!(
            ports_line(&w),
            "HTTPS on 127.0.0.1:443, HTTP on 127.0.0.1:80"
        );
    }
}
