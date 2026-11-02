use burrow_admin_proto::codes::{
    AUTH_BOTH, CLIENT_AND_CLIENTS, DUPLICATE_BIND, DUPLICATE_CLIENT, DUPLICATE_VISITOR,
    MISSING_CLIENT, PUBLIC_PORT_SKIPS_LOGIN, UNREACHABLE_SERVICE,
};

pub(crate) fn worded(code: &str, subject: &str) -> Option<String> {
    let text = match code {
        CLIENT_AND_CLIENTS => "Set Clients only; this service also has a single Client".into(),
        MISSING_CLIENT => "Pick at least one client in Where it runs".into(),
        DUPLICATE_CLIENT => {
            "A client is listed twice with the same target; give the second entry another target, or remove it".into()
        }
        DUPLICATE_VISITOR => "A visitor is listed twice; remove the second one".into(),
        UNREACHABLE_SERVICE => {
            "Nothing reaches this service: turn on a public port, a website or visitors under Reached by".into()
        }
        AUTH_BOTH => "Use passwords or a login service on this website, not both".into(),
        DUPLICATE_BIND => format!("{subject} is already used by another service or listener; pick another port"),
        PUBLIC_PORT_SKIPS_LOGIN => {
            "The public port reaches the app without the website's login; turn the public port off, or keep it on a trusted network".into()
        }
        _ => return None,
    };
    Some(text)
}

#[cfg(test)]
pub(crate) const SCHEMA: [&str; 3] = [
    burrow_admin_proto::codes::MISSING_FIELD,
    burrow_admin_proto::codes::OUT_OF_RANGE,
    burrow_admin_proto::codes::WRONG_TYPE,
];

