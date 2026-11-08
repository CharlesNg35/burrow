use burrow_admin_proto::schema::Entity;
use burrow_config::{RESERVED_LOGIN_NAMES, entity_path};
use burrow_syntax::name::{MAX_NAME_LEN, check_name};
use burrow_ui_kit::{Button, Dialog, TextInput};
use leptos::prelude::*;

use crate::domain::names_of;
use crate::route::{Route, go};
use crate::store::store;

use crate::entities::name_problem;

fn what(entity: &Entity) -> String {
    burrow_admin_logic::entity::info(entity)
        .singular
        .to_string()
}

fn note(entity: &Entity) -> &'static str {
    burrow_admin_logic::entity::info(entity).rename_note
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
        if n != old && store().edit.rename(&path, &n) {
            store().added_to_changes();
            if ent != Entity::Login {
                go(Route::Detail(ent.section().to_string(), n));
            }
        }
    });
    view! {
        <Dialog open=open title=format!("Rename {what} {name}") testid="rename-entity">
            {name_input("rename-to", "New name", to, problem, false, burrow_admin_logic::entity::info(&entity).name_example)}
            <p class=move || if same.get() { "my-3 text-sm text-text-muted" } else { "my-3 text-sm text-warning" }>{note}" References in the config files are updated too."</p>
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| open.set(false)/>
                <Button
                    label="Rename"
                    variant=burrow_ui_kit::ButtonLook::Primary
                    testid="rename"
                    disabled=Signal::derive(move || problem.get().is_some() || to.get().is_empty() || same.get())
                    reason=Signal::derive(move || if same.get() || to.get().is_empty() { "Type a new name first".to_string() } else if problem.get().is_some() { "Fix the name first".to_string() } else { String::new() })
                    on_click=move |_| rename_now.run(())
                />
            </div>
        </Dialog>
    }
}

pub(crate) fn name_input(
    id: &'static str,
    label: &'static str,
    name: RwSignal<String>,
    problem: Signal<Option<String>>,
    available: bool,
    example: &'static str,
) -> impl IntoView {
    let msg = format!("{id}-msg");
    let ok = move || available && !name.get().is_empty() && problem.get().is_none();
    view! {
    <div>
        <label class="mb-1.5 block text-sm font-semibold" for=id>{label}</label>
        <div class="relative">
            <TextInput
                id=id
                value=name
                on_input=move |t: String| name.set(t)
                invalid=Signal::derive(move || problem.get().is_some())
                described_by=msg.clone()
                mono=true
            />
            {move || ok().then(|| view! {
                <span class="pointer-events-none absolute top-1/2 right-2.5 flex -translate-y-1/2 items-center gap-1 text-xs font-semibold text-success"><burrow_ui_kit::Icon name="check" small=true/>"available"</span>
            })}
        </div>
        <p id=msg class="mt-1.5 text-xs text-danger">{move || problem.get()}</p>
        <p class="text-xs text-text-muted">
            {move || ok().then(|| view! { <span class="sr-only" data-testid="name-available">"available"</span> })}
            {format!("For example {example}. Letters, digits, dot, dash, underscore; up to {MAX_NAME_LEN} characters.")}
        </p>
    </div>
    }
}
