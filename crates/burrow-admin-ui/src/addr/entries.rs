use burrow_admin_logic::entries::{Row, rows, to_values};
use burrow_admin_proto::schema::{Node, RefEntriesC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Mode, ServiceEntry};
use burrow_ui_kit::{Combobox, Stepper};
use leptos::prelude::*;

use super::entry_summary::summary;
use super::entry_target::{Open, focus_soon, input_id, shifted, swapped, target_cell, to_main};
use crate::app::{MenuItem, row_menu, row_menus};
use burrow_admin_logic::wording::{MAIN_TARGET, NOT_SET};

type OpenMap = std::collections::BTreeMap<usize, Option<Value>>;
use crate::domain::{FieldCtx, set_checked};
use crate::edit::diags_at;
use crate::form::{Ctx, plain, ref_link};
use crate::store::store;

pub(super) fn sibling(path: &[String], key: Vec<String>) -> Vec<String> {
    let mut p = path.to_vec();
    p.pop();
    p.extend(key);
    p
}

pub(super) fn text_of(v: &Option<Value>) -> String {
    v.as_ref().map(plain).unwrap_or_default()
}

pub(super) fn typed(text: &str) -> Option<Value> {
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
pub(super) struct Place {
    pub(super) fc: FieldCtx,
    single: Option<Vec<String>>,
}

impl Place {
    pub(super) fn read(&self) -> Vec<Row> {
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

    pub(super) fn edit(&self, f: impl FnOnce(&mut Vec<Row>)) {
        let mut all = self.read();
        f(&mut all);
        self.write(all);
    }
}

#[derive(Clone, Copy)]
struct Look {
    balance: Signal<bool>,
    menus: RwSignal<Option<String>>,
    open: Open,
    expanded: RwSignal<Option<usize>>,
    last: bool,
    own: bool,
}

type Follow = fn(Option<usize>, usize) -> Option<usize>;

fn swap_follow(x: Option<usize>, a: usize, b: usize) -> Option<usize> {
    match x {
        Some(k) if k == a => Some(b),
        Some(k) if k == b => Some(a),
        other => other,
    }
}

fn menu_items(place: &Place, i: usize, look: Look) -> Vec<MenuItem> {
    let Look {
        open,
        expanded,
        last,
        own,
        ..
    } = look;
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
    let then = |run: Callback<()>, after: fn(&mut OpenMap, usize), follow: Follow| {
        Callback::new(move |_: ()| {
            run.run(());
            open.update(|o| after(o, i));
            expanded.update(|x| *x = follow(*x, i));
        })
    };
    let mut items = Vec::new();
    if own {
        let place = place.clone();
        items.push(MenuItem {
            label: "Use main target",
            testid: "entry-main",
            danger: false,
            run: Callback::new(move |_: ()| to_main(&place, i, open)),
        });
    }
    let next = input_id(place, i + 1);
    let again = then(
        again,
        |o, i| {
            *o = shifted(o, i + 1, 1);
            o.insert(i + 1, None);
        },
        |_, i| Some(i + 1),
    );
    items.push(MenuItem {
        label: "Add again with another target",
        testid: "entry-again",
        danger: false,
        run: Callback::new(move |_: ()| {
            again.run(());
            focus_soon(next.clone());
        }),
    });
    if i > 0 {
        items.push(MenuItem {
            label: "Move up",
            testid: "entry-up",
            danger: false,
            run: then(
                up,
                |o, i| *o = swapped(o, i, i - 1),
                |x, i| swap_follow(x, i, i - 1),
            ),
        });
    }
    if !last {
        items.push(MenuItem {
            label: "Move down",
            testid: "entry-down",
            danger: false,
            run: then(
                down,
                |o, i| *o = swapped(o, i, i + 1),
                |x, i| swap_follow(x, i, i + 1),
            ),
        });
    }
    items.push(MenuItem {
        label: "Remove",
        testid: "entry-remove",
        danger: true,
        run: then(
            remove,
            |o, i| {
                o.remove(&i);
                *o = shifted(o, i + 1, -1);
            },
            |x, i| match x {
                Some(k) if k == i => None,
                Some(k) if k > i => Some(k - 1),
                other => other,
            },
        ),
    });
    items
}

const CLIENT_W: &str = "sm:w-[240px]";
const WEIGHT_W: &str = "sm:w-[96px]";
const TARGET_W: &str = "sm:w-[320px]";

fn cell_label(text: &'static str) -> impl IntoView {
    view! { <span class="w-14 shrink-0 text-xs font-semibold text-text-muted sm:hidden">{text}</span> }
}

fn row_view(place: Place, c: RefEntriesC, i: usize, look: Look) -> AnyView {
    let Look {
        balance,
        menus,
        open,
        expanded,
        ..
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
    let weight = Signal::derive(move || current().weight.as_ref().map(plain).unwrap_or_default());
    let options = {
        let (entity, reserved) = (c.entity.clone(), c.reserved.clone());
        Signal::derive(move || super::picker::options_for(&entity, &reserved))
    };
    let set_name = {
        let place = place.clone();
        Callback::new(move |picked: Vec<String>| {
            if let Some(n) = picked.into_iter().next() {
                place.edit(|all| all[i].name = n);
            }
        })
    };
    let set_weight = {
        let place = place.clone();
        Callback::new(move |t: String| {
            place.edit(|all| all[i].weight = typed(&t).filter(|w| w.as_i64() != Some(1)))
        })
    };
    let menu = row_menu(
        menus,
        i.to_string(),
        format!("More for entry {}", i + 1),
        menu_items(&place, i, look),
    );
    let phone_menu = row_menu(
        menus,
        format!("{i}-phone"),
        format!("More for entry {}", i + 1),
        menu_items(&place, i, look),
    );
    let mut at = place.fc.path.clone();
    at.push(i.to_string());
    let weight_at = {
        let mut w = at.clone();
        w.push(burrow_config::ServiceClient::WEIGHT.key().to_string());
        w
    };
    let weight_bad = Signal::derive(move || {
        e.plan
            .with(|p| !diags_at(p.as_deref(), &weight_at, true).is_empty())
    });
    let problem = Memo::new(move |_| {
        e.plan.with(|p| {
            diags_at(p.as_deref(), &at, true).first().map(|d| {
                let w = burrow_admin_logic::wording::shown(&d.code, &d.message, &d.subjects);
                burrow_admin_logic::wording::capital(&w)
            })
        })
    });
    Effect::new(move |_| {
        if problem.with(Option::is_some) {
            expanded.set(Some(i));
        }
    });
    let head = summary(&place, i, options, balance, expanded, phone_menu);
    let body = move || {
        if expanded.get() == Some(i) {
            "flex flex-wrap items-center gap-x-3 gap-y-2 max-sm:pb-2"
        } else {
            "flex flex-wrap items-center gap-x-3 gap-y-2 max-sm:hidden"
        }
    };
    let target = target_cell(&place, i, open);
    view! {
        <div class="py-2" data-entry=i.to_string() data-chip=move || name.get().first().cloned().unwrap_or_default()>
            {head}
            <div class=body>
                <div class=format!("order-1 min-w-0 flex-1 sm:flex-none {CLIENT_W}")>
                    <Combobox id=format!("{id}-client") options=options selected=name on_change=set_name placeholder="Pick a client…" label="Client"/>
                </div>
                <div class="order-2 max-sm:hidden sm:order-4">{menu}</div>
                <div class=format!("order-3 flex min-h-[38px] basis-full items-center gap-2 sm:order-2 sm:basis-auto {TARGET_W}")>
                    {cell_label("Target")}
                    {target}
                </div>
                <div class=format!("order-4 flex basis-full items-center gap-2 sm:order-3 sm:basis-auto {WEIGHT_W}") class:hidden=move || !balance.get() data-weight="">
                    {cell_label("Weight")}
                    <Stepper id=format!("{id}-weight") value=weight on_input=set_weight default=1 min=1 max=burrow_config::WEIGHT_MAX invalid=weight_bad placeholder="default 1" label="Weight" touch_only=true/>
                </div>
            </div>
            {move || problem.get().map(|m| view! { <p class="mt-1 text-xs text-danger" data-entry-error="">{m}</p> })}
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
    let is_balance = Signal::derive(move || {
        e.value(&mode_path)
            .and_then(|v| v.as_str().and_then(Mode::from_wire))
            == Some(Mode::Balance)
    });
    let weighted = {
        let place = place.clone();
        Memo::new(move |_| place.read().iter().any(|r| r.weight.is_some()))
    };
    let balance = Signal::derive(move || is_balance.get() || weighted.get());
    let stray = Signal::derive(move || weighted.get() && !is_balance.get());
    let clear_weights = {
        let place = place.clone();
        move |_| place.edit(|all| all.iter_mut().for_each(|r| r.weight = None))
    };
    let options = {
        let (entity, reserved) = (c.entity.clone(), c.reserved.clone());
        Signal::derive(move || super::picker::options_for(&entity, &reserved))
    };
    let (create_label, on_create) = super::picker::create_for(&c.entity);
    let touched = fc.touched;
    let expanded = RwSignal::new(None::<usize>);
    let add = {
        let place = place.clone();
        Callback::new(move |picked: Vec<String>| {
            if let Some(n) = picked.into_iter().next() {
                place.edit(|all| all.push(Row::named(&n)));
                expanded.set(Some(count.get_untracked().saturating_sub(1)));
            }
        })
    };
    let menus = row_menus();
    let open: Open = RwSignal::new(OpenMap::new());
    let flags = {
        let place = place.clone();
        Memo::new(move |_| {
            open.with(|o| {
                place
                    .read()
                    .iter()
                    .enumerate()
                    .map(|(i, r)| r.target.is_some() || o.contains_key(&i))
                    .collect::<Vec<_>>()
            })
        })
    };
    let rows = {
        let (place, c) = (place.clone(), c.clone());
        move || {
            let own = flags.get();
            let n = own.len();
            (0..n)
                .map(|i| {
                    untrack(|| {
                        row_view(
                            place.clone(),
                            c.clone(),
                            i,
                            Look {
                                balance,
                                menus,
                                open,
                                expanded,
                                last: i + 1 == n,
                                own: own[i],
                            },
                        )
                    })
                })
                .collect_view()
        }
    };
    let head = move || {
        (count.get() > 0).then(|| view! {
            <div class="flex gap-x-3 border-b border-border pb-1 text-xs font-semibold text-text-muted max-sm:hidden" data-entries-head="">
                <span class=CLIENT_W>"Client"</span>
                <span class=TARGET_W>"Target"</span>
                <span class=WEIGHT_W class:hidden=move || !balance.get()>"Weight"</span>
                <span class="w-9" aria-hidden="true"></span>
            </div>
        })
    };
    view! {
        <div data-entries="" on:focusout=move |_| { let _ = touched.try_set(true); }>
            {head}
            <div class="divide-y divide-border">{rows}</div>
            <Show when=move || stray.get()>
                <p class="mt-1 flex flex-wrap items-baseline gap-x-2 text-sm text-warning" data-stray-weights="">
                    {burrow_admin_logic::wording::WEIGHTS_NEED_BALANCE}
                    <button type="button" class=format!("cursor-pointer font-semibold {}", burrow_ui_kit::LINK) data-testid="clear-weights" on:click=clear_weights.clone()>"Clear weights"</button>
                </p>
            </Show>
            <div class="mt-2 max-w-[240px]" data-add-entry="">
                <Combobox id=fc.id.clone() options=options selected=Signal::stored(Vec::new()) on_change=add placeholder="Add client…" label="Add client" create_label=create_label on_create=on_create/>
            </div>
        </div>
    }
    .into_any()
}

pub fn entry_line(row: &Row, main: &str) -> String {
    let target = row
        .target
        .as_ref()
        .map(burrow_admin_logic::web::target_value_shown)
        .unwrap_or_else(|| format!("{main} ({MAIN_TARGET})"));
    match &row.weight {
        Some(w) => format!("→ {target} · weight {}", plain(w)),
        None => format!("→ {target}"),
    }
}

pub fn entries_view(c: &RefEntriesC, _node: &Node, v: &Value, ctx: &Ctx) -> AnyView {
    let all = rows(Some(v));
    let main =
        burrow_admin_proto::value::get(&ctx.doc, &sibling(&ctx.path, ServiceEntry::TARGET.rel()))
            .map(burrow_admin_logic::web::target_value_shown)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| NOT_SET.to_string());
    if all.is_empty() {
        return view! { <span class="text-text-muted">"None"</span> }.into_any();
    }
    let lines = all
        .iter()
        .map(|r| {
            view! {
                <li class="flex flex-wrap items-center gap-2" data-entry-line=r.name.clone()>
                    {ref_link(&c.entity, &r.name, &c.reserved, ctx)}
                    <span class="font-mono text-[13px] text-text-muted">{entry_line(r, &main)}</span>
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
        let main = "127.0.0.1:8080";
        assert_eq!(entry_line(&all[0], main), "→ 127.0.0.1:8080 (main target)");
        assert_eq!(entry_line(&all[1], main), "→ 10.0.0.12:3000 · weight 2");
        assert_eq!(typed(" 3001 "), Some(json!(3001)));
        assert_eq!(typed("10.0.0.12:80"), Some(json!("10.0.0.12:80")));
        assert_eq!(typed(""), None);
    }
}
