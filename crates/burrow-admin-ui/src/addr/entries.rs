use burrow_admin_logic::entries::{Row, rows, to_values};
use burrow_admin_proto::schema::{Node, RefEntriesC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Mode, ServiceEntry};
use burrow_ui_kit::{Combobox, TextInput};
use leptos::prelude::*;

use crate::app::{MenuItem, row_menu, row_menus};
use crate::domain::{FieldCtx, set_checked};
use crate::edit::diags_at;
use crate::form::{Ctx, plain, ref_link};
use crate::store::store;

fn sibling(path: &[String], key: Vec<String>) -> Vec<String> {
    let mut p = path.to_vec();
    p.pop();
    p.extend(key);
    p
}

fn text_of(v: &Option<Value>) -> String {
    v.as_ref().map(plain).unwrap_or_default()
}

fn typed(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(match t.parse::<i64>() {
        Ok(n) => Value::from(n),
        Err(_) => Value::String(text.to_string()),
    })
}

#[derive(Clone)]
struct Place {
    fc: FieldCtx,
    single: Option<Vec<String>>,
}

impl Place {
    fn read(&self) -> Vec<Row> {
        let e = store().edit;
        let listed = e.value(&self.fc.path);
        match (&listed, &self.single) {
            (Some(_), _) | (None, None) => rows(listed.as_ref()),
            (None, Some(s)) => rows(e.value(s).as_ref()),
        }
    }

    fn write(&self, next: Vec<Row>) {
        let e = store().edit;
        if let Some(s) = &self.single {
            let one = (next.len() == 1 && next[0].target.is_none() && next[0].weight.is_none())
                .then(|| Value::String(next[0].name.clone()));
            let many = one.is_none() && !next.is_empty();
            e.set(s, one);
            set_checked(
                &self.fc,
                &self.fc.node,
                many.then(|| Value::Array(to_values(&next))),
            );
            return;
        }
        let v = (!next.is_empty()).then(|| Value::Array(to_values(&next)));
        set_checked(&self.fc, &self.fc.node, v);
    }

    fn edit(&self, f: impl FnOnce(&mut Vec<Row>)) {
        let mut all = self.read();
        f(&mut all);
        self.write(all);
    }
}

struct Look {
    balance: Signal<bool>,
    menus: RwSignal<Option<String>>,
    last: bool,
}

fn row_view(place: Place, c: RefEntriesC, i: usize, look: Look) -> AnyView {
    let Look {
        balance,
        menus,
        last,
    } = look;
    let e = store().edit;
    let id = format!("{}-{i}", place.fc.id);
    let current = {
        let place = place.clone();
        move || place.read().get(i).cloned().unwrap_or_default()
    };
    let name = Signal::derive({
        let current = current.clone();
        move || {
            vec![current().name]
                .into_iter()
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
        }
    });
    let target = Signal::derive({
        let current = current.clone();
        move || text_of(&current().target)
    });
    let weight = Signal::derive({
        let current = current.clone();
        move || current().weight.as_ref().map(plain).unwrap_or_default()
    });
    let service_target = e
        .value(&sibling(&place.fc.path, ServiceEntry::TARGET.rel()))
        .map(|v| format!("uses {}", plain(&v)))
        .unwrap_or_else(|| "uses the service target".into());
    let options = {
        let (entity, reserved) = (c.entity.clone(), c.reserved.clone());
        Signal::derive(move || super::picker::options_for(&entity, &reserved))
    };
    let on = |f: fn(&mut Vec<Row>, usize, String)| {
        let place = place.clone();
        Callback::new(move |t: String| place.edit(|all| f(all, i, t)))
    };
    let set_name = {
        let place = place.clone();
        Callback::new(move |picked: Vec<String>| {
            if let Some(n) = picked.into_iter().next() {
                place.edit(|all| all[i].name = n);
            }
        })
    };
    let set_target = on(|all, i, t| all[i].target = typed(&t));
    let set_weight = on(|all, i, t| all[i].weight = typed(&t).filter(|w| w.as_i64() != Some(1)));
    let act = |f: fn(&mut Vec<Row>, usize)| {
        let place = place.clone();
        Callback::new(move |_: ()| place.edit(|all| f(all, i)))
    };
    let up = act(|all, i| {
        if i > 0 {
            all.swap(i, i - 1)
        }
    });
    let down = act(|all, i| {
        if i + 1 < all.len() {
            all.swap(i, i + 1)
        }
    });
    let again = act(|all, i| {
        let name = all[i].name.clone();
        all.insert(i + 1, Row::named(&name));
    });
    let remove = act(|all, i| {
        all.remove(i);
    });
    let mut items = Vec::new();
    if i > 0 {
        items.push(MenuItem {
            label: "Move up",
            testid: "entry-up",
            danger: false,
            run: up,
        });
    }
    if !last {
        items.push(MenuItem {
            label: "Move down",
            testid: "entry-down",
            danger: false,
            run: down,
        });
    }
    items.push(MenuItem {
        label: "Another target",
        testid: "entry-again",
        danger: false,
        run: again,
    });
    items.push(MenuItem {
        label: "Remove",
        testid: "entry-remove",
        danger: true,
        run: remove,
    });
    let menu = row_menu(
        menus,
        i.to_string(),
        format!("More for entry {}", i + 1),
        items,
    );
    let weight_id = format!("{id}-weight");
    let mut at = place.fc.path.clone();
    at.push(i.to_string());
    let problem = move || {
        e.plan.with(|p| {
            diags_at(p.as_deref(), &at, true)
                .first()
                .map(|d| burrow_admin_logic::wording::shown(&d.code, &d.message, &d.subjects))
        })
    };
    view! {
        <div class="rounded-input border border-border px-3 py-2" data-entry=i.to_string() data-chip=move || name.get().first().cloned().unwrap_or_default()>
            <div class="flex flex-wrap items-end gap-2">
                <div class="min-w-[160px] flex-1">
                    <Combobox id=format!("{id}-client") options=options selected=name on_change=set_name placeholder="Pick a client…" label="Client"/>
                </div>
                <div class="min-w-[180px] flex-1">
                    <TextInput id=format!("{id}-target") value=target on_input=set_target mono=true placeholder=service_target label="Target"/>
                </div>
                <div class="w-[140px]" class:hidden=move || !balance.get() data-weight="">
                    <TextInput id=weight_id value=weight on_input=set_weight numeric=true placeholder="1" label="Weight"/>
                </div>
                <div class="pb-1.5">{menu}</div>
            </div>
            {move || problem().map(|m| view! { <p class="mt-1 text-xs text-danger" data-entry-error="">{m}</p> })}
        </div>
    }
    .into_any()
}

