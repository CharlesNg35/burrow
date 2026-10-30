use burrow_admin_proto::codes::{
    BALANCE_TCP_ONLY, HEALTH_CHECK_TCP_ONLY, MAX_CONNECTIONS_TCP_ONLY, MISSING_FIELD,
    NO_CERTIFICATE, NO_CHECK, NO_HTTP, OUT_OF_RANGE, PLAIN_HTTP, PLAIN_NEEDS_HTTP,
    PROXY_PROTOCOL_TCP_ONLY, REDIRECT_NEEDS_HTTP, REDIRECT_PASSTHROUGH, REDIRECT_PLAIN,
    UNUSED_VISITOR, WEB_OFF, WEB_TCP_ONLY, WEIGHT_NOT_BALANCE, WILDCARD_NEEDS_DNS, WRONG_TYPE,
};

const WEB_PROXY: &str = "Settings, Web proxy";

pub fn no_check(subject: &str) -> String {
    format!(
        "Let's Encrypt cannot check {subject}: its TLS and HTTP checks reach only ports 443 and 80; set the HTTPS port to 443 or the HTTP port to 80 in {WEB_PROXY}, use the DNS check (a DNS provider under {WEB_PROXY}, Let's Encrypt), or use your own certificate file"
    )
}

const TCP_ONLY: [(&str, &str); 5] = [
    (
        WEB_TCP_ONLY,
        "a website only works for TCP services; set Protocol to TCP, or remove the website",
    ),
    (
        PROXY_PROTOCOL_TCP_ONLY,
        "PROXY protocol only works for TCP services; set Protocol to TCP, or clear it",
    ),
    (
        MAX_CONNECTIONS_TCP_ONLY,
        "Maximum connections only works for TCP services; set Protocol to TCP, or clear it (for UDP, Maximum UDP senders per port under Settings limits the senders of each port)",
    ),
    (
        BALANCE_TCP_ONLY,
        "Balance only works for TCP services; set Protocol to TCP, or choose Failover",
    ),
    (
        HEALTH_CHECK_TCP_ONLY,
        "Health check only works for TCP services; set Protocol to TCP, or turn it off",
    ),
];

fn key_path(head: &str) -> bool {
    head.contains('.')
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '*'))
}

pub const REPO: &str = "https://github.com/CharlesNg35/burrow";

pub fn releases() -> String {
    format!("{REPO}/releases/latest")
}

pub fn docs(page: &str) -> String {
    format!("{REPO}/blob/main/docs/{page}")
}

pub fn add_label(field: &str, item: &str) -> String {
    if item.is_empty() {
        format!("Add to {field}")
    } else {
        format!("Add {}", item.to_lowercase())
    }
}

pub fn help_shown(code: &str, help: Option<&str>) -> Option<String> {
    if code == UNUSED_VISITOR {
        return Some(
            "Add it to a service's Visitors, or give it direct access to a client.".into(),
        );
    }
    if code == NO_HTTP {
        return Some(format!(
            "Set the HTTP port (80) in {WEB_PROXY} so http:// addresses are sent to https://"
        ));
    }
    help.filter(|h| !speaks_yaml(h)).map(str::to_string)
}

fn speaks_yaml(help: &str) -> bool {
    help.contains(['{', '['])
        || help
            .split_whitespace()
            .any(|w| w.ends_with(':') && key_path(w.trim_end_matches(':')))
}

pub fn field_for(code: &str) -> Option<Vec<String>> {
    use burrow_config::{ServerConfig, WebConfig};
    (code == NO_HTTP).then(|| ServerConfig::WEB.then(WebConfig::HTTP).path())
}

pub const NOT_SET: &str = "not set";

pub fn count_of(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

pub fn used_by_sites(n: usize) -> String {
    format!("Used by {}: ", count_of(n, "site"))
}
pub const SERVER_CHECKS: &str = "The server checks this value when you apply";
pub const FROM_ENV: &str = "from an environment variable";
pub const RUNNING_NOW: &str = "running now";
pub const PROTOCOL_ERROR: &str = "burrow admin and the server don't understand each other here. Update burrow admin or the server; Copy diagnostics has the details.";

pub fn result_words(result: &str) -> &'static str {
    match result {
        "applied" => "applied",
        "applied, some ports could not listen" => "applied, but some ports could not listen",
        "rejected" => "refused by the server; nothing changed",
        "rejected, restored" => "refused by the server; the old file was put back",
        "kept" => "kept in history",
        _ => "no change: the file already matched",
    }
}
pub const UDP_SENDERS: &str = "UDP senders";
pub const CHANGED_LINES: &str = "What changed in server.yaml";

