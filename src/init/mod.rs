mod client;
mod export;
mod plan;
mod server;
mod splice;
mod visitor;
mod web;

pub use client::{client_config, client_document, client_value, init_client};
pub use export::ConfigOutput;
pub use server::{init_server, init_server_asking, server_value};
pub use visitor::{init_visitor, reaches_something, visitor_config, visitor_document};
pub use web::WebInit;

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::base::failure::Failure;
use crate::base::prompt::Prompter;
use crate::config::check::valid_name;
use crate::config::{Format, Role};
use crate::wire::secret::Secret;
use burrow_syntax::endpoint::Endpoint;
use burrow_syntax::ports::{FIRST_SERVICE_PORT, SERVER_PORT, WS_PORT, WSS_PORT};

pub const DEFAULT_CLIENT_NAME: &str = "home";
pub fn default_listen() -> String {
    format!("0.0.0.0:{SERVER_PORT}")
}

pub fn default_service_bind() -> String {
    format!("0.0.0.0:{FIRST_SERVICE_PORT}")
}

#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub format: Option<Format>,
    pub output: Option<PathBuf>,
    pub force: bool,
    pub server: Option<String>,
    pub name: Option<String>,
    pub client_name: Option<String>,
    pub secret: Option<String>,
    pub env_secret: Option<String>,
    pub websocket: Option<WebSocketInit>,
    pub email: Option<String>,
    pub web: Option<WebInit>,
    pub update: bool,
    pub yes: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WebSocketInit {
    pub listen: Option<String>,
    pub path: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

pub fn default_ws_listen() -> String {
    format!("127.0.0.1:{WS_PORT}")
}

pub fn default_ws_tls_listen() -> String {
    format!("0.0.0.0:{WSS_PORT}")
}

struct WebSocketChoice {
    listen: String,
    path: String,
    tls: Option<(String, String)>,
}

fn absolute(file: &str) -> Result<String, Failure> {
    std::path::absolute(file)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| Failure::usage(format!("invalid path \"{file}\": {e}")))
}

impl WebSocketInit {
    fn resolve(&self) -> Result<WebSocketChoice, Failure> {
        let tls = match (&self.tls_cert, &self.tls_key) {
            (Some(c), Some(k)) => Some((absolute(c)?, absolute(k)?)),
            (None, None) => None,
            _ => {
                return Err(Failure::usage("--ws-tls-cert and --ws-tls-key go together")
                    .hint("pass both files, or neither to serve ws:// behind a TLS proxy"));
            }
        };
        let listen = self.listen.clone().unwrap_or_else(|| {
            if tls.is_some() {
                default_ws_tls_listen()
            } else {
                default_ws_listen()
            }
        });
        listen.parse::<std::net::SocketAddr>().map_err(|_| {
            Failure::usage(format!("invalid --ws-listen \"{listen}\""))
                .hint(format!("use ip:port, for example {}", default_ws_listen()))
        })?;
        let path = self
            .path
            .clone()
            .unwrap_or_else(burrow_config::default_ws_path);
        if !burrow_syntax::endpoint::valid_path(&path) {
            return Err(Failure::usage(format!("invalid --ws-path \"{path}\""))
                .hint("use a path like /burrow"));
        }
        Ok(WebSocketChoice { listen, path, tls })
    }
}

impl WebSocketChoice {
    fn shape(&self) -> burrow_syntax::address::WsShape<'_> {
        burrow_syntax::address::WsShape {
            listen: self.listen.parse().expect("checked in resolve"),
            path: &self.path,
            tls: self.tls.is_some(),
        }
    }
}

fn resolve_output(role: Role, opts: &InitOptions) -> Result<(PathBuf, Format), Failure> {
    let format = opts
        .format
        .or_else(|| opts.output.as_deref().and_then(Format::from_path))
        .unwrap_or(Format::Yaml);
    let ext = match format {
        Format::Yaml => "yaml",
        Format::Json => "json",
    };
    let path = opts
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}.{ext}", role.as_str())));
    match Format::from_path(&path) {
        Some(f) if f != format => Err(Failure::usage(format!(
            "{} does not match --format {ext}",
            path.display()
        ))
        .hint("use a matching extension or change --format")),
        Some(_) => Ok((path, format)),
        None => Err(Failure::usage(format!(
            "{} has no .yaml, .yml or .json extension",
            path.display()
        ))
        .hint("burrow picks the config format from the extension")),
    }
}

