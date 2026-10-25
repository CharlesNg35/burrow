use burrow_admin_logic::changeset::as_new_password;
use burrow_admin_proto::msg::{self, HashedPassword};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::is_hash;
use burrow_syntax::passwd::valid_user;
use burrow_ui_kit::{Button, TextInput};
use leptos::prelude::*;

use crate::domain::FieldCtx;
use crate::edit::Edit;
use crate::store::store;

fn status(v: &Value) -> String {
    if is_hash(v) {
        "set".into()
    } else if as_new_password(v).is_some() {
        "new password, not applied yet".into()
    } else if v.as_str().is_some() {
        "hash pasted; checked when you review".into()
    } else {
        String::new()
    }
}

fn user_path(fc: &FieldCtx, user: &str) -> Vec<String> {
    let mut p = fc.path.clone();
    p.push(user.to_string());
    p
}

fn send(e: Edit, path: Vec<String>, password: String, note: RwSignal<Option<String>>) {
    crate::api::request(
        msg::req::HASH_PASSWORD,
        serde_json::to_value(msg::HashPassword { password }).unwrap_or(Value::Null),
        move |r| match r.map(serde_json::from_value::<HashedPassword>) {
            Ok(Ok(h)) => {
                note.set(None);
                e.set_password(&path, &h.handle);
            }
            Ok(Err(_)) => note.set(Some("the server's answer was unreadable".into())),
            Err(err) => note.set(Some(err.message)),
        },
    );
}

fn user_row(fc: FieldCtx, user: String, value: Value) -> impl IntoView {
    let e = store().edit;
    let changing = RwSignal::new(false);
    let password = RwSignal::new(String::new());
    let note = RwSignal::new(None::<String>);
    let path = user_path(&fc, &user);
    let p2 = path.clone();
    let save = Callback::new(move |_| {
        let pw = password.get_untracked();
        if pw.is_empty() {
            return;
        }
        password.set(String::new());
        changing.set(false);
        send(e, p2.clone(), pw, note);
    });
    let remove = Callback::new(move |_| {
        fc.touched.set(true);
        e.set(&path, None);
    });
    let id = format!("{}-{}", fc.id, user);
    let label = format!("New password for {user}");
    view! {
        <li class="flex flex-col gap-1.5 py-1.5" data-user=user.clone()>
            <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono text-[13px] font-semibold">{user.clone()}</span>
                <span class="text-xs text-text-muted" data-testid="password-status">{status(&value)}</span>
                <span class="ml-auto"></span>
                <Button label="New password" small=true testid="password-change" on_click=Callback::new(move |_| changing.update(|c| *c = !*c))/>
                <Button label="Remove" small=true variant="ghost-danger" testid="password-remove" on_click=remove/>
            </div>
            {move || changing.get().then(|| view! {
                <div class="flex items-center gap-2">
                    <TextInput id=format!("{id}-new") value=password on_input=move |v: String| password.set(v) label=label.clone() new_password=true/>
                    <Button label="Set" small=true variant="primary" testid="password-set" on_click=save/>
                </div>
            })}
            {move || note.get().map(|m| view! { <p class="text-xs text-danger" role="status">{m}</p> })}
        </li>
    }
}

pub fn passwords_editor(fc: FieldCtx) -> AnyView {
    let s = store();
    let e = s.edit;
    let path = fc.path.clone();
    let users = Memo::new(move |_| match e.value(&path) {
        Some(Value::Object(m)) => m.into_iter().collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    let user = RwSignal::new(String::new());
    let secret = RwSignal::new(String::new());
    let pasting = RwSignal::new(false);
    let note = RwSignal::new(None::<String>);
    let problem = Memo::new(move |_| {
        let u = user.get().trim().to_string();
        if u.is_empty() {
            return None;
        }
        if !valid_user(&u) {
            return Some("a user name holds no \":\"".to_string());
        }
        users
            .get()
            .iter()
            .any(|(k, _)| *k == u)
            .then(|| format!("{u} is already set; use New password"))
    });
    let fc2 = fc.clone();
    let add = Callback::new(move |_| {
        let u = user.get_untracked().trim().to_string();
        let given = secret.get_untracked();
        if u.is_empty() || given.is_empty() || problem.get_untracked().is_some() {
            return;
        }
        fc2.touched.set(true);
        let path = user_path(&fc2, &u);
        if pasting.get_untracked() {
            e.set(&path, Some(Value::String(given.trim().to_string())));
        } else {
            send(e, path, given, note);
        }
        user.set(String::new());
        secret.set(String::new());
    });
    let fc3 = fc.clone();
    let rows = move || {
        users
            .get()
            .into_iter()
            .map(|(u, v)| user_row(fc3.clone(), u, v))
            .collect_view()
    };
    let id = fc.id.clone();
    let can = s.can_write_now();
    view! {
        <div id=fc.id.clone() class="flex flex-col gap-1" data-testid="passwords">
            <ul class="divide-y divide-border">{rows}</ul>
            {move || users.with(Vec::is_empty).then(|| view! { <p class="text-[13px] text-text-muted" data-testid="passwords-none">"No users yet."</p> })}
            <div class="flex flex-wrap items-start gap-2 pt-1">
                <div class="w-44">
                    <TextInput id=format!("{id}-user") value=user on_input=move |v: String| user.set(v) label="User name" placeholder="user name" mono=true invalid=Signal::derive(move || problem.get().is_some())/>
                </div>
                <div class="w-60">
                    {move || if pasting.get() {
                        view! { <TextInput id=format!("{id}-hash") value=secret on_input=move |v: String| secret.set(v) label="Password hash" placeholder="$argon2id$..." mono=true/> }.into_any()
                    } else {
                        view! { <TextInput id=format!("{id}-password") value=secret on_input=move |v: String| secret.set(v) label="Password" placeholder="password" new_password=true/> }.into_any()
                    }}
                </div>
                <Button label="Add user" small=true disabled=!can testid="password-add" on_click=add/>
            </div>
            {move || problem.get().or(note.get()).map(|m| view! { <p class="text-xs text-danger" role="status" data-testid="password-problem">{m}</p> })}
            <button type="button" class="self-start text-xs text-accent-ink hover:underline" data-testid="password-paste" on:click=move |_| { pasting.update(|p| *p = !*p); secret.set(String::new()); }>
                {move || if pasting.get() { "Type a password instead" } else { "Paste a hash instead (from burrow passwd or htpasswd -B)" }}
            </button>
            <p class="text-xs text-text-muted">"burrow keeps only a hash of each password; nobody can read a password back, here or in server.yaml."</p>
        </div>
    }
    .into_any()
}
