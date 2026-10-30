use burrow_admin_logic::logins::{config, kind_of, kinds, preset};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{Value, json};
use burrow_admin_proto::value::child;
use burrow_config::{Entry, ForwardAuthConfig, ServiceEntry};
use burrow_ui_kit::{Button, ChipInput, Dialog, Select, TextInput, Tone, field_message};
use leptos::prelude::*;

use crate::screens::create::{name_input, problem_of};
use crate::store::store;

use super::{Open, base};

#[derive(Clone, Copy)]
struct Draft {
    kind: RwSignal<String>,
    name: RwSignal<String>,
    service: RwSignal<String>,
    path: RwSignal<String>,
    headers: RwSignal<Vec<String>>,
}

impl Draft {
    fn new() -> Self {
        Draft {
            kind: RwSignal::new(String::new()),
            name: RwSignal::new(String::new()),
            service: RwSignal::new(String::new()),
            path: RwSignal::new(String::new()),
            headers: RwSignal::new(Vec::new()),
        }
    }

    fn fill(&self, open: &Open) {
        let first = kinds()[0].to_string();
        match open {
            Open::Edit(n) => {
                let c = store()
                    .edit
                    .value(&child(&base(), n))
                    .and_then(|v| config(&v));
                let c = c.unwrap_or(ForwardAuthConfig {
                    service: String::new(),
                    path: String::new(),
                    copy_headers: Vec::new(),
                });
                self.kind.set(kind_of(&c.path).to_string());
                self.name.set(n.clone());
                self.service.set(c.service);
                self.path.set(c.path);
                self.headers.set(c.copy_headers);
            }
            _ => {
                self.service.set(String::new());
                self.pick(&first, true);
            }
        }
    }

    fn pick(&self, kind: &str, name_too: bool) {
        self.kind.set(kind.to_string());
        let p = preset(kind);
        if name_too {
            self.name
                .set(p.map(|p| p.name.to_string()).unwrap_or_default());
        }
        self.path
            .set(p.map(|p| p.path.to_string()).unwrap_or_default());
        self.headers.set(
            p.map(|p| p.headers.iter().map(|h| h.to_string()).collect())
                .unwrap_or_default(),
        );
    }

    fn value(&self) -> Value {
        let mut v = json!({
            ForwardAuthConfig::SERVICE.key(): self.service.get_untracked(),
            ForwardAuthConfig::PATH.key(): self.path.get_untracked().trim(),
        });
        let h = self.headers.get_untracked();
        if !h.is_empty() {
            v[ForwardAuthConfig::COPY_HEADERS.key()] = json!(h);
        }
        v
    }
}

fn flags_as_code(text: &'static str) -> impl IntoView {
    text.split(' ')
        .enumerate()
        .map(|(i, w)| {
            let gap = (i > 0).then_some(" ");
            if w.starts_with("--") {
                view! { {gap}<code class="font-mono">{w}</code> }.into_any()
            } else {
                view! { {gap}{w} }.into_any()
            }
        })
        .collect_view()
}

fn header_ok(h: String) -> Option<String> {
    (!burrow_syntax::web::headers::is_name(h.trim()))
        .then(|| "a header name has no spaces or colons, like Remote-User".to_string())
}

