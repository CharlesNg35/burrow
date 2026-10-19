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
mod review;
mod route;
mod screens;
mod secrets;
mod store;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
