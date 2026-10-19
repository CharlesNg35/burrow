use leptos::prelude::*;

fn path(name: &str) -> &'static str {
    match name {
        "alert" => "M12 3l9.5 17h-19L12 3zm0 6v5m0 3v.5",
        "warning" => "M12 3l9.5 17h-19L12 3zm0 6v5m0 3v.5",
        "check" => "M5 12.5l4.5 4.5L19 7.5",
        "lock" => "M7 11V8a5 5 0 0110 0v3M6 11h12v9H6z",
        "copy" => "M9 9h10v11H9zM5 15V4h10",
        "download" => "M12 4v11m-5-5l5 5 5-5M5 20h14",
        "server" => "M4 5h16v6H4zM4 13h16v6H4zM8 8h.01M8 16h.01",
        "overview" => "M4 4h7v7H4zM13 4h7v4h-7zM13 10h7v10h-7zM4 13h7v7H4z",
        "services" => "M4 7h16M4 12h16M4 17h16M7 7v.01M7 12v.01M7 17v.01",
        "clients" => "M4 5h16v10H4zM9 19h6M12 15v4",
        "visitors" => "M12 12a4 4 0 100-8 4 4 0 000 8zM4 20a8 8 0 0116 0",
        "settings" => {
            "M12 15a3 3 0 100-6 3 3 0 000 6zM19 12h2M3 12h2M12 3v2M12 19v2M17 7l1.5-1.5M5.5 18.5L7 17M17 17l1.5 1.5M5.5 5.5L7 7"
        }
        "activity" => "M3 12h4l3-8 4 16 3-8h4",
        "about" => "M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v6M12 7.5v.01",
        "arrow" => "M5 12h14m-5-5l5 5-5 5",
        "close" => "M6 6l12 12M18 6L6 18",
        "env" => {
            "M8 5c-2 0-3 1-3 3v2l-2 2 2 2v2c0 2 1 3 3 3M16 5c2 0 3 1 3 3v2l2 2-2 2v2c0 2-1 3-3 3"
        }
        _ => "M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v6M12 7.5v.01",
    }
}

#[component]
pub fn Icon(name: &'static str) -> impl IntoView {
    view! {
        <svg class="size-4 shrink-0 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d=path(name)></path>
        </svg>
    }
}