fn services() -> Vec<(String, String)> {
    let doc = store().edit.doc();
    let mut out = vec![(String::new(), "Pick the login portal's service".to_string())];
    out.extend(
        ServiceEntry::entries(&doc)
            .map(|m| m.keys().map(|k| (k.clone(), k.clone())).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    out
}

fn labelled(id: &'static str, label: &'static str, body: AnyView) -> impl IntoView {
    view! {
        <div>
            <label class="mb-1.5 block text-sm font-semibold" for=id>{label}</label>
            {body}
        </div>
    }
}

pub(super) fn login_dialog(open: RwSignal<Open>) -> impl IntoView {
    let e = store().edit;
    let d = Draft::new();
    Effect::new(move |_| d.fill(&open.get()));
    let shown = crate::app::shown_while(open, |o| *o != Open::Closed, |o| *o = Open::Closed);
    let adding = Signal::derive(move || open.get() == Open::Add);
    let name_problem = Signal::derive(move || {
        if !adding.get() {
            return None;
        }
        problem_of(&Entity::Login, d.name.get().trim())
    });
    let path_problem = Signal::derive(move || {
        let p = d.path.get();
        burrow_syntax::url_path::route_path(p.trim()).err()
    });
    let service_problem = Signal::derive(move || {
        d.service
            .with(String::is_empty)
            .then(|| "pick the service of the login portal".to_string())
    });
    let blocked = Signal::derive(move || {
        name_problem
            .get()
            .or(service_problem.get())
            .or(path_problem.get())
    });
    let title = Signal::derive(move || match open.get() {
        Open::Edit(n) => format!("Edit login service {n}"),
        _ => "Add a login service".to_string(),
    });
    let kind_options: Vec<(String, String)> = kinds()
        .into_iter()
        .map(|k| (k.to_string(), k.to_string()))
        .collect();
    let note = move || {
        preset(&d.kind.get())
            .map(|p| p.note)
            .filter(|n| !n.is_empty())
            .map(|n| view! { <div data-testid="login-preset-note">{field_message(Tone::Info, flags_as_code(n))}</div> })
    };
    let save = move || {
        let name = match open.get_untracked() {
            Open::Edit(n) => n,
            _ => d.name.get_untracked().trim().to_string(),
        };
        e.set(&child(&base(), &name), Some(d.value()));
        shown.set(false);
    };
    let on_kind = Callback::new(move |k: String| d.pick(&k, adding.get_untracked()));
    let on_service = Callback::new(move |s: String| d.service.set(s));
    let on_path = Callback::new(move |t: String| d.path.set(t));
    let on_headers = Callback::new(move |h: Vec<String>| d.headers.set(h));
    let path_bad = Signal::derive(move || path_problem.get().is_some());
    view! {
        <Dialog open=shown title=title testid="login-dialog" wide=true>
            <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
                <div>
                    {labelled("login-kind", "Kind", view! { <Select id="login-kind" options=kind_options.clone() value=d.kind on_change=on_kind/> }.into_any())}
                    {note}
                </div>
                {labelled("login-service", "Service", view! { <Select id="login-service" options=Signal::derive(services) value=d.service on_change=on_service/> }.into_any())}
                {move || adding.get().then(|| name_input("login-name", "Name", d.name, name_problem, true, crate::screens::create::name_example(Some(&Entity::Login))))}
                <div class="md:col-span-2">
                    {labelled("login-path", "Path", view! { <TextInput id="login-path" value=d.path on_input=on_path invalid=path_bad mono=true placeholder="/api/authz/forward-auth"/> }.into_any())}
                    <p class="mt-1.5 text-xs text-danger">{move || path_problem.get()}</p>
                </div>
                <div class="md:col-span-2">
                    {labelled("login-headers", "Headers to the app", view! { <ChipInput id="login-headers" items=d.headers on_change=on_headers check=header_ok placeholder="Add a header" empty_label="None"/> }.into_any())}
                    <p class="mt-1.5 text-xs text-text-muted">"Headers from the login service's answer that the app receives; a visitor's own copies are always removed."</p>
                </div>
            </div>
            <div class="mt-4 flex flex-wrap items-center justify-end gap-2">
                <span class="mr-auto text-xs text-text-muted" data-testid="dialog-status">{move || blocked.get().map(|b| format!("{}{}", b[..1].to_uppercase(), &b[1..]))}</span>
                <Button label="Cancel" on_click=move |_| shown.set(false)/>
                <Button
                    label=Signal::derive(move || if adding.get() { "Add login service".to_string() } else { "Save".to_string() })
                    variant="primary"
                    testid="login-save"
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
    fn a_header_name_is_a_plain_token() {
        assert_eq!(header_ok("Remote-User".into()), None);
        assert!(header_ok("Remote User".into()).is_some());
        assert!(header_ok(String::new()).is_some());
    }

    #[test]
    fn other_has_no_preset() {
        assert!(preset(burrow_admin_logic::logins::OTHER).is_none());
    }
}
