use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_syntax::name::{check_name, suggest};
use burrow_ui_kit::{Button, Dialog, TextInput};
use leptos::prelude::*;

use crate::api;
use crate::route::{Route, go};
use crate::store::store;

pub fn singular(section: &str) -> &'static str {
    match section {
        "clients" => "client",
        "visitors" => "visitor",
        _ => "service",
    }
}

pub fn taken(section: &str) -> Vec<String> {
    store().edit.draft.with(|d| {
        d.draft()
            .get(section)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    })
}

pub fn name_problem(name: &str, section: &str, except: Option<&str>) -> Option<String> {
    if let Err(e) = check_name(name) {
        return Some(e);
    }
    (taken(section).iter().any(|t| t == name) && except != Some(name))
        .then(|| format!("a {} named {name} already exists", singular(section)))
}

fn template(section: &str) -> Value {
    match section {
        "services" => {
            let first = taken("clients").into_iter().next();
            match first {
                Some(c) => json!({ "client": c, "target": 8080 }),
                None => json!({ "target": 8080 }),
            }
        }
        _ => json!({}),
    }
}

pub fn create(section: &str, name: &str) {
    let e = store().edit;
    let path = vec![section.to_string(), name.to_string()];
    e.set(&path, Some(template(section)));
    if section != "services" {
        let secret = vec![section.to_string(), name.to_string(), "secret".to_string()];
        api::request(msg::req::GENERATE_SECRET, json!({}), move |r| {
            if let Ok(Ok(g)) = r.map(serde_json::from_value::<GeneratedSecret>) {
                e.add_secret(&secret, &g.handle, &g.fp);
            }
        });
    }
    go(Route::Detail(section.to_string(), name.to_string()));
}

#[component]
pub fn NewEntity(section: String, open: RwSignal<bool>) -> impl IntoView {
    let what = singular(&section);
    let base = match what {
        "client" => "home",
        "visitor" => "laptop",
        _ => "web",
    };
    let name = RwSignal::new(String::new());
    let sec = section.clone();
    let problem = Signal::derive(move || {
        let n = name.get();
        if n.is_empty() {
            None
        } else {
            name_problem(&n, &sec, None)
        }
    });
    let sec2 = section.clone();
    let hint = Signal::derive(move || {
        let t = taken(&sec2);
        let refs: Vec<&str> = t.iter().map(String::as_str).collect();
        format!("for example {}", suggest(base, &refs))
    });
    let sec3 = StoredValue::new(section.clone());
    let create_now = Callback::new(move |_: ()| {
        let n = name.get_untracked();
        let sec = sec3.get_value();
        if n.is_empty() || name_problem(&n, &sec, None).is_some() {
            return;
        }
        open.set(false);
        name.set(String::new());
        create(&sec, &n);
    });
    let hint_now = StoredValue::new(hint.get_untracked());
    view! {
        <Dialog open=open title=format!("New {what}") testid="new-entity">
            <label class="mb-1 block font-medium" for="new-name">"Name"</label>
            <TextInput
                id="new-name"
                value=name
                on_input=move |t: String| name.set(t)
                placeholder=hint_now.get_value()
                invalid=Signal::derive(move || problem.get().is_some())
                described_by="new-name-msg"
                mono=true
            />
            <p id="new-name-msg" class="mt-1 min-h-5 text-sm text-danger">{move || problem.get()}</p>
            <p class="mb-3 text-sm text-text-muted">{move || hint.get()}</p>
            <div class="flex justify-end gap-2" on:keydown=move |e| if e.key() == "Enter" { create_now.run(()) }>
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label=format!("Add {what}")
                    variant="primary"
                    testid="create"
                    disabled=Signal::derive(move || name.get().is_empty() || problem.get().is_some())
                    on_click=move |_| create_now.run(())
                />
            </div>
        </Dialog>
    }
}

#[component]
pub fn RenameEntity(section: String, name: String, open: RwSignal<bool>) -> impl IntoView {
    let what = singular(&section);
    let to = RwSignal::new(name.clone());
    let (sec, old) = (section.clone(), name.clone());
    let problem = Signal::derive(move || {
        let n = to.get();
        if n == old {
            None
        } else {
            name_problem(&n, &sec, None)
        }
    });
    let note = match what {
        "client" => {
            "The client.yaml on that machine must use the new name, or the client cannot connect."
        }
        "visitor" => {
            "The visitor.yaml on that machine must use the new name, or the visitor cannot connect."
        }
        _ => "Visitors that forward this service by name must use the new name.",
    };
    let target = StoredValue::new((section.clone(), name.clone()));
    let rename_now = Callback::new(move |_: ()| {
        let (sec2, old2) = target.get_value();
        let n = to.get_untracked();
        open.set(false);
        if n != old2 && store().edit.rename(&[sec2.clone(), old2.clone()], &n) {
            go(Route::Detail(sec2, n));
        }
    });
    view! {
        <Dialog open=open title=format!("Rename {what} {name}") testid="rename-entity">
            <label class="mb-1 block font-medium" for="rename-to">"New name"</label>
            <TextInput
                id="rename-to"
                value=to
                on_input=move |t: String| to.set(t)
                invalid=Signal::derive(move || problem.get().is_some())
                described_by="rename-msg"
                mono=true
            />
            <p id="rename-msg" class="mt-1 min-h-5 text-sm text-danger">{move || problem.get()}</p>
            <p class="mb-3 text-sm text-warning">{note}" References in server.yaml are updated too."</p>
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label="Rename"
                    variant="primary"
                    testid="rename"
                    disabled=Signal::derive(move || problem.get().is_some() || to.get().is_empty())
                    on_click=move |_| rename_now.run(())
                />
            </div>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singular_names() {
        assert_eq!(singular("clients"), "client");
        assert_eq!(singular("services"), "service");
    }
}
