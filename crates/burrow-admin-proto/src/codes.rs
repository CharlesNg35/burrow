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
pub const DUPLICATE_BIND: &str = "duplicate-bind";
pub const UNREACHABLE_SERVICE: &str = "unreachable-service";
pub const ADDS_CLIENT: &str = "adds-client";

pub const READ_BY_THE_UI: [&str; 16] = [
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
];