pub fn diff_head(added: usize, removed: usize, hidden: bool) -> String {
    let secrets = if hidden { " · secrets hidden" } else { "" };
    format!("{added} added, {removed} removed{secrets}")
}
pub const WRITTEN_AS: &str = "Saved as";
pub const CONNECT_PLACEHOLDER: &str = "SERVER_ADDRESS";

pub fn rewrite_reason(reason: &str) -> &'static str {
    match reason {
        "a list of mappings" | "a mapping was expected" | "the mapping becomes empty" => {
            "the new value cannot keep the old short form"
        }
        _ => "its layout is one burrow cannot change line by line",
    }
}

pub fn rewritten(place: &str, reason: &str) -> String {
    format!(
        "burrow rewrites {place} in server.yaml in its own layout, because {}. Comments in that part are lost.",
        rewrite_reason(reason)
    )
}
pub const OFFLINE: &str = "offline";
pub const ONLINE: &str = "online";
pub const SESSION_ENDED_HINT: &str = "Run burrow admin again; it opens a new tab.";
pub const VERSION_GONE: &str = "The details of this change are no longer kept.";
pub const TYPE_NAME_FIRST: &str = "Type a name first";
pub const CLIENTS_OFFLINE: &str = "Clients offline";
pub const REFUSED_CONNECTIONS: &str = "Refused connections";
pub const REFUSED_WHY: &str = "wrong key";
pub const REFUSED_HELP: &str = "A client or visitor offered a wrong or unknown key";

pub fn applies_label(how: &burrow_admin_proto::schema::Applies) -> Option<&'static str> {
    use burrow_admin_proto::schema::Applies;
    match how {
        Applies::Live => Some("applies at once"),
        Applies::Reload => Some("applies to new connections"),
        Applies::Restart => Some("needs a restart"),
        Applies::Unknown => None,
    }
}
pub const YOUR_CHANGES: &str = "your changes";
pub const NOT_ADDED: &str = "not added";
pub const IN_CHANGES: &str = "in your changes";
pub const NOT_APPLIED: &str = "Not applied yet";

pub fn version_name(stamp: Option<&str>) -> String {
    match stamp {
        Some(s) => format!("version of {s}"),
        None => "version (time unknown)".into(),
    }
}
pub const MAIN_TARGET: &str = "main target";
pub const OTHER_TARGET: &str = "another target";
pub const USE_MAIN_TARGET: &str = "Use the main target";

pub fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
pub const WEIGHTS_NEED_BALANCE: &str = "Weights only apply with Balance.";

pub fn required(label: &str) -> String {
    format!("{label} is required")
}

pub fn beside_label(label: &str, message: &str) -> String {
    let rest = message
        .strip_prefix(label)
        .filter(|r| r.starts_with(' '))
        .map(|r| r.trim_start());
    match rest {
        Some(r) => r.strip_prefix("is ").unwrap_or(r).to_string(),
        None => message.to_string(),
    }
}

pub fn missing_field<'a>(code: &str, message: &'a str) -> Option<&'a str> {
    if code != MISSING_FIELD {
        return None;
    }
    message
        .strip_prefix("missing field \"")?
        .split('"')
        .next()
        .filter(|k| !k.is_empty())
}

fn quoted_key(message: &str) -> Option<&str> {
    message.strip_prefix('"')?.split('"').next()
}

fn field_name(path: Option<&[String]>, key: &str) -> String {
    path.and_then(burrow_config::label_of)
        .unwrap_or_else(|| capital(&key.replace('_', " ")))
}

fn bounds_text(label: &str) -> String {
    let l = label.trim_start_matches("must be ");
    match l
        .strip_prefix("between ")
        .and_then(|r| r.split_once(" and "))
    {
        Some((a, b)) => format!("{a} to {b}"),
        None => l.to_string(),
    }
}

pub fn diag_text(d: &burrow_admin_proto::diag::Diag) -> String {
    let path = d.path.as_deref();
    if let Some(key) = missing_field(&d.code, &d.message) {
        let mut at = path.map(<[String]>::to_vec).unwrap_or_default();
        at.push(key.to_string());
        return required(&field_name(Some(&at), key));
    }
    match (d.code.as_str(), d.label.as_deref(), quoted_key(&d.message)) {
        (OUT_OF_RANGE, Some(l), Some(key)) => {
            format!("{}: {}", field_name(path, key), bounds_text(l))
        }
        (WRONG_TYPE, Some(l), Some(key)) => format!("{}: {l}", field_name(path, key)),
        _ => crate::diag_words::worded(&d.code, d.subjects.first().map_or("", String::as_str))
            .unwrap_or_else(|| shown(&d.code, &d.message, &d.subjects)),
    }
}

