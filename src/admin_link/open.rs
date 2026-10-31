use crate::base::failure::Failure;
use crate::config::settings::VisitorSettings;
use crate::dialer::Dialer;
use crate::dialer::admin::{AdminOpenError, AdminRefusal, AdminRx, AdminTx};
use burrow_admin_proto::frame::AdminOpen;
use burrow_admin_proto::local::Reason;

pub fn explain(e: &AdminOpenError, visitor: &str, server: &str) -> (Reason, String) {
    match e {
        AdminOpenError::Refused(AdminRefusal::Forbidden) => (
            Reason::Forbidden,
            format!("visitor \"{visitor}\" has no admin rights on {server}"),
        ),
        AdminOpenError::Refused(AdminRefusal::Busy) => (
            Reason::Busy,
            format!(
                "{server} already has the most admin sessions it allows for visitor \"{visitor}\"; close another admin tab"
            ),
        ),
        AdminOpenError::Refused(AdminRefusal::NoAdmin) => (
            Reason::NoAdmin,
            format!("{server} was built without the admin interface"),
        ),
        AdminOpenError::Refused(AdminRefusal::Explained(t)) => (Reason::Refused, t.clone()),
        AdminOpenError::Refused(AdminRefusal::Other(code)) => (
            Reason::Refused,
            format!("{server} refused the admin session (status {code})"),
        ),
        AdminOpenError::Dial(crate::dialer::DialError::Rejected) => (
            Reason::Rejected,
            format!("{server} rejected visitor \"{visitor}\"; check the name and secret"),
        ),
        AdminOpenError::Dial(crate::dialer::DialError::Unreachable(w))
        | AdminOpenError::Dial(crate::dialer::DialError::Lost(w)) => {
            (Reason::Unreachable, w.clone())
        }
        AdminOpenError::Lost(w) => (Reason::Unreachable, w.clone()),
    }
}

pub fn open_message(client: &str) -> AdminOpen {
    AdminOpen {
        min: burrow_admin_proto::PROTOCOL_MIN,
        max: burrow_admin_proto::PROTOCOL_MAX,
        client: format!("{client}/{}", env!("CARGO_PKG_VERSION")),
        features: Vec::new(),
    }
}

pub fn failure(e: &AdminOpenError, settings: &VisitorSettings) -> Failure {
    let (reason, message) = explain(e, &settings.name, &settings.server);
    let f = Failure::new(message);
    match reason {
        Reason::Forbidden => f.hint(format!(
            "add \"admin: write\" (or \"admin: read\") to visitors.{} in server.yaml on the server",
            settings.name
        )),
        Reason::Rejected => f.hint("check name and secret against visitors in the server config"),
        Reason::NoAdmin => f.hint("update burrow on the server"),
        _ => f,
    }
}

pub async fn open(
    dialer: &Dialer,
    settings: &VisitorSettings,
    client: &str,
) -> Result<(AdminRx, AdminTx), Failure> {
    dialer
        .open_admin(&open_message(client))
        .await
        .map(|(rx, tx, _)| (rx, tx))
        .map_err(|e| failure(&e, settings))
}

pub async fn check_access(settings: &VisitorSettings, client: &str) -> Result<(), Failure> {
    let (_, tx) = open(&Dialer::visitor(settings), settings, client).await?;
    tx.close().await;
    Ok(())
}
