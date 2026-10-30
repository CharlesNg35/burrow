use burrow_admin_logic::wording;
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::Diag;
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
                            d.path
                                .clone()
                                .map(|x| (x, burrow_admin_logic::wording::diag_text(d)))
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

fn field(p: &Path) -> Option<Path> {
    p.get(..3).map(<[String]>::to_vec)
}

fn server_problem(entity: &Path, d: &Diag, label: &dyn Fn(&Path) -> String) -> (Path, String) {
    let mut at = d
        .path
        .iter()
        .chain(d.also.iter())
        .filter(|x| x.starts_with(entity))
        .max_by_key(|x| x.len())
        .cloned()
        .unwrap_or_else(|| entity.clone());
    if let Some(key) = wording::missing_field(&d.code, &d.message) {
        at.push(key.to_string());
        let message = wording::required(&label(&at));
        return (at, message);
    }
    let message = wording::diag_text(d);
    (at, message)
}

fn merged(mut own: Vec<(Path, String)>, server: Vec<(Path, String)>) -> Vec<(Path, String)> {
    let flagged: Vec<Path> = own.iter().filter_map(|(p, _)| field(p)).collect();
    own.extend(
        server
            .into_iter()
            .filter(|(at, _)| field(at).is_none_or(|f| !flagged.contains(&f))),
    );
    let mut out: Vec<(Path, String)> = Vec::new();
    for item in own {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

pub(super) fn problems_of(section: &str, entity: &Path) -> Vec<Problem> {
    let s = store();
    let e = s.edit;
    let label = |path: &Path| {
        let mut at = vec![section.to_string(), "*".to_string()];
        at.extend(path.iter().skip(2).cloned());
        s.schema
            .with(|sc| {
                sc.as_ref()
                    .and_then(|sc| sc.root.at(&at).map(|n| n.label.clone()))
            })
            .filter(|l| !l.is_empty())
            .or_else(|| path.last().cloned())
            .unwrap_or_default()
    };
    let mut own: Vec<(Path, String)> = e.related.with(|r| {
        r.iter()
            .filter(|(p, _)| p.starts_with(entity))
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect()
    });
    own.extend(e.local.with(|l| {
        l.iter()
            .filter(|(p, _)| p.starts_with(entity))
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect::<Vec<_>>()
    }));
    let server = e.plan.with(|p| {
        p.as_ref()
            .map(|p| {
                p.diags
                    .iter()
                    .filter(|d| {
                        d.is_error()
                            && d.within(entity)
                            && d.code != burrow_admin_proto::codes::UNREACHABLE_SERVICE
                    })
                    .map(|d| server_problem(entity, d, &label))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    merged(own, server)
        .into_iter()
        .map(|(path, message)| {
            let key = path.get(2).cloned();
            Problem {
                tab: key
                    .as_deref()
                    .map(|k| placement::tab_of(section, k))
                    .unwrap_or(placement::SETTINGS),
                label: field(&path).map(|f| label(&f)).unwrap_or_default(),
                path,
                message,
            }
        })
        .collect()
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
    let capital = wording::capital;
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
                                        <span>{format!(": {}", wording::beside_label(&p.label, &p.message))}</span>
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

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::diag::Severity;

    fn p(x: &[&str]) -> Path {
        x.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_server_problem_on_a_field_the_form_flags_shows_once_in_the_field_wording() {
        let entity = p(&["services", "pool"]);
        let d = Diag {
            severity: Severity::Error,
            code: burrow_admin_proto::codes::MISSING_FIELD.into(),
            message: "missing field \"target\" in service \"pool\"".into(),
            path: Some(entity.clone()),
            also: Vec::new(),
            label: None,
            help: None,
            line: None,
            col: None,
            before: false,
            subjects: Vec::new(),
        };
        let label = |_: &Path| "Main target".to_string();
        let server = server_problem(&entity, &d, &label);
        assert_eq!(
            server,
            (
                p(&["services", "pool", "target"]),
                "Main target is required".into()
            )
        );
        let own = vec![(
            p(&["services", "pool", "target"]),
            "Main target is required".to_string(),
        )];
        assert_eq!(merged(own.clone(), vec![server.clone()]), own);
        let other = (p(&["services", "pool", "bind"]), "taken".to_string());
        assert_eq!(
            merged(own.clone(), vec![server, other.clone()]),
            vec![own[0].clone(), other]
        );
        assert_eq!(merged(Vec::new(), vec![own[0].clone()]), own);
    }
}
