use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Field;
use burrow_admin_proto::serde_json::{Value, json};
use burrow_admin_proto::value::child;
use burrow_config::{Protocol, ServiceEntry};
use burrow_ui_kit::{Tone, field_message};
use leptos::prelude::*;

use crate::store::store;

use super::row::row;
use super::{BIND, PROTOCOL, SERVICE_VISITORS, WEB, editing_now, field_id, plain};

pub const PUBLIC_WHO: &str = "anyone who can reach the server, on its own port";
pub const WEBSITE_WHO: &str = "browsers, by name";
pub const VISITORS_WHO: &str = "your devices with a burrow visitor key";
pub const WEBSITE_NEEDS_TCP: &str = "websites need TCP";
pub const NOT_A_WEBSITE: &str = "off: not a website";

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Public,
    Website,
    Visitors,
}

impl Way {
    fn key(self) -> &'static str {
        match self {
            Way::Public => BIND,
            Way::Website => WEB,
            Way::Visitors => SERVICE_VISITORS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Way::Public => "Public port",
            Way::Website => "Website",
            Way::Visitors => "Visitors",
        }
    }

    fn who(self) -> &'static str {
        match self {
            Way::Public => PUBLIC_WHO,
            Way::Website => WEBSITE_WHO,
            Way::Visitors => VISITORS_WHO,
        }
    }

    fn fresh(self) -> Option<Value> {
        match self {
            Way::Public => None,
            Way::Website => Some(json!({ burrow_config::WebSite::HOSTS.key(): [] })),
            Way::Visitors => None,
        }
    }

    const ALL: [Way; 3] = [Way::Public, Way::Website, Way::Visitors];
}

fn shown(way: Way, v: Option<&Value>) -> String {
    let Some(v) = v.filter(|v| burrow_admin_logic::web::way_on(Some(v))) else {
        return "off".into();
    };
    match way {
        Way::Website => {
            let names = burrow_admin_logic::web::names(v);
            if names.is_empty() {
                "no names yet".into()
            } else {
                names.join(", ")
            }
        }
        Way::Visitors => match v {
            Value::Array(a) => a.iter().map(plain).collect::<Vec<_>>().join(", "),
            other => plain(other),
        },
        Way::Public => plain(v),
    }
}

fn website_summary(v: Option<&Value>, https_on: bool) -> String {
    let names = shown(Way::Website, v);
    let kind = v
        .and_then(|v| burrow_admin_logic::web::site_kind(&json!({ WEB: v }), https_on))
        .map(|k| match k {
            burrow_admin_logic::web::SiteKind::Https => "HTTPS",
            burrow_admin_logic::web::SiteKind::Plain => "plain HTTP",
            burrow_admin_logic::web::SiteKind::Passthrough => "passthrough",
        });
    match kind {
        Some(k) => format!("{names} · {k}"),
        None => names,
    }
}

fn view_line(way: Way, svc: Path) -> impl IntoView {
    let e = store().edit;
    let p = child(&svc, way.key());
    let text = move || shown(way, e.value(&p).as_ref());
    view! {
        <li class="border-b border-rowline py-3 first:pt-0 last:border-b-0" data-way=way.key()>
            <div class="mb-1.5 flex flex-wrap items-baseline gap-x-2">
                <span class="text-sm font-semibold">{way.label()}</span>
                <span class="text-xs text-text-muted">{way.who()}</span>
            </div>
            <div class="font-mono text-[13px] break-words" data-testid="way-value">{text}</div>
        </li>
    }
}

struct Switch {
    way: Way,
    svc: Path,
    on: RwSignal<bool>,
    kept: StoredValue<Option<Value>>,
    blocked: Signal<Option<&'static str>>,
    touched: RwSignal<bool>,
}

impl Switch {
    fn toggle(&self) -> impl IntoView + use<> {
        let e = store().edit;
        let (way, on, kept, blocked, touched) =
            (self.way, self.on, self.kept, self.blocked, self.touched);
        let path = child(&self.svc, way.key());
        let id = format!("{}-reach-{}", field_id(&self.svc), way.key());
        view! {
            <burrow_ui_kit::Toggle
                id=id
                checked=Signal::derive(move || on.get() && blocked.get().is_none())
                label=way.label()
                disabled=Signal::derive(move || blocked.get().is_some())
                on_change=move |v: bool| {
                    touched.set(true);
                    if v {
                        let back = kept.get_value().or_else(|| way.fresh());
                        if back.is_some() {
                            e.set(&path, back);
                        }
                        if way != Way::Website {
                            let path = path.clone();
                            request_animation_frame(move || crate::screens::entity::go_to_field(&path));
                        }
                    } else {
                        kept.set_value(e.peek(&path));
                        e.set(&path, None);
                    }
                    on.set(v);
                }
            />
        }
    }
}