#[cfg(test)]
pub(crate) const SHOWN_CODES: [&str; 17] = [
    WEIGHT_NOT_BALANCE,
    UNUSED_VISITOR,
    NO_HTTP,
    REDIRECT_NEEDS_HTTP,
    PLAIN_NEEDS_HTTP,
    NO_CHECK,
    WEB_OFF,
    NO_CERTIFICATE,
    WILDCARD_NEEDS_DNS,
    PLAIN_HTTP,
    REDIRECT_PASSTHROUGH,
    REDIRECT_PLAIN,
    WEB_TCP_ONLY,
    PROXY_PROTOCOL_TCP_ONLY,
    MAX_CONNECTIONS_TCP_ONLY,
    BALANCE_TCP_ONLY,
    HEALTH_CHECK_TCP_ONLY,
];

pub fn shown(code: &str, message: &str, subjects: &[String]) -> String {
    let subject = subjects.first().map_or("", String::as_str);
    if let Some(key) = missing_field(code, message) {
        return required(key);
    }
    if code == WEIGHT_NOT_BALANCE {
        return "a weight only applies with Balance; clear it or choose Balance".into();
    }
    if code == UNUSED_VISITOR {
        return format!("Visitor {subject} can't reach anything");
    }
    if code == NO_HTTP {
        return "No HTTP port: http:// addresses get no answer".into();
    }
    if code == REDIRECT_NEEDS_HTTP {
        return format!(
            "answering plain HTTP too needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in {WEB_PROXY}, or send http:// to https://"
        );
    }
    if code == PLAIN_NEEDS_HTTP {
        return format!(
            "plain HTTP needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in {WEB_PROXY}, or choose another HTTPS option"
        );
    }
    if code == NO_CHECK {
        return no_check(subject);
    }
    if code == WEB_OFF {
        return format!("the web proxy is off; turn it on with its ports in {WEB_PROXY}");
    }
    if code == NO_CERTIFICATE {
        return format!(
            "{subject} has no certificate: turn on Let's Encrypt under {WEB_PROXY}, choose My own certificate under HTTPS, or list the name in the web proxy's Certificate files"
        );
    }
    if code == WILDCARD_NEEDS_DNS {
        return format!(
            "Let's Encrypt gives {subject} only through a DNS check: add a DNS provider for its domain under {WEB_PROXY}, Let's Encrypt, or use your own certificate file for it"
        );
    }
    if code == PLAIN_HTTP {
        let service = Some(subject)
            .filter(|s| !s.is_empty())
            .unwrap_or("this website");
        return format!(
            "{service} is served as plain HTTP: anyone on the path can read and change its traffic"
        );
    }
    if code == REDIRECT_PASSTHROUGH {
        return "the app does its own HTTPS, so burrow cannot also answer plain HTTP for it; remove this choice in server.yaml (burrow edit)".into();
    }
    if code == REDIRECT_PLAIN {
        return "a plain HTTP website cannot send visitors to https:// in this build; remove this choice in server.yaml (burrow edit)".into();
    }
    if let Some((_, text)) = TCP_ONLY.iter().find(|(c, _)| *c == code) {
        return (*text).into();
    }
    match message.split_once(": ") {
        Some((head, rest)) if key_path(head) => rest.to_string(),
        _ => message.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Add,
    Edit,
}

pub fn needs_write_admin(action: WriteAction, visitor: Option<&str>) -> String {
    let what = match action {
        WriteAction::Add => "Adding",
        WriteAction::Edit => "Editing",
    };
    match visitor.filter(|v| !v.is_empty()) {
        Some(v) => format!("{what} needs write admin. {v} has read."),
        None => format!("{what} needs write admin."),
    }
}

pub const CHANGE_IN_FILE: &str = "Change this in server.yaml (burrow edit)";
pub const CHANGE_IN_FILE_THEN_RESTART: &str =
    "Change this in server.yaml (burrow edit), then restart the server";
pub const TOGGLE_IN_FILE: &str = "Turn it on or off in server.yaml (burrow edit)";
pub const NAMED_IN_FILE: &str =
    "named in server.yaml (burrow edit); remove it there to upload one here";

#[cfg(test)]
mod tests;
