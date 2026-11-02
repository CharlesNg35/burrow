pub const PLAIN_NEEDS_HTTP: &str = "plain-needs-http";
pub const REDIRECT_PASSTHROUGH: &str = "redirect-passthrough";
pub const REDIRECT_PLAIN: &str = "redirect-plain";
pub const REDIRECT_NEEDS_HTTP: &str = "redirect-needs-http";
pub const NO_CERTIFICATE: &str = "no-certificate";
pub const WEB_OFF: &str = "web-off";
pub const NO_CHECK: &str = "no-check";
pub const WILDCARD_NEEDS_DNS: &str = "wildcard-needs-dns";
pub const PLAIN_HTTP: &str = "plain-http";
pub const WEB_TCP_ONLY: &str = "web-tcp-only";
pub const PROXY_PROTOCOL_TCP_ONLY: &str = "proxy-protocol-tcp-only";
pub const MAX_CONNECTIONS_TCP_ONLY: &str = "max-connections-tcp-only";
pub const BALANCE_TCP_ONLY: &str = "balance-tcp-only";
pub const HEALTH_CHECK_TCP_ONLY: &str = "health-check-tcp-only";
pub const WEIGHT_NOT_BALANCE: &str = "weight-not-balance";
pub const DUPLICATE_BIND: &str = "duplicate-bind";
pub const PORT_IN_USE: &str = "port-in-use";
pub const WEB_PORT_SHARED: &str = "web-port-shared";
pub const OPENS_PORT: &str = "opens-port";
pub const STOPS_WEBSITES: &str = "stops-websites";
pub const WEBSOCKET_MOVED: &str = "websocket-moved";
pub const WEBSOCKET_OFF: &str = "websocket-off";
pub const WEBSOCKET_HOST: &str = "websocket-host";
pub const UNREACHABLE_SERVICE: &str = "unreachable-service";
pub const MISSING_FIELD: &str = "missing-field";
pub const CLIENT_AND_CLIENTS: &str = "client-and-clients";
pub const MISSING_CLIENT: &str = "missing-client";
pub const DUPLICATE_CLIENT: &str = "duplicate-client";
pub const DUPLICATE_VISITOR: &str = "duplicate-visitor";
pub const AUTH_BOTH: &str = "auth-both";
pub const OUT_OF_RANGE: &str = "out-of-range";
pub const WRONG_TYPE: &str = "wrong-type";
pub const ADDS_CLIENT: &str = "adds-client";
pub const PUBLIC_PORT_SKIPS_LOGIN: &str = "public-port-skips-login";
pub const NO_HTTP: &str = "no-http";
pub const CLOSES_PORT: &str = "closes-port";
pub const STOPS_WEBSITE: &str = "stops-website";
pub const REMOVES_VISITORS: &str = "removes-visitors";
pub const FORWARD_AUTH: &str = "forward-auth";
pub const FORWARD_AUTH_H2: &str = "forward-auth-h2";
pub const UNKNOWN_LOGIN: &str = "unknown-login";
pub const FORWARD_AUTH_UNUSED: &str = "forward-auth-unused";
pub const UNUSED_VISITOR: &str = "unused-visitor";
pub const PROFILE_NOT_OFFERED: &str = "profile-not-offered";
pub const ENV_UNSET: &str = "env-unset";
pub const WRONG_ROLE: &str = "wrong-role";
pub const NO_FORWARDS: &str = "no-forwards";
pub const SYNTAX: &str = "syntax";
pub const NO_SERVICES: &str = "no-services";
pub const NO_CLIENTS: &str = "no-clients";
pub const RENAMES: &str = "renames";
pub const REMOVES_SERVICE: &str = "removes-service";
pub const REBINDS_SERVICE: &str = "rebinds-service";
pub const CHANGES_SERVICE: &str = "changes-service";
pub const ADDS_SERVICE: &str = "adds-service";
pub const REMOVES_CLIENT: &str = "removes-client";
pub const DISCONNECTS_CLIENT: &str = "disconnects-client";
pub const REMOVES_VISITOR: &str = "removes-visitor";
pub const VISITOR_LOSES_ACCESS: &str = "visitor-loses-access";
pub const CHANGES_SELF_ALLOW: &str = "changes-self-allow";
pub const CHANGES_LISTEN: &str = "changes-listen";
pub const PLAINTEXT_TOGGLED: &str = "plaintext-toggled";
pub const RESTART_NEEDED: &str = "restart-needed";
pub const LISTEN_CLASH: &str = "listen-clash";
pub const LOCKED_KEPT: &str = "locked-kept";
pub const READ_LEVEL: &str = "read-level";
pub const FILE_NOT_WRITABLE: &str = "file-not-writable";
pub const READ_ONLY_FS: &str = "read-only-fs";
pub const FOLDER_NOT_WRITABLE: &str = "folder-not-writable";
pub const OLDER_ADMIN: &str = "older-admin";
pub const TLS_UNREADABLE: &str = "tls-unreadable";
pub const DROP_IN_KEY: &str = "drop-in-key";
pub const DROP_IN_SECTION: &str = "drop-in-section";
pub const DUPLICATE_NAME: &str = "duplicate-name";
pub const DROP_IN_UNSAFE: &str = "drop-in-unsafe";
pub const DROP_IN_UNREADABLE: &str = "drop-in-unreadable";
pub const DROP_IN_IGNORED: &str = "drop-in-ignored";
pub const FILE_READ_ONLY: &str = "file-read-only";
pub const FILE_CHANGED: &str = "file-changed";
pub const OLDER_ADMIN_FILES: &str = "older-admin-files";

pub const READ_BY_THE_UI: [&str; 18] = [
    PLAIN_NEEDS_HTTP,
    REDIRECT_PASSTHROUGH,
    REDIRECT_PLAIN,
    REDIRECT_NEEDS_HTTP,
    NO_CERTIFICATE,
    WEB_OFF,
    NO_CHECK,
    WILDCARD_NEEDS_DNS,
    PLAIN_HTTP,
    WEB_TCP_ONLY,
    PROXY_PROTOCOL_TCP_ONLY,
    MAX_CONNECTIONS_TCP_ONLY,
    BALANCE_TCP_ONLY,
    HEALTH_CHECK_TCP_ONLY,
    DUPLICATE_BIND,
    UNREACHABLE_SERVICE,
    NO_HTTP,
    UNUSED_VISITOR,
];

pub const READ_BY_THE_CLI: [&str; 6] = [
    WRONG_ROLE,
    NO_FORWARDS,
    SYNTAX,
    NO_SERVICES,
    NO_CLIENTS,
    TLS_UNREADABLE,
];