fn website_row(sw: &Switch) -> impl IntoView + use<> {
    let e = store().edit;
    let (on, blocked) = (sw.on, sw.blocked);
    let p = child(&sw.svc, WEB);
    let https_on = Signal::derive(move || {
        burrow_config::ServerConfig::WEB
            .get(&e.doc())
            .and_then(|w| burrow_config::WebConfig::HTTPS.get(w))
            .is_some_and(|h| !h.is_null())
    });
    let open = move |_| {
        if let Ok(Some(el)) =
            document().query_selector(&format!("[data-group={}]", ServiceEntry::GROUP_WEBSITE))
        {
            el.scroll_into_view();
        }
    };
    move || {
        match (blocked.get(), on.get()) {
        (Some(why), _) => view! { <p class="mt-2 text-[13px] text-text-muted sm:ml-[46px]" data-testid="website-blocked">{why}</p> }.into_any(),
        (None, false) => view! { <p class="mt-2 text-[13px] text-text-muted sm:ml-[46px]" data-testid="website-off">{NOT_A_WEBSITE}</p> }.into_any(),
        (None, true) => view! {
            <p class="mt-2 flex flex-wrap items-center gap-3 text-[13px] sm:ml-[46px]" data-testid="website-summary">
                <span class="font-mono">{website_summary(e.value(&p).as_ref(), https_on.get())}</span>
                <button type="button" class="cursor-pointer font-semibold text-accent-ink hover:underline" data-testid="edit-website" on:click=open>"Edit website ↓"</button>
            </p>
        }
        .into_any(),
    }
    }
}

fn way_block(sw: Switch, field: Option<(Field, Path, Path, Path)>) -> impl IntoView {
    let (way, on, blocked) = (sw.way, sw.on, sw.blocked);
    let under = match way {
        Way::Website => website_row(&sw).into_any(),
        _ => {
            let body = field.map(|(f, fp, fs, entity)| {
                if way == Way::Visitors {
                    view! { <leptos::context::Provider value=super::row::QuietLabel>{row(f, fp, fs, entity)}</leptos::context::Provider> }.into_any()
                } else {
                    row(f, fp, fs, entity)
                }
            });
            let shown = move || on.get() && blocked.get().is_none();
            view! { <div class="mt-3 border-l-2 border-rowline pl-4 sm:ml-[17px]" class:hidden=move || !shown()>{body}</div> }.into_any()
        }
    };
    view! {
        <div class="border-b border-rowline py-4 first:pt-0 last:border-b-0" data-way=way.key()>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
                {sw.toggle()}
                <span class="text-xs text-text-muted">{way.who()}{(way == Way::Visitors).then_some(", with burrow visit")}</span>
            </div>
            {under}
        </div>
    }
}

pub(super) fn reached_by(svc: Path, spath: Path, entity: Path, mine: &[&Field]) -> AnyView {
    let e = store().edit;
    let editing = editing_now();
    let can = store().can_write_now();
    let udp = {
        let p = child(&svc, PROTOCOL);
        Memo::new(move |_| {
            e.value(&p).and_then(|v| v.as_str().map(str::to_string))
                == Some(Protocol::Udp.wire().to_string())
        })
    };
    let lines = Way::ALL
        .into_iter()
        .map(|w| view_line(w, svc.clone()))
        .collect_view();
    let touched = RwSignal::new(false);
    let mut blocks = Vec::new();
    for way in Way::ALL {
        let path = child(&svc, way.key());
        let on = RwSignal::new(burrow_admin_logic::web::way_on(e.peek(&path).as_ref()));
        let blocked =
            Signal::derive(move || (way == Way::Website && udp.get()).then_some(WEBSITE_NEEDS_TCP));
        let field = mine.iter().find(|f| f.key == way.key()).map(|f| {
            (
                (*f).clone(),
                child(&svc, &f.key),
                child(&spath, &f.key),
                entity.clone(),
            )
        });
        let sw = Switch {
            way,
            svc: svc.clone(),
            on,
            kept: StoredValue::new(None),
            blocked,
            touched,
        };
        blocks.push(way_block(sw, field).into_any());
    }
    let svc_now = svc.clone();
    let none = Memo::new(move |_| {
        let w = burrow_admin_logic::web::ways(&e.value(&svc_now).unwrap_or_default());
        !(w.public || w.website || w.visitors)
    });
    let no_way = move || {
        none.get().then(|| {
            let tone = if touched.get() { Tone::Danger } else { Tone::Info };
            view! { <div data-testid="no-way-in">{field_message(tone, crate::screens::paths::PICK_VISITORS.to_string())}</div> }
        })
    };
    view! {
        <div class="md:col-span-2" data-testid="reached-by">
            <div class:hidden=move || can && editing.try_get().unwrap_or(false)>
                <ul data-testid="reached-by-view">{lines}</ul>
            </div>
            <div class:hidden=move || !(can && editing.try_get().unwrap_or(false))>{blocks}{no_way}</div>
        </div>
    }
    .into_any()
}
