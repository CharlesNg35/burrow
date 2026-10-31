use burrow_admin_proto::schema::{ANY, Entity};
use burrow_admin_proto::serde_json::{Value, json};
use burrow_admin_proto::value::child;
use burrow_config::OidcConfig;
use burrow_syntax::env::is_reference;
use burrow_syntax::web::oidc::{CALLBACK_PATH, issuer_problem};
use burrow_ui_kit::{Button, Dialog, TextInput, Tone, field_message};
use leptos::prelude::*;

use crate::screens::create::{name_input, problem_of};
use crate::store::store;

use super::{Kind, Open};

#[derive(Clone, Copy)]
struct Draft {
    name: RwSignal<String>,
    issuer: RwSignal<String>,
    client_id: RwSignal<String>,
    secret: RwSignal<String>,
    hidden: RwSignal<Option<Value>>,
    old: RwSignal<Option<Value>>,
    locked: bool,
}

fn secret_locked() -> bool {
    let mut at = Kind::Oidc.base();
    at.push(ANY.to_string());
    at.push(OidcConfig::CLIENT_SECRET.key().to_string());
    store()
        .schema
        .with_untracked(|s| s.as_ref().is_some_and(|s| s.locked_at(&at).is_some()))
}

impl Draft {
    fn new() -> Self {
        Draft {
            name: RwSignal::new(String::new()),
            issuer: RwSignal::new(String::new()),
            client_id: RwSignal::new(String::new()),
            secret: RwSignal::new(String::new()),
            hidden: RwSignal::new(None),
            old: RwSignal::new(None),
            locked: secret_locked(),
        }
    }

