use burrow_admin_proto::schema::Schema;
use burrow_ui_kit::Tone;
use leptos::context::Provider;
use leptos::prelude::*;

use crate::edit::Edit;
use burrow_admin_logic::settings_tabs::{Tab, tab_of};
use burrow_ui_kit::{PillTabs, TabMark};

use crate::form::{PageSearch, ShowKeys, card_fields, settings_card, settings_cards};
use crate::route::Route;
use crate::screens::{Row, filter_chips, search_box};

use crate::route::TAB_QUERY;
use crate::store::store;

fn show_card(key: &str) {
    if let Ok(Some(el)) =
        document().query_selector(&format!("[data-testid=settings] [data-card=\"{key}\"]"))
    {
        el.scroll_into_view();
    }
}

fn open_asked_card() {
    let Some(want) = crate::route::query(crate::route::CARD_QUERY) else {
        return;
    };
    let s = store();
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        let Some(schema) = s.schema.get() else {
            return false;
        };
        let skip = burrow_admin_logic::tree::entity_sections(&schema);
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        let card = settings_cards(&schema.root, &skip)
            .into_iter()
            .find(|c| c.fields.iter().any(|f| f.key == want));
        if let Some(c) = card {
            request_animation_frame(move || show_card(&c.key));
        }
        true
    });
}

pub fn groups(
    schema: &Schema,
    changed: &dyn Fn(&[String]) -> bool,
    restart: &[String],
) -> Vec<(Tab, Row)> {
    let skip = burrow_admin_logic::tree::entity_sections(schema);
    let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
    let locked = schema.locked_paths();
    settings_cards(&schema.root, &skip)
        .into_iter()
        .map(|c| {
            let inside = card_fields(&c);
            let tops: Vec<String> = c.fields.iter().map(|f| f.key.clone()).collect();
            let n_locked = inside
                .iter()
                .filter(|(p, _)| locked.iter().any(|l| p.starts_with(l)))
                .count();
            let is_changed = tops.iter().any(|k| changed(std::slice::from_ref(k)));
            let needs_restart = restart
                .iter()
                .any(|r| tops.iter().any(|k| r.split('.').next() == Some(k.as_str())));
            let mut filters = Vec::new();
            if needs_restart {
                filters.push("restart");
            }
            if is_changed {
                filters.push("changed");
            }
            let mut tags = Vec::new();
            if is_changed {
                tags.push(("changed".to_string(), Tone::Accent));
            }
            if n_locked > 0 {
                let all = n_locked == inside.len();
                tags.push((
                    if all {
                        "locked".to_string()
                    } else {
                        format!("{n_locked} locked")
                    },
                    Tone::Neutral,
                ));
            }
            if needs_restart {
                tags.push(("restart needed".to_string(), Tone::Info));
            }
            let row = Row {
                key: c.key.clone(),
                to: Route::Settings,
                icon: "settings",
                title: c.title.clone(),
                line2: String::new(),
                tags,
                filters,
                terms: inside
                    .iter()
                    .map(|(p, f)| format!("{} {}", f.node.label, p.join(".")))
                    .collect::<Vec<_>>()
                    .join("\n"),
                ..Row::default()
            };
            (tab_of_card(&c), row)
        })
        .collect()
}

pub fn setting_errors(diags: &[burrow_admin_proto::diag::Diag]) -> usize {
    diags
        .iter()
        .filter(|d| {
            d.is_error()
                && d.path
                    .as_ref()
                    .and_then(|p| p.first())
                    .is_some_and(|k| !crate::route::SECTIONS.contains(&k.as_str()))
        })
        .count()
}

fn tab_of_card(c: &crate::form::SettingsCard) -> Tab {
    c.fields.first().map_or(Tab::Server, |f| tab_of(&f.key))
}

fn asked_tab() -> Tab {
    crate::route::query(TAB_QUERY)
        .and_then(|t| Tab::parse(&t))
        .or_else(|| crate::route::query(crate::route::CARD_QUERY).map(|f| tab_of(&f)))
        .unwrap_or(Tab::Server)
}

#[derive(Clone, Copy)]
struct Look {
    query: RwSignal<String>,
    filter: RwSignal<&'static str>,
}

impl Look {
    fn shows(self, row: &Row) -> bool {
        let f = self.filter.get();
        row.matches(&self.query.get().to_lowercase()) && (f == "all" || row.filters.contains(&f))
    }

    fn narrowed(self) -> bool {
        !self.query.get().is_empty() || self.filter.get() != "all"
    }
}

fn pill_count(rows: &[(Tab, Row)], tab: Tab, look: Look) -> String {
    if !look.narrowed() {
        return String::new();
    }
    rows.iter()
        .filter(|(t, r)| *t == tab && look.shows(r))
        .count()
        .to_string()
}

