use burrow_admin_logic::logins::{chosen, config, logins, note};
use burrow_admin_proto::serde_json::Value;
use burrow_config::ServerConfig;
use leptos::prelude::*;

use crate::domain::{FieldCtx, choice};
use crate::route::{CARD_QUERY, Link, Route};
use crate::store::store;

const NONE: &str = "None";

pub(crate) fn site_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let site = path[..path.len() - 1].to_vec();
    let names = Memo::new(move |_| logins(&e.doc()).keys().cloned().collect::<Vec<_>>());
    let value = {
        let site = site.clone();
        Signal::derive(move || {
            let all = logins(&e.doc());
            e.value(&site)
                .and_then(|w| chosen(&w, &all))
                .unwrap_or_default()
        })
    };
    let p2 = path.clone();
    let set = Callback::new(move |v: String| {
        e.set(&p2, (!v.is_empty()).then_some(Value::String(v)));
    });
    let control = move || {
        let mut options = vec![(String::new(), NONE.to_string(), false)];
        options.extend(names.get().into_iter().map(|n| (n.clone(), n, false)));
        choice(
            fc.id.clone(),
            fc.node.label.clone(),
            options,
            value,
            set,
            Signal::stored(Vec::new()),
        )
    };
    let line = move || {
        let n = value.get();
        let c = logins(&e.doc()).get(&n).and_then(config)?;
        Some(
            view! { <p class="mt-1.5 text-xs text-text-muted" data-testid="login-note">{note(&n, &c)}</p> },
        )
    };
    view! {
        <div data-testid="site-login">
            <div class="flex flex-wrap items-center gap-3">
                {control}
                <Link to=Route::Settings query=Some((CARD_QUERY, ServerConfig::WEB.key())) class="text-sm font-semibold text-accent-ink no-underline hover:underline" testid="manage-logins">"Manage login services →"</Link>
            </div>
            {line}
        </div>
    }
    .into_any()
}
