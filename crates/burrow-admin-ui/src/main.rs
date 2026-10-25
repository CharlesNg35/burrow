//! The burrow admin web UI, a Leptos app compiled to WebAssembly.
//! It talks to burrow admin through burrow-admin-proto and must not use serde_json directly.

#![forbid(unsafe_code)]

mod addr;
mod api;
mod app;
#[cfg(test)]
mod contrast;
mod domain;
mod edit;
mod form;
mod format;
mod grants;
mod review;
mod route;
mod screens;
mod secrets;
mod store;
mod uploads;

fn main() {
    console_error_panic_hook::set_once();
    if let Some(boot) = leptos::prelude::document().get_element_by_id("boot") {
        boot.remove();
    }
    leptos::mount::mount_to_body(app::App);
}
