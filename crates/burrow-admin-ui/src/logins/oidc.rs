use burrow_admin_logic::oidc::{chosen, config, note, providers, secret_shown, site_value};
use burrow_admin_proto::value::child;
use burrow_config::ServerConfig;
use burrow_ui_kit::{ComboOption, Combobox};
use leptos::prelude::*;

use super::Kind;
use crate::domain::{FieldCtx, set_checked};
use crate::route::{CARD_QUERY, Link, Route};
use crate::store::store;

pub(super) fn card_body(name: &str) -> AnyView {
    let Some(c) = store()
        .edit
        .value(&child(&Kind::Oidc.base(), name))
        .and_then(|v| config(&v))
    else {
        return ().into_any();
    };
    view! {
        <p class="mt-1 text-[13px] text-text-muted" data-testid="oidc-summary"><span class="font-mono">{c.issuer.trim().to_string()}</span></p>
        <p class="mt-0.5 text-[13px]" data-testid="oidc-client">"Client ID "<span class="font-mono">{c.client_id.trim().to_string()}</span></p>
        <p class="mt-0.5 text-[13px]" data-testid="oidc-secret">"Client secret: "<span class="font-mono">{secret_shown(&c)}</span></p>
    }
    .into_any()
}

fn options() -> Vec<ComboOption> {
    providers(&store().edit.doc())
        .iter()
        .map(|(name, v)| ComboOption {
            value: name.clone(),
            label: name.clone(),
            facet: config(v)
                .map(|c| c.issuer.trim().to_string())
                .unwrap_or_default(),
            dot: None,
        })
        .collect()
}

pub(crate) fn oidc_site_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let site = fc.path[..fc.path.len() - 1].to_vec();
    let selected = Signal::derive(move || {
        let all = providers(&e.doc());
        e.value(&site).map(|w| chosen(&w, &all)).unwrap_or_default()
    });
    let (fc2, node) = (fc.clone(), fc.node.clone());
    let on_change = Callback::new(move |list: Vec<String>| {
        set_checked(&fc2, &node, site_value(&list));
    });
    let line = move || {
        let names = selected.get();
        (!names.is_empty()).then(|| {
            let text = note(&names, &providers(&e.doc()));
            view! { <p class="mt-1.5 text-xs text-text-muted" data-testid="oidc-note">{text}</p> }
        })
    };
    let empty = move || {
        providers(&e.doc()).is_empty().then(|| {
            view! { <p class="mt-1 text-sm text-text-muted" data-testid="no-oidc-providers">"Add a login provider under Settings → Web first"</p> }
        })
    };
    view! {
        <div data-testid="site-oidc">
            <div class="flex flex-wrap items-center gap-3">
                <div class="min-w-[14rem]" data-picker="multi">
                    <Combobox
                        id=fc.id.clone()
                        options=Signal::derive(options)
                        selected=selected
                        on_change=on_change
                        multi=true
                        clearable=true
                        header=Kind::Oidc.entity().plural()
                        placeholder="Add…"
                        label=fc.node.label.clone()
                        described_by=fc.msg_id.clone()
                    />
                </div>
                <Link to=Route::Settings query=Some((CARD_QUERY, ServerConfig::WEB.key())) class="text-sm font-semibold text-accent-ink no-underline hover:underline" testid="manage-oidc">"Manage login providers →"</Link>
            </div>
            {empty}
            {line}
        </div>
    }
    .into_any()
}
