use burrow_admin_logic::fallback::needs_newer_ui;
use burrow_admin_logic::one_of::{entry_of, filled, label, opening, options, picked, switch};
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Field, Node, OneOf, SemType};
use burrow_admin_proto::value::child;
use burrow_ui_kit::{Button, ButtonLook, Dialog, FieldRow, FieldState, Select, Tone};
use leptos::prelude::*;

use crate::edit::diags_at;
use crate::store::store;

use super::{ShowKeys, editing_now, field_id, join, object_view, row};

const NONE: &str = "";

pub(super) struct Choice<'a> {
    pub node: &'a Node,
    pub group: &'a OneOf,
    pub path: &'a Path,
    pub spath: &'a Path,
    pub entity: &'a Path,
}

fn member(node: &Node, key: &str) -> Option<Field> {
    fields(node).iter().find(|f| f.key == key).cloned()
}

fn locked_choice(node: &Node, keys: &[String]) -> bool {
    keys.iter()
        .filter_map(|k| member(node, k))
        .all(|f| f.node.locked || !f.node.editable.is_yes())
}

fn chosen_rows(c: &Choice, keys: &[String]) -> Vec<AnyView> {
    keys.iter()
        .filter_map(|k| member(c.node, k))
        .map(|f| {
            let (p, sp) = (child(c.path, &f.key), child(c.spath, &f.key));
            let n = inner(&f.node);
            if matches!(n.sem, SemType::Object(_)) && !needs_newer_ui(n) {
                view! { <div data-choice-fields=f.key.clone()>{object_view(&f.node, p, sp, c.entity.clone())}</div> }
                    .into_any()
            } else {
                row(f, p, sp, c.entity.clone())
            }
        })
        .collect()
}

fn needs(node: &Node, keys: &[String]) -> String {
    let wanted: Vec<String> = keys
        .iter()
        .filter_map(|k| member(node, k))
        .flat_map(|f| match &inner(&f.node).sem {
            SemType::Object(_) => fields(&f.node)
                .iter()
                .filter(|g| g.node.required)
                .map(|g| g.node.label.clone())
                .collect::<Vec<_>>(),
            _ => vec![f.node.label.clone()],
        })
        .collect();
    format!("{} needs its {}", label(node, keys), wanted.join(", "))
}

fn state_at(c: &Choice, shown: Memo<Option<usize>>) -> Signal<FieldState> {
    let e = store().edit;
    let (path, node, g) = (c.path.clone(), c.node.clone(), c.group.clone());
    Signal::derive(move || FieldState {
        error: e.plan.with(|p| {
            diags_at(p.as_deref(), &path, false)
                .into_iter()
                .find(|d| d.is_error() && d.path.as_deref() == Some(path.as_slice()))
                .map(
                    |d| match shown.get().and_then(|i| g.choices.get(i).map(|k| (i, k))) {
                        Some((i, keys)) if !filled(&g, e.value(&path).as_ref(), i) => {
                            needs(&node, keys)
                        }
                        _ => burrow_admin_logic::wording::diag_text(d),
                    },
                )
        }),
        ..FieldState::default()
    })
}

fn switch_dialog(
    c: &Choice,
    pending: RwSignal<Option<Option<usize>>>,
    apply: Callback<Option<usize>>,
) -> impl IntoView + use<> {
    let e = store().edit;
    let (node, g, path) = (c.node.clone(), c.group.clone(), c.path.clone());
    let open = crate::shown::shown_while(pending, Option::is_some, |p| *p = None);
    let name_of = move |i: Option<usize>| {
        i.and_then(|i| g.choices.get(i))
            .map(|k| label(&node, k))
            .unwrap_or_else(|| "none".into())
    };
    let title = {
        let name_of = name_of.clone();
        Signal::derive(move || format!("Switch to {}?", name_of(pending.get().flatten())))
    };
    let g2 = c.group.clone();
    let sub = Signal::derive(move || {
        let now = e.value(&path);
        let was = picked(&g2, now.as_ref());
        format!("The {} settings of this entry are removed.", name_of(was))
    });
    view! {
        <Dialog open=open title=title sub=sub icon="alert" tone=Tone::Warning testid="one-of-switch">
            <div class="flex justify-end gap-2">
                <Button label="Cancel" testid="one-of-cancel" on_click=move |_| open.set(false)/>
                <Button label="Switch" variant=ButtonLook::Primary testid="one-of-confirm" on_click=move |_| {
                    if let Some(to) = pending.get_untracked() {
                        apply.run(to);
                    }
                    open.set(false);
                }/>
            </div>
        </Dialog>
    }
}

