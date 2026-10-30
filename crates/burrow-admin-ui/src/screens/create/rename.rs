use burrow_admin_proto::schema::Entity;
use burrow_config::{RESERVED_LOGIN_NAMES, entity_path};
use burrow_syntax::name::check_name;
use burrow_ui_kit::{Button, Dialog};
use leptos::prelude::*;

use crate::domain::names_of;
use crate::route::{Route, go};
use crate::store::store;

use super::{name_input, name_problem};

fn what(entity: &Entity) -> String {
    match entity {
        Entity::Login => "login service".to_string(),
        e => e.as_str().to_string(),
    }
}

fn note(entity: &Entity) -> &'static str {
    match entity {
        Entity::Client => {
            "The client.yaml on that machine must use the new name, or the client cannot connect."
        }
        Entity::Visitor => {
            "The visitor.yaml on that machine must use the new name, or the visitor cannot connect."
        }
        Entity::Login => "Websites that use it are renamed with it.",
        _ => "Visitors that forward this service by name must use the new name.",
    }
}

pub(crate) fn problem_of(entity: &Entity, name: &str) -> Option<String> {
    if *entity != Entity::Login {
        return name_problem(name, entity.section(), None);
    }
    if let Err(e) = check_name(name) {
        return Some(e);
    }
    if RESERVED_LOGIN_NAMES.contains(&name) {
        return Some(format!(
            "{name} means on or off; give the login service another name"
        ));
    }
    names_of(entity)
        .iter()
        .any(|t| t == name)
        .then(|| format!("a login service named {name} already exists"))
}

#[component]
pub fn RenameEntity(entity: Entity, name: String, open: RwSignal<bool>) -> impl IntoView {
    let what = what(&entity);
    let note = note(&entity);
    let to = RwSignal::new(name.clone());
    let same = {
        let old = name.clone();
        Signal::derive(move || to.with(|t| t.trim() == old))
    };
    let (ent, old) = (entity.clone(), name.clone());
    let problem = Signal::derive(move || {
        let n = to.get();
        if n == old { None } else { problem_of(&ent, &n) }
    });
    let target = StoredValue::new((entity.clone(), name.clone()));
    let rename_now = Callback::new(move |_: ()| {
        let (ent, old) = target.get_value();
        let n = to.get_untracked();
        open.set(false);
        let mut path = entity_path(&ent);
        path.push(old.clone());
        if n != old && store().edit.rename(&path, &n) && ent != Entity::Login {
            go(Route::Detail(ent.section().to_string(), n));
        }
    });
    view! {
        <Dialog open=open title=format!("Rename {what} {name}") testid="rename-entity">
            {name_input("rename-to", "New name", to, problem, false)}
            <p class=move || if same.get() { "my-3 text-sm text-text-muted" } else { "my-3 text-sm text-warning" }>{note}" References in server.yaml are updated too."</p>
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label="Rename"
                    variant="primary"
                    testid="rename"
                    disabled=Signal::derive(move || problem.get().is_some() || to.get().is_empty() || same.get())
                    reason=Signal::derive(move || if same.get() || to.get().is_empty() { "Type a new name first".to_string() } else if problem.get().is_some() { "Fix the name first".to_string() } else { String::new() })
                    on_click=move |_| rename_now.run(())
                />
            </div>
        </Dialog>
    }
}
