use burrow_admin_proto::codes::{WEBSOCKET_HOST, WEBSOCKET_MOVED, WEBSOCKET_OFF};
use burrow_admin_proto::plan::{Impact, Level};
use burrow_config::ServerConfig;

use crate::admin::core::ports::LiveSession;
use crate::config::settings::{ServerSettings, Shares, WebSocketSettings};

fn at(w: &WebSocketSettings) -> String {
    format!("{}{}", w.listen, w.path)
}

fn cut_off(clients: &[String]) -> String {
    if clients.is_empty() {
        "no client is connected over WebSocket now".into()
    } else {
        format!(
            "clients connected over WebSocket now ({}) will be cut off and need their own config changed",
            clients.join(", ")
        )
    }
}

fn warning(code: &str, message: String) -> Impact {
    Impact {
        level: Level::Warning,
        code: code.into(),
        message,
        confirm: Some(ServerConfig::WEBSOCKET.key().into()),
    }
}

pub fn impacts(
    old: &ServerSettings,
    new: &ServerSettings,
    sessions: &[LiveSession],
) -> Vec<Impact> {
    let mut over: Vec<String> = sessions
        .iter()
        .filter(|s| s.over_websocket)
        .map(|s| s.client.clone())
        .collect();
    over.sort();
    over.dedup();
    let Some(was) = &old.websocket else {
        return Vec::new();
    };
    let Some(now) = &new.websocket else {
        return vec![warning(
            WEBSOCKET_OFF,
            format!("turns WebSocket off; {}", cut_off(&over)),
        )];
    };
    let mut out = Vec::new();
    if was.listen != now.listen || was.path != now.path {
        out.push(warning(
            WEBSOCKET_MOVED,
            format!(
                "moves the WebSocket listener from {} to {}; {}",
                at(was),
                at(now),
                cut_off(&over)
            ),
        ));
    }
    if was.host != now.host && was.shares != Shares::Nothing {
        let named = if over.is_empty() {
            "no client uses it now".to_string()
        } else {
            format!(
                "clients that use it ({}) need their own config changed",
                over.join(", ")
            )
        };
        let show = |h: &Option<String>| h.clone().unwrap_or_else(|| "none".into());
        out.push(warning(
            WEBSOCKET_HOST,
            format!(
                "changes the tunnel's name from {} to {}; {named}",
                show(&was.host),
                show(&now.host)
            ),
        ));
    }
    out
}
