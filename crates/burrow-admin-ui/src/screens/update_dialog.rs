use burrow_ui_kit::{Button, ButtonLook, Dialog, Fact, Facts, Icon};
use leptos::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Confirm {
    pub title: String,
    pub from: Option<String>,
    pub to: String,
    pub names: Option<(String, String, String)>,
    pub notes: Vec<(&'static str, String)>,
    pub action: String,
}

pub fn release_link(version: &str, text: fn(&str) -> String) -> Option<AnyView> {
    burrow_admin_logic::wording::release_notes(version).map(|(v, url)| view! {
        <a href=url target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 font-semibold text-accent-ink" data-testid="whats-new">{text(&v)}<Icon name="external" small=true/></a>
    }.into_any())
}

pub fn short_link(_: &str) -> String {
    "What's new".into()
}

pub fn long_link(v: &str) -> String {
    format!("What's new in {v}")
}

pub fn update_dialog(
    open: RwSignal<bool>,
    testid: &'static str,
    confirm_testid: &'static str,
    c: Signal<Confirm>,
    go: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let body = move || {
        let c = c.get();
        let versions = c.from.clone().map(|from| {
            let to = c.to.clone();
            view! {
                <Facts testid="update-versions">
                    <Fact label="Current" mono=true>{from}</Fact>
                    <Fact label="New" mono=true>
                        <span class="inline-flex flex-wrap items-center gap-1.5">{to.clone()}{release_link(&to, short_link).map(|l| view! { <span class="text-text-faint">"·"</span><span class="font-sans text-sm">{l}</span> })}</span>
                    </Fact>
                </Facts>
            }
        });
        let names = c.names.clone().map(|(shown, skipped, all)| view! {
            <p class="text-sm" data-testid="update-names">
                {shown}
                {(!skipped.is_empty()).then(|| view! { " " <span class="text-text-muted" title=all>{skipped}</span> })}
            </p>
        });
        let notes = c
            .notes
            .iter()
            .map(|(icon, text)| view! { <li class="flex items-start gap-2"><span class="mt-0.5 text-text-muted"><Icon name=*icon small=true/></span><span>{text.clone()}</span></li> })
            .collect_view();
        let link = c
            .from
            .is_none()
            .then(|| release_link(&c.to, long_link))
            .flatten();
        view! {
            {versions}
            {names}
            <ul class="mt-3 space-y-1.5 text-sm" data-testid="update-notes">{notes}</ul>
            <div class="mt-4 flex items-center justify-end gap-2">
                {link.map(|l| view! { <span class="mr-auto text-xs">{l}</span> })}
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button label=c.action.clone() variant=ButtonLook::Primary testid=confirm_testid on_click=move |_| {
                    open.set(false);
                    go();
                }/>
            </div>
        }
    };
    view! {
        <Dialog open=open title=Signal::derive(move || c.with(|c| c.title.clone())) icon="download" testid=testid>
            {body}
        </Dialog>
    }
}
