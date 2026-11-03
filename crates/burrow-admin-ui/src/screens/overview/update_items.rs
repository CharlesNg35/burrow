use burrow_admin_logic::own_update::{Say, say};
use burrow_admin_logic::update_words::{Trouble, trouble};
use burrow_admin_proto::status::Status;
use burrow_admin_proto::updates::UpdateStatus;
use burrow_config::{ClientEntry, Entry};
use burrow_ui_kit::Tone;

use super::{Extra, Item};
use crate::route::Route;
use crate::store::Retry;

fn failed(u: &UpdateStatus) -> bool {
    say(!u.state.finished(), Some((u.state, u.secs_ago)), false) == Say::Failed
}

fn item(key: String, t: Trouble, to: Option<Route>, retry: Retry) -> Item {
    Item {
        key,
        tone: Tone::Warning,
        title: t.title,
        detail: t.sentence,
        to,
        field: None,
        extra: Some(Extra {
            raw: t.raw,
            retry: Some(retry),
            notes: Some(t.to).filter(|v| !v.is_empty()),
        }),
    }
}

pub(super) fn items(status: &Status) -> Vec<Item> {
    let own = status
        .server_update
        .as_ref()
        .and_then(|o| o.last.as_ref())
        .filter(|u| failed(u))
        .and_then(|u| trouble(u, None))
        .map(|t| item("server-update".into(), t, None, Retry::Server));
    let clients = status.clients.iter().filter_map(|c| {
        let u = c.update.as_ref().filter(|u| failed(u))?;
        let t = trouble(u, Some(&c.name))?;
        Some(item(
            format!("client-update:{}", c.name),
            t,
            Some(Route::Detail(ClientEntry::section().into(), c.name.clone())),
            Retry::Client(c.name.clone()),
        ))
    });
    own.into_iter().chain(clients).collect()
}