fn check_name(flag: &str, name: &str) -> Result<(), Failure> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(Failure::usage(format!("invalid {flag} \"{name}\""))
            .hint("use 1-64 letters, digits, \"-\", \"_\" or \".\""))
    }
}

fn check_env_name(name: &str) -> Result<(), Failure> {
    let ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Failure::usage(format!("invalid --env-secret \"{name}\""))
            .hint("use an environment variable name like HOME_SECRET"))
    }
}

pub fn write_new(path: &Path, contents: &str, force: bool) -> Result<(), Failure> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if force {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => Failure::new(format!(
            "{} already exists, not overwriting it",
            path.display()
        ))
        .hint("pass --force to replace it, or -o to write somewhere else"),
        _ => Failure::new(format!("cannot write {}: {e}", path.display()))
            .hint("check that the directory exists and is writable"),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                Failure::new(format!("cannot set permissions on {}: {e}", path.display()))
            })?;
    }
    #[cfg(windows)]
    crate::base::fs::restrict_on_windows(path);
    file.write_all(contents.as_bytes())
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))
}

pub fn output_format(given: Option<Format>, output: Option<&Path>) -> Result<Format, Failure> {
    let Some(path) = output else {
        return Ok(given.unwrap_or(Format::Yaml));
    };
    match (given, Format::from_path(path)) {
        (Some(g), Some(f)) if g != f => Err(Failure::usage(format!(
            "{} does not match --format {}",
            path.display(),
            if g == Format::Json { "json" } else { "yaml" }
        ))
        .hint("use a matching extension or change --format")),
        (_, Some(f)) => Ok(f),
        (_, None) => Err(Failure::usage(format!(
            "{} has no .yaml, .yml or .json extension",
            path.display()
        ))
        .hint("burrow picks the config format from the extension")),
    }
}

struct Login {
    server: String,
    name: String,
    secret_text: String,
}

fn ask_login(
    role: Role,
    path: &Path,
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
) -> Result<Login, Failure> {
    if path.exists() && !opts.force {
        return Err(Failure::new(format!(
            "{} already exists, not overwriting it",
            path.display()
        ))
        .hint("pass --force to replace it, or -o to write somewhere else"));
    }
    let missing = |what: &str, flag: &str| {
        Failure::usage(format!("missing {what}")).hint(format!(
            "pass {flag} (stdin is not a terminal, so burrow cannot ask)"
        ))
    };
    let server = match &opts.server {
        Some(s) => s.clone(),
        None => prompt
            .ask(&format!(
                "server host name or IP (port {SERVER_PORT} unless you add :PORT)"
            ))
            .ok_or_else(|| missing("server address", "--server HOST"))?,
    };
    let server = Endpoint::parse(&server)
        .map_err(|e| {
            Failure::usage(format!("invalid server address: {e}")).hint(
                "use a host name or IP, for example myserver.com, or myserver.com:2333 for another port",
            )
        })?
        .to_string();
    let kind = role.as_str();
    let name = match &opts.name {
        Some(n) => n.clone(),
        None => prompt
            .ask(&format!("{kind} name (as in the server config)"))
            .ok_or_else(|| missing(&format!("{kind} name"), "--name NAME"))?,
    };
    check_name(&format!("{kind} name"), &name)?;
    let secret_text = match (&opts.env_secret, &opts.secret) {
        (Some(var), _) => {
            check_env_name(var)?;
            format!("${{{var}}}")
        }
        (None, Some(s)) => s.clone(),
        (None, None) => prompt
            .ask("secret (from the server config)")
            .ok_or_else(|| missing("secret", "--secret VALUE or --env-secret VAR"))?,
    };
    if !secret_text.starts_with("${") {
        Secret::parse(&secret_text).map_err(|e| {
            Failure::usage(format!("the secret {}", e.describe())).hint(format!(
                "copy it from the server config, or run on the server: {} {kind}-config NAME -o {kind}.yaml",
                crate::base::failure::command()
            ))
        })?;
    }
    Ok(Login {
        server,
        name,
        secret_text,
    })
}
