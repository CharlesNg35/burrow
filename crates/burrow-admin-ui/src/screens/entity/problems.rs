use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::get;
use burrow_config::ServiceEntry;
use burrow_ui_kit::{Callout, Tone};
use leptos::prelude::*;

use crate::screens::paths::Problems;
use crate::screens::placement;
use crate::store::store;

pub(super) fn problems_for(path: &Path) -> Problems {
    let e = store().edit;
    let errs: Vec<(Path, String)> = e
        .related
        .with(|r| {
            r.iter()
                .map(|(p, m)| (p.clone(), m.clone()))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .chain(e.local.with(|l| {
            l.iter()
                .map(|(p, m)| (p.clone(), m.clone()))
                .collect::<Vec<_>>()
        }))
        .chain(e.plan.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.diags
                        .iter()
                        .filter(|d| d.is_error())
                        .filter_map(|d| {
                            d.path.clone().map(|x| {
                                (
                                    x,
                                    burrow_admin_logic::wording::shown(
                                        &d.code,
                                        &d.message,
                                        &d.subjects,
                                    ),
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }))
        .filter(|(p, _)| p.starts_with(path))
        .collect();
    let at = |k: &str| {
        errs.iter()
            .find(|(p, _)| p.get(2).map(String::as_str) == Some(k))
            .map(|(_, m)| m.clone())
    };
    let doc = e.doc();
    let ports = at(ServiceEntry::TARGET.key()).map(|_| {
        let count = |v: Option<&Value>, f: fn(&Value) -> Option<u16>| v.and_then(f).unwrap_or(1);
        let b = count(
            get(
                &doc,
                &[
                    path[0].clone(),
                    path[1].clone(),
                    ServiceEntry::BIND.key().into(),
                ],
            ),
            burrow_admin_logic::relations::bind_ports,
        );
        let t = count(
            get(
                &doc,
                &[
                    path[0].clone(),
                    path[1].clone(),
                    ServiceEntry::TARGET.key().into(),
                ],
            ),
            burrow_admin_logic::relations::target_ports,
        );
        format!("{t} ≠ {b} ports")
    });
    Problems {
        source: at(ServiceEntry::ALLOW_FROM.key())
            .or_else(|| at(ServiceEntry::DENY_FROM.key()))
            .map(|_| "invalid CIDR".into()),
        clients: at(ServiceEntry::HEALTH_CHECK.key()).map(|_| "health".into()),
        ports,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub path: Path,
    pub tab: &'static str,
    pub label: String,
    pub message: String,
}

pub(super) fn problems_of(section: &str, entity: &Path) -> Vec<Problem> {
    let s = store();
    let e = s.edit;
    let mut raw: Vec<(Path, String)> = e.related.with(|r| {
        r.iter()
            .filter(|(p, _)| p.starts_with(entity))
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect()
    });
    raw.extend(e.local.with(|l| {
        l.iter()
            .filter(|(p, _)| p.starts_with(entity))
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect::<Vec<_>>()
    }));
    let field = |p: &Path| p.get(..3).map(<[String]>::to_vec);
    let flagged: Vec<Path> = raw.iter().filter_map(|(p, _)| field(p)).collect();
    raw.extend(e.plan.with(|p| {
        p.as_ref()
            .map(|p| {
                p.diags
                    .iter()
                    .filter(|d| {
                        d.is_error()
                            && d.within(entity)
                            && d.code != burrow_admin_proto::codes::UNREACHABLE_SERVICE
                    })
                    .map(|d| {
                        let at = d
                            .path
                            .iter()
                            .chain(d.also.iter())
                            .filter(|x| x.starts_with(entity))
                            .max_by_key(|x| x.len())
                            .cloned()
                            .unwrap_or_else(|| entity.clone());
                        (
                            at,
                            burrow_admin_logic::wording::shown(&d.code, &d.message, &d.subjects),
                        )
                    })
                    .filter(|(at, _)| field(at).is_none_or(|f| !flagged.contains(&f)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }));
    let mut out: Vec<Problem> = Vec::new();
    for (path, message) in raw {
        if out.iter().any(|p| p.path == path && p.message == message) {
            continue;
        }
        let key = path.get(2).cloned();
        let label = key
            .as_ref()
            .and_then(|k| {
                s.schema.with(|sc| {
                    sc.as_ref().and_then(|sc| {
                        sc.root
                            .at(&[section.to_string(), "*".to_string(), k.clone()])
                            .map(|n| n.label.clone())
                    })
                })
            })
            .unwrap_or_default();
        out.push(Problem {
            tab: key
                .as_deref()
                .map(|k| placement::tab_of(section, k))
                .unwrap_or(placement::SETTINGS),
            path,
            label,
            message,
        });
    }
    out
}

pub(super) fn apply_notes() -> Option<AnyView> {
    let e = store().edit;
    let (refused, warned): (Vec<String>, Vec<String>) = e.plan.with(|p| {
        p.as_ref()
            .map(|p| {
                (
                    p.refusals.iter().map(|r| r.message.clone()).collect(),
                    p.impacts
                        .iter()
                        .filter(|i| i.level != burrow_admin_proto::plan::Level::Info)
                        .map(|i| i.message.clone())
                        .collect(),
                )
            })
            .unwrap_or_default()
    });
    if refused.is_empty() && warned.is_empty() {
        return None;
    }
    let capital = |m: &str| {
        let mut c = m.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect::<String>())
            .unwrap_or_default()
    };
    Some(
        view! {
            <div class="mb-4 space-y-2" data-testid="apply-notes">
                {(!refused.is_empty()).then(|| view! {
                    <Callout tone=Tone::Danger title="Apply would be refused">
                        <ul>{refused.iter().map(|m| view! { <li>{format!("{}.", capital(m))}</li> }).collect_view()}</ul>
                    </Callout>
                })}
                {(!warned.is_empty()).then(|| view! {
                    <Callout tone=Tone::Warning title="When you apply">
                        <ul>{warned.iter().map(|m| view! { <li>{format!("{}.", capital(m))}</li> }).collect_view()}</ul>
                    </Callout>
                })}
            </div>
        }
        .into_any(),
    )
}

pub fn go_to_field(path: &[String]) {
    let entity = path
        .first()
        .is_some_and(|s| crate::route::SECTIONS.contains(&s.as_str()));
    let ids: Vec<String> = (if entity { 3 } else { 1 }..=path.len())
        .rev()
        .map(|n| crate::form::field_id(&path[..n]))
        .collect();
    request_animation_frame(move || {
        let doc = document();
        let Some(el) = ids.iter().find_map(|id| doc.get_element_by_id(id)) else {
            return;
        };
        if crate::form::reveal(&el) {
            request_animation_frame(move || scroll_and_focus(el));
        } else {
            scroll_and_focus(el);
        }
    });
}

fn scroll_and_focus(el: web_sys::Element) {
    use wasm_bindgen::JsCast;
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(web_sys::ScrollLogicalPosition::Center);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
    let target = el
        .query_selector("input:not([type=hidden]), select, textarea, button")
        .ok()
        .flatten()
        .unwrap_or(el);
    if !target.has_attribute("tabindex")
        && !matches!(
            target.tag_name().as_str(),
            "INPUT" | "SELECT" | "TEXTAREA" | "BUTTON"
        )
    {
        let _ = target.set_attribute("tabindex", "-1");
    }
    if let Ok(h) = target.dyn_into::<web_sys::HtmlElement>() {
        let _ = h.focus();
    }
}

pub(super) fn problem_list(
    problems: Vec<Problem>,
    what: &'static str,
    name: String,
    tab: RwSignal<&'static str>,
) -> impl IntoView {
    let n = problems.len();
    let title = format!(
        "{n} problem{} in {what} {name}",
        if n == 1 { "" } else { "s" }
    );
    view! {
        <div class="mb-4" data-testid="form-problems">
            <Callout tone=Tone::Danger title=title>
                <p class="mb-1.5">{if n == 1 { "Fix it to save:" } else { "Fix them to save:" }}</p>
                <ul class="space-y-1">
                    {problems.into_iter().map(|p| {
                        let path = p.path.clone();
                        let to = p.tab;
                        let here = p.label.clone();
                        view! {
                            <li data-problem=p.path.join("/")>
                                {if here.is_empty() {
                                    view! { <span>{p.message.clone()}</span> }.into_any()
                                } else {
                                    view! {
                                        <button type="button" class="cursor-pointer font-semibold underline decoration-dotted underline-offset-2 hover:decoration-solid" on:click=move |_| {
                                            tab.set(to);
                                            go_to_field(&path);
                                        }>{here}</button>
                                        <span>{format!(": {}", p.message)}</span>
                                    }.into_any()
                                }}
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </Callout>
        </div>
    }
}