    fn fill(&self, open: &Open) {
        let old = match open {
            Open::Edit(n) => store().edit.value(&child(&Kind::Oidc.base(), n)),
            _ => None,
        };
        let text = |key: &str| {
            old.as_ref()
                .and_then(|v| v.get(key))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let secret = old
            .as_ref()
            .and_then(|v| v.get(OidcConfig::CLIENT_SECRET.key()))
            .cloned();
        let shown = secret
            .as_ref()
            .and_then(Value::as_str)
            .filter(|s| is_reference(s))
            .map(str::to_string);
        self.name.set(match open {
            Open::Edit(n) => n.clone(),
            _ => String::new(),
        });
        self.issuer.set(text(OidcConfig::ISSUER.key()));
        self.client_id.set(text(OidcConfig::CLIENT_ID.key()));
        self.old.set(secret.clone());
        self.hidden.set(secret.filter(|_| shown.is_none()));
        self.secret.set(shown.unwrap_or_default());
    }

    fn value(&self) -> Value {
        let mut v = json!({
            OidcConfig::ISSUER.key(): self.issuer.get_untracked().trim(),
            OidcConfig::CLIENT_ID.key(): self.client_id.get_untracked().trim(),
        });
        let typed = self.secret.get_untracked().trim().to_string();
        let secret = match (self.locked, typed.is_empty(), self.hidden.get_untracked()) {
            (true, _, _) => self.old.get_untracked(),
            (false, false, _) => Some(Value::String(typed)),
            (false, true, kept) => kept,
        };
        if let Some(s) = secret {
            v[OidcConfig::CLIENT_SECRET.key()] = s;
        }
        v
    }

    fn problem(&self, name: Option<String>) -> Option<String> {
        let issuer = self.issuer.get();
        if let Some(p) = name {
            return Some(p);
        }
        if let Some(p) = issuer_problem(&issuer) {
            return Some(format!("the issuer {p}, like https://accounts.google.com"));
        }
        if self.client_id.with(|c| c.trim().is_empty()) {
            return Some("type the client ID the provider gave you".to_string());
        }
        let s = self.secret.get();
        (!self.locked && !s.trim().is_empty() && !is_reference(&s)).then(|| {
            "write the secret as a variable like ${GOOGLE_SECRET} or a file like file:/etc/burrow/google.secret, never the secret itself".to_string()
        })
    }
}

fn labelled(id: &'static str, label: &'static str, body: AnyView, help: String) -> impl IntoView {
    view! {
        <div class="md:col-span-2">
            <label class="mb-1.5 block text-sm font-semibold" for=id>{label}</label>
            {body}
            <p class="mt-1.5 text-xs text-text-muted">{help}</p>
        </div>
    }
}

fn input(id: &'static str, value: RwSignal<String>, placeholder: &'static str) -> AnyView {
    let on = Callback::new(move |t: String| value.set(t));
    view! { <TextInput id=id value=value on_input=on mono=true placeholder=placeholder/> }
        .into_any()
}

fn locked_secret(old: Option<Value>, name: &str) -> AnyView {
    let now = match old.as_ref().and_then(Value::as_str) {
        None => burrow_admin_logic::oidc::PUBLIC_CLIENT.to_string(),
        Some(s) if is_reference(s) => format!("from {}", s.trim()),
        Some(_) => burrow_admin_logic::oidc::HIDDEN_SECRET.to_string(),
    };
    let name = if name.is_empty() { "<name>" } else { name };
    let how = format!(
        "Locked here: write client_secret: ${{{}_SECRET}} under web.oidc.{name} in server.yaml on the server, set that variable for burrow, then restart it.",
        name.to_ascii_uppercase().replace('-', "_")
    );
    view! {
        <div class="md:col-span-2" data-testid="oidc-secret-locked">
            <p class="mb-1.5 text-sm font-semibold">"Client secret"</p>
            <p class="font-mono text-sm">{now}</p>
            <p class="mt-1.5 text-xs text-text-muted">{how}</p>
        </div>
    }
    .into_any()
}

fn secret_help(hidden: Option<Value>) -> String {
    match hidden {
        Some(_) => "The secret is written in server.yaml and stays hidden; type ${VAR} or file:path to replace it.".to_string(),
        None => "From an environment variable like ${GOOGLE_SECRET} or a file like file:/etc/burrow/google.secret; leave it empty for a public client.".to_string(),
    }
}

pub(super) fn oidc_dialog(open: RwSignal<Open>) -> impl IntoView {
    let e = store().edit;
    let d = Draft::new();
    Effect::new(move |_| {
        let now = open.get();
        untrack(|| d.fill(&now));
    });
    let shown = crate::app::shown_while(open, |o| *o != Open::Closed, |o| *o = Open::Closed);
    let adding = Signal::derive(move || open.get() == Open::Add);
    let name_problem = Signal::derive(move || {
        adding
            .get()
            .then(|| problem_of(&Entity::Oidc, d.name.get().trim()))
            .flatten()
    });
    let blocked = Signal::derive(move || d.problem(name_problem.get()));
    let title = Signal::derive(move || match open.get() {
        Open::Edit(n) => format!("Edit login provider {n}"),
        _ => "Add a login provider".to_string(),
    });
    let save = move || {
        let name = match open.get_untracked() {
            Open::Edit(n) => n,
            _ => d.name.get_untracked().trim().to_string(),
        };
        e.set(&child(&Kind::Oidc.base(), &name), Some(d.value()));
        shown.set(false);
    };
    let callback = StoredValue::new(format!(
        "Register https://<the site's name>{CALLBACK_PATH} as the redirect URI with the provider, for each site that uses it."
    ));
    view! {
        <Dialog open=shown title=title testid="oidc-dialog" wide=true>
            <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
                {move || adding.get().then(|| name_input("oidc-name", "Name", d.name, name_problem, true, crate::screens::create::name_example(Some(&Entity::Oidc))))}
                {labelled("oidc-issuer", "Issuer", input("oidc-issuer", d.issuer, "https://accounts.google.com"), "The provider's address, for example https://accounts.google.com or https://sso.example.com/realms/home".to_string())}
                {labelled("oidc-client-id", "Client ID", input("oidc-client-id", d.client_id, "1234.apps.googleusercontent.com"), "The client ID you got when you registered burrow with the provider, for example 1234.apps.googleusercontent.com".to_string())}
                {move || if d.locked {
                    locked_secret(d.old.get(), d.name.get().trim())
                } else {
                    labelled("oidc-secret", "Client secret", input("oidc-secret", d.secret, "${GOOGLE_SECRET}"), secret_help(d.hidden.get())).into_any()
                }}
                <div class="md:col-span-2" data-testid="oidc-callback-note">{field_message(Tone::Info, callback.get_value())}</div>
            </div>
            <div class="mt-4 flex flex-wrap items-center justify-end gap-2">
                <span class="mr-auto text-xs text-text-muted" data-testid="dialog-status">{move || blocked.get().map(|b| format!("{}{}", b[..1].to_uppercase(), &b[1..]))}</span>
                <Button label="Cancel" on_click=move |_| shown.set(false)/>
                <Button
                    label=Signal::derive(move || if adding.get() { "Add login provider".to_string() } else { "Save".to_string() })
                    variant="primary"
                    testid="oidc-save"
                    disabled=Signal::derive(move || blocked.get().is_some())
                    reason=Signal::derive(move || blocked.get().unwrap_or_default())
                    on_click=move |_| save()
                />
            </div>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_secret_help_says_a_hidden_secret_stays_unless_replaced() {
        assert!(secret_help(Some(Value::Null)).contains("stays hidden"));
        assert!(secret_help(None).contains("${GOOGLE_SECRET}"));
    }
}
