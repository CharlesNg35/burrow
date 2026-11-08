pub mod req {
    pub const WELCOME: &str = "welcome";
    pub const SCHEMA: &str = "schema";
    pub const STATE: &str = "state";
    pub const SUBSCRIBE: &str = "subscribe";
    pub const VALIDATE: &str = "validate";
    pub const APPLY: &str = "apply";
    pub const GENERATE_SECRET: &str = "generate_secret";
    pub const CONNECT_INFO: &str = "connect_info";
    pub const SUGGEST: &str = "suggest";
    pub const HISTORY: &str = "history";
    pub const REVERT: &str = "revert";
    pub const PING: &str = "ping";
    pub const UPLOAD: &str = "upload";
    pub const LOCAL_CA: &str = "local_ca";
    pub const HASH_PASSWORD: &str = "hash_password";
    pub const UPDATE_CLIENTS: &str = "update_clients";
    pub const UPDATE_SERVER: &str = "update_server";
    pub const CONFIG_VIEW: &str = "config_view";
    pub const CHECK_TEXT: &str = "check_text";
    pub const CONFIG_TEXT: &str = "config_text";

    pub const ALL: &[&str] = &[
        WELCOME,
        SCHEMA,
        STATE,
        SUBSCRIBE,
        VALIDATE,
        APPLY,
        GENERATE_SECRET,
        CONNECT_INFO,
        SUGGEST,
        HISTORY,
        REVERT,
        PING,
        UPLOAD,
        LOCAL_CA,
        HASH_PASSWORD,
        UPDATE_CLIENTS,
        UPDATE_SERVER,
        CONFIG_VIEW,
        CHECK_TEXT,
        CONFIG_TEXT,
    ];
}

pub mod suggest {
    pub const FREE_PORT: &str = "free-port";
    pub const NAME: &str = "name";
}

pub mod ev {
    pub const STATUS: &str = "status";
    pub const CONFIG: &str = "config";
    pub const REVOKED: &str = "revoked";
    pub const BYE: &str = "bye";
}

pub fn needed_for(request: &str) -> Option<&'static str> {
    use crate::features as f;
    match request {
        req::GENERATE_SECRET => Some(f::SECRET_GENERATE),
        req::CONNECT_INFO => Some(f::CONNECT_SNIPPETS),
        req::HISTORY => Some(f::HISTORY),
        req::REVERT => Some(f::REVERT),
        req::UPLOAD => Some(f::UPLOAD),
        req::HASH_PASSWORD => Some(f::PASSWORDS),
        req::LOCAL_CA => Some(f::LOCAL_CA),
        req::UPDATE_CLIENTS => Some(f::CLIENT_UPDATES),
        req::UPDATE_SERVER => Some(f::SERVER_UPDATES),
        req::CONFIG_VIEW => Some(f::CONFIG_VIEW),
        req::CHECK_TEXT => Some(f::CHECK_TEXT),
        req::CONFIG_TEXT => Some(f::CONFIG_TEXT),
        _ => None,
    }
}