#[cfg(test)]
pub(crate) const WORDED: [&str; 8] = [
    CLIENT_AND_CLIENTS,
    MISSING_CLIENT,
    DUPLICATE_CLIENT,
    DUPLICATE_VISITOR,
    UNREACHABLE_SERVICE,
    AUTH_BOTH,
    DUPLICATE_BIND,
    PUBLIC_PORT_SKIPS_LOGIN,
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    const KEY_LED: &str = "the message is \"<key path>: words\"; shown() drops the key path and the field label stands beside it";
    const CLIENT_FILE: &str =
        "only in client.yaml or visitor.yaml, which the admin UI does not edit";
    const FILE_LOAD: &str = "a file that does not load at all; the admin UI shows it with its line and column, as burrow validate does";
    const PLAIN: &str = "already a plain sentence about this server";
    const IMPACT: &str = "an impact or confirm line the server writes in plain words for Review";
    const TEST_ONLY: &str = "used only by a unit test fixture";
    const OLDER_UI: &str = "a session-only notice that only a burrow admin older than v0.2.0 receives and shows as written";
    const WHY_NOT: &str = "a reason the server gives in a plain sentence with its hint, shown as written where editing or saving is not possible";

    fn server_lines() -> Vec<(&'static str, &'static str)> {
        let impact = [
            "opens-port",
            "stops-websites",
            "websocket-moved",
            "websocket-off",
            "websocket-host",
            "adds-client",
            "closes-port",
            "stops-website",
            "removes-visitors",
            "port-in-use",
            "web-port-shared",
            "adds-service",
            "changes-listen",
            "changes-self-allow",
            "changes-service",
            "disconnects-client",
            "plaintext-toggled",
            "rebinds-service",
            "removes-client",
            "removes-service",
            "removes-visitor",
            "renames",
            "restart-needed",
            "visitor-loses-access",
            "locked-kept",
        ];
        let why_not = [
            "file-not-writable",
            "folder-not-writable",
            "read-only-fs",
            "read-level",
            "file-read-only",
            "older-admin-files",
        ];
        let mut out = Vec::new();
        out.extend(impact.map(|c| (c, IMPACT)));
        out.extend(why_not.map(|c| (c, WHY_NOT)));
        out
    }

    fn allowed() -> Vec<(&'static str, &'static str)> {
        let key_led = [
            "app-path",
            "basic-auth",
            "certificate-cover",
            "certificate-extra",
            "certificate-file",
            "certificate-source",
            "challenge-unused",
            "client-auth",
            "client-auth-crl",
            "client-auth-tunnel",
            "dns-acme-dns",
            "dns-command",
            "dns-provider",
            "dns-resolver",
            "dns-uncovered",
            "error-pages",
            "health-path",
            "health-path-grpc",
            "health-path-host",
            "internal-name",
            "invalid-range",
            "invalid-size",
            "letsencrypt-ca",
            "passthrough-key",
            "plain-password",
            "redirect-from",
            "single-label-host",
            "target-ca",
            "target-unchecked",
            "tls-unreadable",
            "open-range",
            "dns-domains",
            "dns-rfc2136",
            "grpc-reach",
            "duplicate-path",
            "redirect-name",
            "redirect-target",
            "certificate-hosts",
            "certificate-entry",
            "invalid-host",
            "duplicate-host",
            "forward-auth",
            "unknown-login",
            "forward-auth-unused",
            "forward-auth-h2",
        ];
        let client_file = [
            "allow-all",
            "allow-ignored",
            "no-allow-list",
            "invalid-cidr",
            "invalid-socket",
            "invalid-reconnect",
            "unused-option",
            "invalid-address",
            "invalid-forward",
            "no-forwards",
            "range-mismatch",
            "shared-forward",
            "socks-one-port",
            "socks-udp",
            "udp-unix-target",
            "invalid-header",
            "invalid-proxy",
        ];
        let file_load = [
            "syntax",
            "read",
            "invalid",
            "not-a-mapping",
            "unknown-role",
            "wrong-role",
            "world-readable",
            "edit-spare",
            "env-syntax",
            "drop-in-key",
            "drop-in-section",
            "drop-in-ignored",
            "drop-in-unreadable",
            "drop-in-unsafe",
        ];
        let plain = [
            "access-log",
            "duplicate-name",
            "admin-shared",
            "allow-every-address",
            "bind-is-control-port",
            "duplicate-secret",
            "env-unset",
            "insecure-plaintext",
            "invalid-bandwidth",
            "invalid-name",
            "invalid-path",
            "invalid-pool",
            "invalid-reach",
            "invalid-source",
            "letsencrypt-directory",
            "letsencrypt-email",
            "link-local-target",
            "low-port",
            "missing-secret",
            "no-clients",
            "no-listener",
            "no-services",
            "no-web-proxy",
            "plain-ws-443",
            "proxied-source",
            "proxy-tls-check",
            "public-metrics",
            "reach-any",
            "reach-needs-self-allow",
            "reserved-name",
            "sensitive-port",
            "socket-tcp-only",
            "target-ip",
            "target-is-burrow",
            "target-outside-self-allow",
            "tls-file",
            "too-many-secrets",
            "too-many-services",
            "tunnel-certificate",
            "tunnel-host",
            "tunnel-path",
            "unknown-field",
            "unknown-value",
            "unused-admin-from",
            "weak-secret",
            "web-overlap",
            "web-port",
            "wildcard-hint",
            "dns-wildcard",
            "listen-clash",
            "profile-not-offered",
        ];
        let mut out = Vec::new();
        out.extend(key_led.map(|c| (c, KEY_LED)));
        out.extend(client_file.map(|c| (c, CLIENT_FILE)));
        out.extend(file_load.map(|c| (c, FILE_LOAD)));
        out.extend(plain.map(|c| (c, PLAIN)));
        out.extend(server_lines());
        out.extend(["x", "unknown-client"].map(|c| (c, TEST_ONLY)));
        out.push((burrow_admin_proto::codes::OLDER_ADMIN, OLDER_UI));
        out
    }

    fn proto_codes() -> Vec<String> {
        include_str!("../../burrow-admin-proto/src/codes.rs")
            .lines()
            .filter_map(|l| l.split_once("&str = \"")?.1.split('"').next())
            .map(str::to_string)
            .collect()
    }

    fn emitted(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir)
            .expect("the validator's sources")
            .flatten()
        {
            let p = entry.path();
            if p.is_dir() {
                emitted(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for part in text.split("Diagnostic::").skip(1) {
                    let call = part.trim_start_matches(|c: char| c.is_ascii_lowercase());
                    let arg = call.trim_start_matches(['(', ' ', '\n']);
                    if let Some(code) = arg.strip_prefix('"').and_then(|a| a.split('"').next()) {
                        out.insert(code.to_string());
                    }
                }
            }
        }
    }

    #[test]
    fn every_code_the_ui_can_show_has_plain_words_or_a_reason() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
        let mut codes: BTreeSet<String> = proto_codes().into_iter().collect();
        emitted(&root.join("config"), &mut codes);
        emitted(&root.join("admin"), &mut codes);
        assert!(
            codes.len() > 100,
            "the scan found the validator's codes: {codes:?}"
        );
        let allowed = allowed();
        let shown_codes: Vec<&str> = crate::wording::SHOWN_CODES.to_vec();
        let unmapped: Vec<&String> = codes
            .iter()
            .filter(|c| {
                !super::SCHEMA.contains(&c.as_str())
                    && !super::WORDED.contains(&c.as_str())
                    && !shown_codes.contains(&c.as_str())
                    && !allowed.iter().any(|(a, _)| a == c)
            })
            .collect();
        assert!(
            unmapped.is_empty(),
            "codes with no plain words and no reason: {unmapped:?}"
        );
        let stale: Vec<&&str> = allowed
            .iter()
            .map(|(c, _)| c)
            .filter(|c| !codes.contains(**c))
            .collect();
        assert!(stale.is_empty(), "allowed codes nobody emits: {stale:?}");
    }
}