fn pill_marks(rows: &[(Tab, Row)], tab: Tab) -> Vec<TabMark> {
    let tagged = |word: &str| {
        rows.iter()
            .filter(|(t, r)| *t == tab && r.tags.iter().any(|(x, _)| x.contains(word)))
            .count()
    };
    let changed = tagged("changed");
    let locked = tagged("locked");
    let mut out = Vec::new();
    if changed > 0 {
        out.push(TabMark {
            key: "changed",
            n: changed,
            icon: None,
            dot: Some(Tone::Accent),
            title: format!("{changed} changed"),
        });
    }
    if locked > 0 {
        out.push(TabMark {
            key: "locked",
            n: locked,
            icon: Some("lock"),
            dot: None,
            title: format!("{locked} with locked settings"),
        });
    }
    out
}

fn tab_bar(
    rows: Memo<Vec<(Tab, Row)>>,
    tab: RwSignal<Tab>,
    look: Look,
    search: AnyView,
) -> impl IntoView {
    let keys = Tab::ALL.map(|t| (t.key(), t.label().to_string())).to_vec();
    let counts =
        Signal::derive(move || rows.with(|r| Tab::ALL.map(|t| pill_count(r, t, look)).to_vec()));
    let marks = Signal::derive(move || rows.with(|r| Tab::ALL.map(|t| pill_marks(r, t)).to_vec()));
    view! {
        <div class="mb-3 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-card border border-border bg-surface px-2 py-1.5" data-testid="settings-tabs">
            <div class="min-w-0 max-lg:w-full lg:flex-1">
                <PillTabs
                    tabs=keys
                    active=Signal::derive(move || tab.get().key())
                    counts=counts
                    marks=marks
                    on_change=Callback::new(move |k: &'static str| {
                        if let Some(t) = Tab::parse(k) {
                            tab.set(t);
                            crate::route::set_query(TAB_QUERY, k);
                        }
                    })
                />
            </div>
            <div class="max-lg:w-full lg:w-64">{search}</div>
        </div>
    }
}

fn tab_cards(tab: RwSignal<Tab>, look: Look, rows: Memo<Vec<(Tab, Row)>>) -> impl IntoView {
    let s = store();
    let shown = Memo::new(move |_| {
        let now = tab.get();
        rows.with(|r| {
            r.iter()
                .filter(|(t, row)| *t == now && look.shows(row))
                .map(|(_, row)| row.key.clone())
                .collect::<Vec<_>>()
        })
    });
    let web_on = Memo::new(move |_| {
        s.edit.draft.with(|d| {
            burrow_config::ServerConfig::WEB
                .get(d.draft())
                .is_some_and(|w| !w.is_null())
        })
    });
    move || {
        let schema = s.schema.get()?;
        let skip = burrow_admin_logic::tree::entity_sections(&schema);
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        let now = tab.get();
        let shown = shown.get();
        let web_on = web_on.get();
        let cards = settings_cards(&schema.root, &skip)
            .into_iter()
            .filter(|c| shown.contains(&c.key))
            .collect::<Vec<_>>();
        let empty = cards.is_empty().then(|| {
            view! { <p class="py-6 text-sm text-text-muted" data-testid="settings-none">"No setting on this tab matches."</p> }
        });
        let web_note = (now == Tab::Web && !web_on).then(|| {
            view! { <p class="mb-3 text-sm text-text-muted" data-testid="web-off-note">"Turn the web proxy on to serve websites."</p> }
        });
        Some(view! {
            <div data-tab-panel=now.key()>
                {web_note}
                {cards.iter().map(settings_card).collect_view()}
                {(now == Tab::Web && web_on).then(super::local_ca::local_ca_card)}
                {empty}
            </div>
        })
    }
}

