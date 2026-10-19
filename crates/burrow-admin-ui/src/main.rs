#![forbid(unsafe_code)]

mod api;
mod app;
mod form;
mod format;
mod route;
mod screens;
mod store;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