pub(super) fn choice_rows(c: Choice) -> AnyView {
    let s = store();
    let e = s.edit;
    let (path, spath) = (c.path.clone(), c.spath.clone());
    let (node, g) = (c.node.clone(), c.group.clone());
    let p = path.clone();
    let now = Memo::new(move |_| e.value(&p));
    let g1 = g.clone();
    let picked_now = Memo::new(move |_| now.with(|v| picked(&g1, v.as_ref())));
    let editable: Vec<usize> = (0..g.choices.len())
        .filter(|i| !locked_choice(&node, &g.choices[*i]))
        .collect();
    let entry = e
        .draft
        .with_untracked(|d| entry_of(&g, burrow_admin_proto::value::get(d.base(), &path)));
    let wanted: RwSignal<Option<usize>> = RwSignal::new(opening(&g, &editable, entry));
    let shown = Memo::new(move |_| picked_now.get().or(wanted.get()));
    let apply = {
        let (node, g, path) = (node.clone(), g.clone(), path.clone());
        Callback::new(move |to: Option<usize>| {
            let v = switch(&node, &g, e.value(&path).as_ref(), to);
            e.set(&path, Some(v));
            wanted.set(to);
        })
    };
    let pending: RwSignal<Option<Option<usize>>> = RwSignal::new(None);
    let redraw = RwSignal::new(0u32);
    Effect::new(move |was: Option<bool>| {
        let open = pending.with(Option::is_some);
        if was == Some(true) && !open {
            redraw.update(|n| *n += 1);
        }
        open
    });
    let g2 = g.clone();
    let on_change = Callback::new(move |v: String| {
        let to = v.parse::<usize>().ok();
        let was = picked_now.get_untracked();
        if was.is_some()
            && was != to
            && now.with_untracked(|o| filled(&g2, o.as_ref(), was.unwrap_or(0)))
        {
            pending.set(Some(to));
        } else {
            apply.run(to);
        }
    });
    let none_label = if g.required { "Pick one" } else { "None" };
    let mut opts: Vec<(String, String)> = vec![(NONE.to_string(), none_label.to_string())];
    opts.extend(
        options(&node, &g)
            .into_iter()
            .map(|(i, l)| (i.to_string(), l)),
    );
    let names: Vec<String> = opts.iter().map(|(_, l)| l.clone()).collect();
    let table = use_context::<ShowKeys>().is_some_and(|k| k.0);
    let editing = editing_now();
    let all_locked = g.choices.iter().all(|k| locked_choice(&node, k));
    let can = s.can_write_now() && !all_locked;
    let key_path = child(&path, &g.key);
    let id = field_id(&key_path);
    let control = {
        let id = id.clone();
        let label_text = g.label.clone();
        move || {
            redraw.track();
            let value =
                Signal::derive(move || shown.get().map(|i| i.to_string()).unwrap_or_default());
            if can && editing.try_get().unwrap_or(false) {
                let opts = opts.clone();
                view! { <div class="max-w-80"><Select id=id.clone() options=opts value=value on_change=on_change label=label_text.clone()/></div> }.into_any()
            } else {
                let names = names.clone();
                let text = move || {
                    shown
                        .get()
                        .and_then(|i| names.get(i + 1).cloned())
                        .unwrap_or_else(|| names[0].clone())
                };
                view! { <div class="flex min-h-[38px] items-center text-[13px]" id=id.clone() data-testid="one-of-value">{text}</div> }.into_any()
            }
        }
    };
    let head = view! {
        <FieldRow
            id=id
            label=g.label.clone()
            help=g.help.clone()
            path=join(&key_path)
            schema_path=join(&child(&spath, &g.key))
            key_hint=if table { burrow_admin_logic::wording::key_hint(&path) } else { String::new() }
            table=table
            wide=true
            state=state_at(&c, shown)
        >
            {control}
        </FieldRow>
    };
    let rows = {
        let choices = g.choices.clone();
        let (node, path, spath, entity) =
            (node.clone(), path.clone(), spath.clone(), c.entity.clone());
        move || {
            let keys = shown
                .get()
                .and_then(|i| choices.get(i).cloned())
                .unwrap_or_default();
            let at = Choice {
                node: &node,
                group: &g,
                path: &path,
                spath: &spath,
                entity: &entity,
            };
            chosen_rows(&at, &keys).collect_view()
        }
    };
    let wrap = if table {
        "divide-y divide-rowline"
    } else {
        "contents"
    };
    view! {
        <div class=wrap data-one-of=c.group.key.clone()>
            {head}
            {rows}
            {switch_dialog(&c, pending, apply)}
        </div>
    }
    .into_any()
}
