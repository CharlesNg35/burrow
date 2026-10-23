use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use super::super::settings::{ClientSettings, Limit};
use super::connection::{Connection, Dial, check_connection};
use super::server::insecure_warning;
use super::{Spans, name_error, secret_error, tuning, unix, valid_name};
use crate::allowlist::{AllowList, Rule};
use crate::diagnostics::Diagnostic;
use crate::secret::Secret;
use burrow_config::{ClientConfig, OnRejected};
use burrow_syntax::endpoint::Endpoint;

pub fn check_client(
    cfg: &ClientConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ClientSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let Connection {
        endpoint,
        headers,
        ca_pem,
        ca_file,
        proxy,
    } = check_connection(
        Dial {
            server: &cfg.server,
            headers: &cfg.headers,
            ca: cfg.ca.as_deref(),
            proxy: cfg.proxy.as_deref(),
        },
        spans,
        base_dir,
        out,
    );
    let websocket = endpoint.as_ref().is_some_and(Endpoint::is_websocket);
    if cfg.insecure_plaintext && websocket {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "insecure_plaintext does not apply over WebSocket: forwarded data stays encrypted",
            )
            .at(spans.value(&["insecure_plaintext"]))
            .help(
                "it must still match the server setting; remove it on both sides to silence this",
            ),
        );
    }
    if !valid_name(&cfg.name) {
        out.push(name_error("client", &cfg.name, spans.value(&["name"])));
    }
    let secret = match Secret::parse(&cfg.secret) {
        Ok(s) => Some(s),
        Err(e) => {
            out.push(secret_error(
                &format!("client \"{}\"", cfg.name),
                &e,
                spans.value(&["secret"]),
            ));
            None
        }
    };

    let mut rules = Vec::new();
    for (i, entry) in cfg.allow.iter().enumerate() {
        if let Some(r) = parse_rule(entry, "allow", spans.value(&["allow", &i.to_string()]), out) {
            rules.push(r);
        }
    }
    if cfg.allow_all {
        out.push(
            Diagnostic::warning(
                "allow-all",
                "allow_all is enabled: the server can make this client connect anywhere, Unix sockets included",
            )
            .at(spans.value(&["allow_all"]))
            .label("no restriction on targets")
            .help("list only the networks you expose, for example allow: [127.0.0.0/8]"),
        );
        if !cfg.allow.is_empty() {
            out.push(
                Diagnostic::warning(
                    "allow-ignored",
                    "\"allow\" is ignored because allow_all is true",
                )
                .at(spans.key(&["allow"]))
                .help("remove allow_all to enforce the list"),
            );
        }
    } else if cfg.allow.is_empty() {
        let d = Diagnostic::warning(
            "no-allow-list",
            "no allow list: this client will refuse every target",
        )
        .help("add the networks the server may reach, for example: allow: [127.0.0.0/8]");
        out.push(if spans.has(&["allow"]) {
            d.at(spans.key(&["allow"]))
        } else {
            d
        });
    }

    if cfg.insecure_plaintext && !websocket {
        out.push(insecure_warning(spans));
    }

    let initial = Duration::from_millis(cfg.reconnect.initial_delay_ms.into());
    let max = Duration::from_secs(cfg.reconnect.max_delay_secs.into());
    if initial > max {
        out.push(
            Diagnostic::error(
                "invalid-reconnect",
                "reconnect.initial_delay_ms is larger than reconnect.max_delay_secs",
            )
            .at(spans.value(&["reconnect", "initial_delay_ms"]))
            .help("make the initial delay smaller than the maximum delay"),
        );
    }

    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    let endpoint = endpoint?;
    Some(ClientSettings {
        server: endpoint.to_string(),
        endpoint,
        headers,
        ca_pem,
        ca_file,
        name: cfg.name.clone(),
        secret: secret?,
        allow: if cfg.allow_all {
            AllowList::All
        } else {
            AllowList::Rules(rules)
        },
        insecure_plaintext: cfg.insecure_plaintext,
        cipher: cfg.cipher,
        proxy,
        reconnect_initial: initial,
        reconnect_max: max,
        give_up_after: (cfg.reconnect.give_up_after_secs > 0)
            .then(|| Duration::from_secs(cfg.reconnect.give_up_after_secs.into())),
        exit_on_rejected: cfg.reconnect.on_rejected == OnRejected::Exit,
        reject_confirm: Duration::from_secs(15),
        connect_timeout: Duration::from_secs(cfg.connect_timeout_secs.into()),
        tcp: tuning(&cfg.tcp),
        udp_session_timeout: Duration::from_secs(cfg.udp.session_timeout_secs.into()),
        max_connections: Limit {
            value: cfg.max_connections,
            written: spans.has(&["max_connections"]),
        },
        max_udp_peers: Limit {
            value: cfg.max_udp_peers,
            written: spans.has(&["max_udp_peers"]),
        },
    })
}

pub(super) fn parse_rule(
    entry: &str,
    field: &str,
    at: crate::config::tree::Span,
    out: &mut Vec<Diagnostic>,
) -> Option<Rule> {
    let e = match entry.parse::<Rule>() {
        Ok(r) => return Some(r),
        Err(e) => e,
    };
    out.push(if entry.trim().starts_with(unix::PREFIX) {
        Diagnostic::error(
            "invalid-socket",
            format!("invalid {field} entry: {}", e.describe()),
        )
        .at(at)
        .label("not a valid socket rule")
        .help("use unix:/absolute/path.sock for one socket, or unix:/folder/* for the sockets in a folder")
    } else {
        let help = if entry.parse::<IpAddr>().is_err()
            && !entry.contains('/')
            && entry.chars().any(|c| c.is_ascii_alphabetic())
            && !entry.contains("::")
        {
            format!("{field} entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32")
        } else {
            "use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)".to_string()
        };
        Diagnostic::error(
            "invalid-cidr",
            format!("invalid {field} entry: {}", e.describe()),
        )
        .at(at)
        .label("not a valid CIDR")
        .help(help)
    });
    None
}
