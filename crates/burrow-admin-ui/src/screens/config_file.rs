use burrow_admin_logic::running::{difference_note, differences};
use burrow_admin_proto::msg;
use burrow_admin_proto::serde_json;
use burrow_admin_proto::view::{ConfigView, Shown, ShownRequest};
use burrow_ui_kit::{Callout, CodeBlock, Toggle, Tone};
use leptos::prelude::*;

use crate::api;
use crate::route::{Link, Route};
use crate::store::store;

type Loaded = Option<Result<ConfigView, String>>;

fn fetch(show: Shown, into: RwSignal<Loaded>, latest: StoredValue<u64>) {
    latest.update_value(|n| *n += 1);
    let asked = latest.get_value();
    api::request(
        msg::req::CONFIG_VIEW,
        api::body(&ShownRequest { show }),
        move |r| {
            if latest.try_get_value() != Some(asked) {
                return;
            }
            let got = r
                .map_err(|e| e.message)
                .and_then(|v| serde_json::from_value::<ConfigView>(v).map_err(|e| e.to_string()));
            let _ = into.try_set(Some(got));
        },
    );
}

fn running_note() -> impl IntoView {
    let s = store();
    let note = Memo::new(move |_| {
        s.state.with(|st| {
            st.as_ref()
                .and_then(|st| difference_note(s.status.with(|ss| differences(st, ss.as_deref()))))
        })
    });
    move || {
        note.get().map(|n| {
            view! {
                <div class="mb-3">
                    <Callout tone=Tone::Warning title=n testid="file-differs">
                        <Link to=Route::Overview class="font-semibold text-accent-ink no-underline hover:underline" testid="file-differs-link">"See Needs attention"</Link>
                    </Callout>
                </div>
            }
        })
    }
}

fn file_body(loaded: RwSignal<Loaded>) -> impl IntoView {
    let text = Signal::derive(move || {
        loaded.with(|l| match l {
            Some(Ok(v)) => v.text(),
            _ => String::new(),
        })
    });
    let shape = Memo::new(move |_| {
        loaded.with(|l| match l {
            None => None,
            Some(Ok(v)) => Some(Ok(v.download_name())),
            Some(Err(e)) => Some(Err(e.clone())),
        })
    });
    move || match shape.get() {
        None => view! { <p class="text-sm text-text-muted">"Loading…"</p> }.into_any(),
        Some(Err(e)) => {
            view! { <p class="text-sm text-danger" data-testid="file-error">{e}</p> }.into_any()
        }
        Some(Ok(name)) => {
            view! { <CodeBlock text=text file_name=name testid="file-text"/> }.into_any()
        }
    }
}

#[component]
pub fn ConfigFile() -> impl IntoView {
    let s = store();
    let offered = Memo::new(move |_| {
        s.welcome
            .with(|w| w.as_ref().is_some_and(|w| w.offers(msg::req::CONFIG_VIEW)))
    });
    let show = RwSignal::new(Shown::Written);
    let loaded: RwSignal<Loaded> = RwSignal::new(None);
    let rev = Memo::new(move |_| s.state.with(|st| st.as_ref().map(|st| st.rev.clone())));
    let latest = StoredValue::new(0u64);
    Effect::new(move |_| {
        rev.track();
        let now = show.get();
        if offered.get() {
            fetch(now, loaded, latest);
        }
    });
    let toggle = move || {
        offered.get().then(|| {
            view! {
            <Toggle
                id="file-defaults"
                checked=Signal::derive(move || show.get() == Shown::WithDefaults)
                on_change=Callback::new(move |on: bool| {
                    show.set(if on { Shown::WithDefaults } else { Shown::Written })
                })
                label=Signal::stored("Show defaults too".to_string())
            />
                }
        })
    };
    let body = move || {
        if offered.get() {
            file_body(loaded).into_any()
        } else {
            view! { <p class="text-sm text-text-muted" data-testid="file-older">"This server runs an older burrow that cannot show its config file here. Update burrow on the server."</p> }.into_any()
        }
    };
    view! {
        <section class="mb-4 rounded-card border border-border bg-surface" data-card="file" data-testid="config-file" data-rev=move || loaded.with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).map(|v| v.rev.clone()).unwrap_or_default()) data-state-rev=move || rev.get().unwrap_or_default()>
            {burrow_ui_kit::card_head(
                "The config the server reads".into(),
                Some("View only. Change it in the other tabs, or with burrow edit.".into()),
                view! { <span data-testid="file-toggle">{toggle}</span> }.into_any(),
            )}
            <div class="min-w-0 px-4 py-3">
                {running_note}
                {body}
            </div>
        </section>
    }
}
