mod client;
mod server;

pub use client::{ClientConfigOutput, client_config, client_value, init_client};
pub use server::{init_server, server_value};

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::check::valid_name;
use crate::config::{Format, Role};
use crate::failure::Failure;

pub const DEFAULT_CLIENT_NAME: &str = "home";
pub const DEFAULT_LISTEN: &str = "0.0.0.0:7835";
pub const DEFAULT_SERVICE_BIND: &str = "0.0.0.0:7840";

pub trait Prompter {
    fn ask(&mut self, question: &str) -> Option<String>;
    fn interactive(&self) -> bool {
        false
    }
}

pub struct NoPrompt;

impl Prompter for NoPrompt {
    fn ask(&mut self, _: &str) -> Option<String> {
        None
    }
}

pub struct TtyPrompt;

impl Prompter for TtyPrompt {
    fn interactive(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    }

    fn ask(&mut self, question: &str) -> Option<String> {
        if !self.interactive() {
            return None;
        }
        eprint!("{question}: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        let line = line.trim().to_string();
        (!line.is_empty()).then_some(line)
    }
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
}

#[derive(Debug, Clone, Default)]
pub struct WebSocketInit {
    pub listen: Option<String>,
    pub path: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

pub const DEFAULT_WS_LISTEN: &str = "127.0.0.1:7836";
pub const DEFAULT_WS_TLS_LISTEN: &str = "0.0.0.0:7837";

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
                DEFAULT_WS_TLS_LISTEN
            } else {
                DEFAULT_WS_LISTEN
            }
            .to_string()
        });
        listen.parse::<std::net::SocketAddr>().map_err(|_| {
            Failure::usage(format!("invalid --ws-listen \"{listen}\""))
                .hint("use ip:port, for example 127.0.0.1:7836")
        })?;
        let path = self
            .path
            .clone()
            .unwrap_or_else(crate::config::model::default_ws_path);
        if !path.starts_with('/') || path.contains(['?', '#', ' ']) {
            return Err(Failure::usage(format!("invalid --ws-path \"{path}\""))
                .hint("use a path like /burrow"));
        }
        Ok(WebSocketChoice { listen, path, tls })
    }
}

impl WebSocketChoice {
    fn shape(&self) -> crate::config::address::WsShape<'_> {
        crate::config::address::WsShape {
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
