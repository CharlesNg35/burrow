use burrow_admin_logic::files::{main_name, save_in_file, save_in_options};
use burrow_ui_kit::Combobox;
use leptos::prelude::*;

use crate::store::store;

pub(crate) fn save_in_field(name: RwSignal<String>, choice: RwSignal<String>) -> impl IntoView {
    let s = store();
    s.files_on().then(|| {
        let options = Memo::new(move |_| {
            let n = name.get();
            crate::addr::plain_options(
                s.with_files(|i| save_in_options(i, if n.is_empty() { "new" } else { &n }))
                    .unwrap_or_default(),
            )
        });
        let value = Signal::derive(move || {
            let c = choice.get();
            vec![if c.is_empty() {
                s.with_files(main_name).unwrap_or_default()
            } else {
                c
            }]
        });
        view! {
            <div data-testid="save-in">
                <span class="mb-1.5 block text-sm font-semibold">"Save in"</span>
                <Combobox id="new-save-in" options=options selected=value on_change=Callback::new(move |v: Vec<String>| choice.set(v.into_iter().next().unwrap_or_default())) header="Config files" label="Save in"/>
                <p class="mt-1.5 text-xs text-text-muted">"The config file this is written to. A file of its own goes in the folder next to the config, for example server.d/blog.yaml."</p>
            </div>
        }
    })
}

pub(crate) fn place_new(section: &str, name: &str, choice: &str) {
    let s = store();
    if choice.is_empty() || !s.files_on() {
        return;
    }
    let Some((file, main)) = s.with_files(|i| (save_in_file(i, choice, name), main_name(i))) else {
        return;
    };
    s.edit
        .place(&[section.to_string(), name.to_string()], &file, &main);
}