fn follow_matches(rows: Memo<Vec<(Tab, Row)>>, tab: RwSignal<Tab>, look: Look) {
    Effect::new(move |_| {
        if !look.narrowed() {
            return;
        }
        let now = tab.get_untracked();
        let hit = rows.with(|r| {
            let here = r.iter().any(|(t, x)| *t == now && look.shows(x));
            let first = r.iter().find(|(_, x)| look.shows(x)).map(|(t, _)| *t);
            first.filter(|_| !here)
        });
        if let Some(t) = hit {
            tab.set(t);
        }
    });
}

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    let look = Look {
        query: RwSignal::new(String::new()),
        filter: RwSignal::new("all"),
    };
    let tab = RwSignal::new(asked_tab());
    let rows = Memo::new(move |_| {
        let schema = s.schema.get();
        let restart = s
            .state
            .with(|st| st.as_ref().map(|st| st.runtime.restart_pending.clone()))
            .unwrap_or_default();
        let changed = |p: &[String]| s.edit.draft.with(|d| d.modified(p));
        schema
            .map(|sc| groups(&sc, &changed, &restart))
            .unwrap_or_default()
    });
    let plain_rows =
        Signal::derive(move || rows.with(|r| r.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>()));
    open_asked_card();
    Effect::new(move |seen: Option<u64>| {
        let now = s.query_seen.get();
        if seen.is_some_and(|n| n != now) {
            tab.set(asked_tab());
            open_asked_card();
        }
        now
    });
    follow_matches(rows, tab, look);
    let e = s.edit;
    let editing = Memo::new(move |_| e.is_editing(&[]));
    let (changed, problems) = (changed_fields(e), problem_count(e));
    let start = move |_| e.start_edit(&[]);
    let close = move || discard_settings(e);
    let save = move || {
        e.stop_edit(&[]);
        s.added_to_changes();
    };
    let can = s.can_write_now();
    let filters = vec![
        ("all", "All".into()),
        ("restart", "Restart".into()),
        ("changed", "Changed".into()),
    ];
    view! {
        <Provider value=ShowKeys(true)>
        <Provider value=PageSearch(look.query)>
        <Provider value=crate::form::EditMode(Signal::derive(move || editing.get()))>
        <div class="px-8 pt-7 pb-12 max-sm:px-4" id="detail">
            <burrow_ui_kit::PageHeader
                kicker="Settings"
                sub="Everything in server.yaml besides clients, visitors and services. Each field says when it takes effect."
                actions={view! {
                    {move || (!editing.get()).then(|| view! {
                        <burrow_ui_kit::Button label="Edit" icon="edit" testid="edit" disabled=!can reason=if can { String::new() } else { burrow_admin_logic::wording::needs_write_admin(burrow_admin_logic::wording::WriteAction::Edit, None) } on_click=start/>
                    })}
                }.into_any()}
            >
                "Server settings"
            </burrow_ui_kit::PageHeader>
            {tab_bar(rows, tab, look, search_box(look.query, "Search settings".into()).into_any())}
            <div class="mb-4" data-testid="settings-search">
                {filter_chips(filters, look.filter, plain_rows, true)}
            </div>
            <div data-testid="settings">{tab_cards(tab, look, rows)}</div>
            {move || editing.get().then(|| {
                crate::screens::bar::save_bar(crate::screens::bar::SaveBar {
                    count: problems.get(),
                    problems: Vec::new(),
                    changed,
                    blocker: Signal::stored(None),
                    on_discard: Callback::new(move |_| close()),
                    on_save: Callback::new(move |_| save()),
                })
            })}
        </div>
        </Provider>
        </Provider>
        </Provider>
    }
}

fn settings_of(
    doc: &burrow_admin_proto::serde_json::Value,
) -> burrow_admin_proto::serde_json::Value {
    let mut out = burrow_admin_proto::serde_json::Map::new();
    if let Some(m) = doc.as_object() {
        for (k, v) in m {
            if !crate::route::SECTIONS.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    burrow_admin_proto::serde_json::Value::Object(out)
}

fn changed_fields(e: Edit) -> Signal<usize> {
    Signal::derive(move || {
        let now = settings_of(&e.doc());
        e.editing.with(|m| {
            let snap = m
                .get(&Vec::<String>::new())
                .cloned()
                .flatten()
                .map(|d| settings_of(&d))
                .unwrap_or_else(|| now.clone());
            burrow_admin_logic::changeset::ops_between(&snap, &now).len()
        })
    })
}

fn problem_count(e: Edit) -> Signal<usize> {
    Signal::derive(move || {
        let local = e.local.with(|l| {
            l.keys()
                .filter(|p| {
                    p.first()
                        .is_some_and(|k| !crate::route::SECTIONS.contains(&k.as_str()))
                })
                .count()
        });
        let server = e
            .plan
            .with(|p| p.as_ref().map_or(0, |p| setting_errors(&p.diags)));
        local.max(server)
    })
}

fn discard_settings(e: Edit) {
    let snap = e
        .editing
        .with_untracked(|m| m.get(&Vec::<String>::new()).cloned().flatten());
    if let Some(snap) = snap {
        let now = e.doc();
        let mut keys: Vec<String> = Vec::new();
        for d in [&snap, &now] {
            if let Some(m) = d.as_object() {
                keys.extend(
                    m.keys()
                        .filter(|k| !crate::route::SECTIONS.contains(&k.as_str()))
                        .cloned(),
                );
            }
        }
        keys.sort();
        keys.dedup();
        for k in keys {
            let (a, b) = (snap.get(&k).cloned(), now.get(&k).cloned());
            if a != b {
                e.set(std::slice::from_ref(&k), a);
            }
        }
        e.local.update(|l| {
            l.retain(|p, _| {
                p.first()
                    .is_some_and(|k| crate::route::SECTIONS.contains(&k.as_str()))
            })
        });
    }
    e.stop_edit(&[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::diag::{Diag, Severity};

    fn diag(severity: Severity, path: &[&str]) -> Diag {
        Diag {
            severity,
            code: "x".into(),
            message: "m".into(),
            path: Some(path.iter().map(|s| s.to_string()).collect()),
            also: Vec::new(),
            label: None,
            help: None,
            line: None,
            col: None,
            before: false,
            subjects: Vec::new(),
        }
    }

    #[test]
    fn a_server_error_in_a_setting_blocks_adding_the_settings_to_the_changes() {
        let diags = [
            diag(Severity::Error, &["pool", "min_idle"]),
            diag(Severity::Warning, &["pool", "max_idle"]),
            diag(Severity::Error, &["services", "web", "bind"]),
        ];
        assert_eq!(setting_errors(&diags), 1);
        assert_eq!(setting_errors(&diags[1..]), 0);
    }
}
