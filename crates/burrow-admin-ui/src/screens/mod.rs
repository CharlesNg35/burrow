mod about;
pub mod activity;
pub mod create;
mod entity;
mod overview;
mod settings;

use burrow_ui_kit::{EmptyState, button_class};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::store;

pub fn screen(route: Route) -> AnyView {
    let _ = store();
    match route {
        Route::Overview => overview::Overview().into_any(),
        Route::List(section) => view! { <entity::List section=section/> }.into_any(),
        Route::Detail(section, name) => view! { <entity::Detail section=section name=name/> }.into_any(),
        Route::Settings => settings::Settings().into_any(),
        Route::Activity => activity::Activity().into_any(),
        Route::About => about::About().into_any(),
        Route::NotFound(path) => view! {
            <EmptyState title="Page not found" message=format!("Nothing lives at {path}.") testid="not-found">
                <Link to=Route::Overview class=button_class("secondary", false)>"Go to the overview"</Link>
            </EmptyState>
        }
        .into_any(),
    }
}