pub fn entries(fc: FieldCtx, c: RefEntriesC) -> AnyView {
    let e = store().edit;
    let single = super::merged_partner(&fc.spath).map(|s| sibling(&fc.path, vec![s]));
    let place = Place {
        fc: fc.clone(),
        single,
    };
    let mode_path = sibling(&fc.path, ServiceEntry::MODE.rel());
    let count = {
        let place = place.clone();
        Memo::new(move |_| place.read().len())
    };
    let balance = {
        let place = place.clone();
        Signal::derive(move || {
            let mode = e
                .value(&mode_path)
                .and_then(|v| v.as_str().and_then(Mode::from_wire));
            mode == Some(Mode::Balance) || place.read().iter().any(|r| r.weight.is_some())
        })
    };
    let options = {
        let (entity, reserved) = (c.entity.clone(), c.reserved.clone());
        Signal::derive(move || super::picker::options_for(&entity, &reserved))
    };
    let (create_label, on_create) = super::picker::create_for(&c.entity);
    let touched = fc.touched;
    let add = {
        let place = place.clone();
        Callback::new(move |picked: Vec<String>| {
            if let Some(n) = picked.into_iter().next() {
                place.edit(|all| all.push(Row::named(&n)));
            }
        })
    };
    let menus = row_menus();
    let rows = {
        let (place, c) = (place.clone(), c.clone());
        move || {
            let n = count.get();
            (0..n)
                .map(|i| {
                    row_view(
                        place.clone(),
                        c.clone(),
                        i,
                        Look {
                            balance,
                            menus,
                            last: i + 1 == n,
                        },
                    )
                })
                .collect_view()
        }
    };
    let head = move || {
        (count.get() > 0).then(|| view! {
            <div class="flex gap-2 px-3 text-xs font-semibold text-text-muted max-sm:hidden" data-entries-head="">
                <span class="min-w-[160px] flex-1">"Client"</span>
                <span class="min-w-[180px] flex-1">"Target"</span>
                <span class="w-[140px]" class:hidden=move || !balance.get()>"Weight"</span>
                <span class="w-7" aria-hidden="true"></span>
            </div>
        })
    };
    view! {
        <div class="space-y-2" data-entries="" on:focusout=move |_| { let _ = touched.try_set(true); }>
            {head}
            {rows}
            <div class="max-w-[320px]" data-add-entry="">
                <Combobox id=fc.id.clone() options=options selected=Signal::stored(Vec::new()) on_change=add placeholder="Add client…" label="Add client" create_label=create_label on_create=on_create/>
            </div>
        </div>
    }
    .into_any()
}

pub fn entry_line(row: &Row) -> String {
    let target = row
        .target
        .as_ref()
        .map(plain)
        .unwrap_or_else(|| "(service target)".into());
    match &row.weight {
        Some(w) => format!("→ {target} · weight {}", plain(w)),
        None => format!("→ {target}"),
    }
}

pub fn entries_view(c: &RefEntriesC, _node: &Node, v: &Value, ctx: &Ctx) -> AnyView {
    let all = rows(Some(v));
    if all.is_empty() {
        return view! { <span class="text-text-muted">"None"</span> }.into_any();
    }
    let lines = all
        .iter()
        .map(|r| {
            view! {
                <li class="flex flex-wrap items-center gap-2" data-entry-line=r.name.clone()>
                    {ref_link(&c.entity, &r.name, &c.reserved, ctx)}
                    <span class="font-mono text-[13px] text-text-muted">{entry_line(r)}</span>
                </li>
            }
        })
        .collect_view();
    view! { <ul class="space-y-1">{lines}</ul> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_view_line_names_the_target_and_the_weight() {
        let all = rows(Some(
            &json!(["a", {"name": "gw", "target": "10.0.0.12:3000", "weight": 2}]),
        ));
        assert_eq!(entry_line(&all[0]), "→ (service target)");
        assert_eq!(entry_line(&all[1]), "→ 10.0.0.12:3000 · weight 2");
        assert_eq!(typed(" 3001 "), Some(json!(3001)));
        assert_eq!(typed("10.0.0.12:80"), Some(json!("10.0.0.12:80")));
        assert_eq!(typed(""), None);
    }
}
