use leptos::prelude::*;

pub fn path(name: &str) -> &'static str {
    match name {
        "overview" => "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
        "stack" => "M4 5h16v5H4zM4 14h16v5H4zM7.5 7.5h.01M7.5 16.5h.01",
        "monitor" => "M3.5 5h17v11h-17zM9 20h6M12 16v4",
        "person" => "M12 12a4 4 0 100-8 4 4 0 000 8zM5 20a7 7 0 0114 0",
        "settings" => "M4 7h10M18 7h2M4 17h4M12 17h8M14 4.5v5M8 14.5v5",
        "activity" | "history" => {
            "M3.5 12a8.5 8.5 0 108.5-8.5A8.5 8.5 0 005.5 6.5M3.5 3.5v3h3M12 8v4l3 2"
        }
        "about" | "info" => "M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v5.5M12 7.8v.01",
        "alert" | "warning" => "M12 3.5l9 16h-18zM12 10v4.5M12 17.2v.01",
        "alert-circle" => "M12 21a9 9 0 100-18 9 9 0 000 18zM12 7.5v5.5M12 16.2v.01",
        "check" => "M5 12.5l4.5 4.5L19 7.5",
        "lock" => "M7 11V8a5 5 0 0110 0v3M5.5 11h13v9.5h-13zM12 15v2",
        "copy" => "M9 9h11v11H9zM5 15H4V4h11v1",
        "download" => "M12 4v11m-5-5l5 5 5-5M5 20h14",
        "server" | "box" => "M12 3l8 4.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9",
        "arrow" => "M5 12h14m-5-5l5 5-5 5",
        "close" | "x" => "M6 6l12 12M18 6L6 18",
        "env" => {
            "M8 5c-2 0-3 1-3 3v2l-2 2 2 2v2c0 2 1 3 3 3M16 5c2 0 3 1 3 3v2l2 2-2 2v2c0 2-1 3-3 3"
        }
        "globe" => {
            "M12 21a9 9 0 100-18 9 9 0 000 18zM3 12h18M12 3c2.5 2.5 3.7 5.5 3.7 9s-1.2 6.5-3.7 9c-2.5-2.5-3.7-5.5-3.7-9S9.5 5.5 12 3z"
        }
        "network" | "flow" => "M10 3h4v4h-4zM3.5 17h4v4h-4zM16.5 17h4v4h-4zM12 7v4M5.5 17v-3h13v3",
        "link" => {
            "M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1"
        }
        "shield" => "M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z",
        "eye" => {
            "M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12zM12 15a3 3 0 100-6 3 3 0 000 6z"
        }
        "passkey" => {
            "M14.5 9.5a4.5 4.5 0 11-9 0 4.5 4.5 0 019 0zM13.2 12.7L20 19.5M17 16.5l2-2M15 14.5l1.5-1.5"
        }
        "terminal" => "M4 5h16v14H4zM7.5 9.5l3 2.5-3 2.5M12.5 15h4",
        "revert" => "M9 14l-5-5 5-5M4 9h10a6 6 0 010 12h-3",
        "diff" => {
            "M6 3v12M6 15a3 3 0 100 6 3 3 0 000-6zM18 9a3 3 0 100-6 3 3 0 000 6zM18 9v3a3 3 0 01-3 3H9"
        }
        "plus" => "M12 5v14M5 12h14",
        "chevron-down" => "M6 9l6 6 6-6",
        "chevron-right" => "M9 6l6 6-6 6",
        "chevron-left" => "M15 6l-6 6 6 6",
        "search" => "M11 18a7 7 0 100-14 7 7 0 000 14zM20 20l-4-4",
        "filter" => "M4 5h16l-6 7.5V19l-4-2v-4.5z",
        "refresh" => "M20 5v5h-5M4 19v-5h5M18.5 9A7 7 0 006 7.5L4 10M5.5 15A7 7 0 0018 16.5l2-2.5",
        "wifi-off" => {
            "M3 3l18 18M8.5 16.5a5 5 0 017 0M5 12.5a10 10 0 015.5-2.8M13.5 9.8A10 10 0 0119 12.5M2 9a15 15 0 015.2-3M12 20h.01"
        }
        "edit" => "M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4",
        "trash" => "M4 7h16M10 11v6M14 11v6M5.5 7l1 13h11l1-13M9 7V4h6v3",
        "external" => "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
        "more" => "M5 12h.01M12 12h.01M19 12h.01",
        "fork" => "M10 3h4v4h-4zM3.5 17h4v4h-4zM16.5 17h4v4h-4zM12 7v4M5.5 17v-3h13v3",
        "sun" => {
            "M12 16a4 4 0 100-8 4 4 0 000 8zM12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"
        }
        "moon" => "M20 14.5A8 8 0 019.5 4a8 8 0 1010.5 10.5z",
        "logo" => "M5 20V12a7 7 0 0114 0v8M9 20v-7a3 3 0 016 0v7",
        _ => "M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v5.5M12 7.8v.01",
    }
}

#[component]
pub fn Icon(
    name: &'static str,
    #[prop(optional)] small: bool,
    #[prop(optional)] large: bool,
) -> impl IntoView {
    let size = if small {
        "size-[13px]"
    } else if large {
        "size-[17px]"
    } else {
        "size-[15px]"
    };
    view! {
        <svg
            class=format!("{size} shrink-0 fill-none stroke-current [stroke-linecap:round] [stroke-linejoin:round] [stroke-width:1.7]")
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
        >
            <path d=path(name)></path>
        </svg>
    }
}
