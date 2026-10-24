use burrow_admin_logic::fallback::needs_newer_ui;
use burrow_admin_logic::relations::field_state;
use burrow_admin_logic::tree::inner;
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::schema::{Field, Node, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::as_env;
use burrow_ui_kit::{FieldRow, FieldState, Toggle, Tone};
use leptos::prelude::*;

use crate::domain::{self, FieldCtx};
use crate::edit::diags_at;
use crate::store::store;

use super::controls::{locked_box, reset_button};
use super::{
    ShowKeys, UNREACHABLE, applies_tag, ctx_now, editing_now, field_id, is_wide, join, locked,
    locked_hint, object_view, value_view,
};

pub(super) fn row(f: Field, path: Path, spath: Path, entity: Path) -> AnyView {
    let s = store();
    let e = s.edit;
    let Some(schema) = s.schema.get_untracked() else {
        return ().into_any();
    };
    let node = f.node.clone();
    let st = {
        let node = node.clone();
        let entity = entity.clone();
        let schema = schema.clone();
        Memo::new(move |_| {
            e.draft
                .with(|d| field_state(&node, &schema.root, d.draft(), &entity))
        })
    };
    let lock = schema.locked_paths().iter().any(|l| spath.starts_with(l));
    let lock_here = schema.locked_paths().contains(&spath);
    let hint = locked_hint(&node);
    let can = s.can_write_now() && node.editable.is_yes() && !lock;
    let editing = editing_now();
    let show_keys = use_context::<ShowKeys>().map(|k| k.0).unwrap_or(false);
    let n = inner(&node).clone();
    let parts = Parts {
        node: node.clone(),
        path: path.clone(),
        spath: spath.clone(),
        entity: entity.clone(),
        st,
        lock,
        can,
        editing,
    };
    if let SemType::Object(_) = n.sem
        && !needs_newer_ui(&n)
    {
        return object_row(&parts, n, lock_here, hint);
    }
    let mut badges: Vec<(String, Tone)> = Vec::new();
    let mut aside: Vec<(String, Tone)> = Vec::new();
    if !lock {
        if show_keys {
            aside.extend(applies_tag(&node));
        } else {
            badges.extend(applies_tag(&node));
        }
    }
    if e.peek(&path).as_ref().and_then(as_env).is_some() {
        badges.push(("from environment".into(), Tone::Info));
    }
    let touched = RwSignal::new(false);
    let id = field_id(&path);
    let msg_id = format!("{id}-msg");
    let items = matches!(
        inner(&node).sem,
        SemType::List(_) | SemType::RefList(_) | SemType::Map(_) | SemType::RefMap(_)
    );
    let state = field_state_of(&parts, touched, items);
    let body = field_body(&parts, &id, &msg_id, touched);
    let label = node.label.clone();
    let help = if lock_here {
        format!("{}.", locked_hint(&node))
    } else {
        node.help.clone()
    };
    let key_hint = if show_keys {
        spath.join(".\u{200b}")
    } else if matches!(inner(&node).sem, SemType::List(_)) {
        f.key.clone()
    } else {
        String::new()
    };
    let wide = node.wide.unwrap_or_else(|| is_wide(&node));
    let saved_line = matches!(
        inner(&node).sem,
        SemType::IpPort(_) | SemType::PortRange(_) | SemType::Target(_)
    );
    let visible = Memo::new(move |_| st.with(|s| s.visible));
    let (jp, js) = (join(&path), join(&spath));
    (move || {
        visible.get().then(|| {
            let body = body.clone();
            view! {
                <FieldRow
                    id=id.clone()
                    label=label.clone()
                    help=help.clone()
                    hide_help=Signal::derive(move || saved_line && !lock && editing.try_get().unwrap_or(false))
                    path=jp.clone()
                    schema_path=js.clone()
                    key_hint=key_hint.clone()
                    badges=badges.clone()
                    aside=aside.clone()
                    table=show_keys
                    wide=wide
                    state=state
                >
                    {body}
                </FieldRow>
            }
        })
    })
    .into_any()
}

#[derive(Clone)]
struct Parts {
    node: Node,
    path: Path,
    spath: Path,
    entity: Path,
    st: Memo<burrow_admin_logic::relations::FieldState>,
    lock: bool,
    can: bool,
    editing: Signal<bool>,
}

fn object_row(p: &Parts, n: Node, lock_here: bool, hint: String) -> AnyView {
    let e = store().edit;
    let Parts {
        node,
        path,
        spath,
        entity,
        st,
        can,
        editing,
        ..
    } = p.clone();
    let optional = matches!(node.sem, SemType::Optional(_));
    let label = node.label.clone();
    let help = node.help.clone();
    let p2 = path.clone();
    let is_set = Memo::new(move |_| e.value(&p2).is_some_and(|v| !v.is_null()));
    let (p3, tid) = (path.clone(), field_id(&path));
    let toggle = move || {
        (optional && can && editing.try_get().unwrap_or(false)).then(|| {
            let p3 = p3.clone();
            let id = tid.clone();
            view! {
                <Toggle
                    id=id
                    checked=is_set
                    label=Signal::derive(move || if is_set.get() { "In use".to_string() } else { "Not set".to_string() })
                    on_change=move |on: bool| {
                        e.set(&p3, on.then(|| Value::Object(Default::default())));
                    }
                />
            }
        })
    };
    let none_label = node.none_label.clone().unwrap_or_else(|| "Not set".into());
    let (p4, sp4, en4) = (path.clone(), spath.clone(), entity.clone());
    let body = move || {
        if !optional || is_set.get() {
            object_view(&n, p4.clone(), sp4.clone(), en4.clone())
        } else {
            view! { <p class="text-sm text-text-muted" data-testid="not-set">{none_label.clone()}</p> }
                .into_any()
        }
    };
    let visible = Memo::new(move |_| st.with(|s| s.visible));
    let (jp, js) = (join(&path), join(&spath));
    (move || {
        visible.get().then(|| {
            let (toggle, body) = (toggle.clone(), body.clone());
            view! {
                <fieldset class="rounded-[10px] border border-border bg-surface px-3.5 py-3 md:col-span-2 dark:bg-surface-2" data-path=jp.clone() data-schema=js.clone()>
                    <legend class="px-1 text-sm font-semibold">{label.clone()}</legend>
                    <div class="mb-3 flex flex-wrap items-center gap-3">
                        <p class="flex-1 text-xs text-text-muted">{help.clone()}</p>
                        {lock_here.then(|| view! {
                            <p class="flex items-center gap-1.5 text-xs text-text-muted" data-testid="locked-hint">
                                <burrow_ui_kit::Icon name="lock" small=true/>
                                {hint.clone()}
                            </p>
                        })}
                        {toggle}
                    </div>
                    {body}
                </fieldset>
            }
        })
    })
    .into_any()
}

fn field_state_of(p: &Parts, touched: RwSignal<bool>, items: bool) -> Signal<FieldState> {
    let e = store().edit;
    let Parts { st, lock, .. } = p.clone();
    let (path, node) = (&p.path, &p.node);
    let path = path.clone();
    let node = node.clone();
    Signal::derive(move || {
        let s = st.get();
        let picked = touched.get();
        let server = e.plan.with(|p| {
            diags_at(p.as_deref(), &path, items)
                .into_iter()
                .filter(|d| picked || d.code != UNREACHABLE)
                .map(|d| {
                    (
                        d.severity,
                        burrow_admin_logic::wording::shown(&d.code, &d.message),
                    )
                })
                .collect::<Vec<_>>()
        });
        let server_error = server
            .iter()
            .find(|(sev, _)| *sev == Severity::Error)
            .map(|(_, m)| m.clone());
        let server_warning = server
            .iter()
            .find(|(sev, _)| *sev == Severity::Warning)
            .map(|(_, m)| m.clone());
        let related = e.related.with(|r| r.get(&path).cloned());
        let local = touched
            .get()
            .then(|| e.local.with(|l| l.get(&path).cloned()))
            .flatten()
            .or(related);
        FieldState {
            error: local.or(server_error),
            warning: server_warning.or(s.warnings.first().cloned()),
            info: None,
            disabled_reason: (!s.enabled).then(|| s.disabled_reason.clone()).flatten(),
            locked_reason: if lock {
                Some(locked_hint(&node))
            } else {
                locked(&node)
            },
            modified: e.draft.with(|d| d.modified(&path)),
        }
    })
}

fn field_body(
    p: &Parts,
    id: &str,
    msg_id: &str,
    touched: RwSignal<bool>,
) -> impl Fn() -> AnyView + Clone + use<> {
    let e = store().edit;
    let Parts {
        st,
        lock,
        can,
        editing,
        ..
    } = p.clone();
    let (path, spath, entity, node) = (&p.path, &p.spath, &p.entity, &p.node);
    let (id, msg_id) = (id.to_string(), msg_id.to_string());
    let (path, spath, entity, node) = (path.clone(), spath.clone(), entity.clone(), node.clone());
    let id = id.clone();
    let msg_id = msg_id.clone();
    move || {
        let enabled = st.with(|s| s.enabled);
        if lock && editing.try_get().unwrap_or(false) {
            return locked_box(node.clone(), path.clone(), id.clone());
        }
        if !lock && can && enabled && editing.try_get().unwrap_or(false) {
            let fc = FieldCtx {
                node: node.clone(),
                path: path.clone(),
                spath: spath.clone(),
                entity: entity.clone(),
                id: id.clone(),
                msg_id: msg_id.clone(),
                touched,
                inherited: st.with_untracked(|s| s.default_from.clone()),
            };
            view! {
                <div class="flex flex-wrap items-start gap-2" data-editor="">
                    <div class="min-w-0 flex-1">{domain::editor(fc)}</div>
                    {reset_button(path.clone())}
                </div>
                {crate::grants::refused_line(path.clone())}
            }
            .into_any()
        } else {
            let node = node.clone();
            let path = path.clone();
            let single = crate::addr::merged_partner(&spath).map(|s| {
                let mut p = path.clone();
                p.pop();
                p.push(s);
                p
            });
            let inherited = st.with_untracked(|s| s.default_from.clone());
            let clear = {
                let path = path.clone();
                move || {
                    let reason =
                        st.with(|s| (!s.enabled).then(|| s.disabled_reason.clone()).flatten())?;
                    (can && editing.try_get().unwrap_or(false) && e.value(&path).is_some()).then(|| {
                        let path = path.clone();
                        let label = format!("Clear ({})", reason.replace("do not support", "do not use"));
                        view! {
                            <button type="button" class=burrow_ui_kit::button_class("ghost", true) data-testid="clear-unused" on:click=move |_| e.set(&path, None)>{label}</button>
                        }
                    })
                }
            };
            view! {
                <div class="flex min-h-[38px] flex-wrap items-center gap-3 text-[13px]" id=id.clone()>
                    {move || {
                        let v = e.value(&path).or_else(|| {
                            single
                                .as_ref()
                                .and_then(|s| e.value(s))
                                .map(|c| Value::Array(vec![c]))
                        });
                        match ctx_now() {
                            Some(c) => value_view(&node, v.as_ref(), inherited.as_ref(), &c),
                            None => ().into_any(),
                        }
                    }}
                    {clear}
                </div>
            }
            .into_any()
        }
    }
}
