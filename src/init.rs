use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::check::{parse_host_port, valid_name};
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::tree::{self, Value as TreeValue};
use crate::config::{Format, Loaded, Role, load_file};
use crate::failure::Failure;
use crate::secret::Secret;
use crate::transport::endpoint::Endpoint;

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

pub fn server_value(client: &str, secret_text: &str) -> Value {
    json!({
        "listen": DEFAULT_LISTEN,
        "clients": { client: { "secret": secret_text } },
        "services": {
            "web": { "client": client, "bind": DEFAULT_SERVICE_BIND, "target": "127.0.0.1:3000" }
        }
    })
}

fn with_public_address(value: Value, address: &str) -> Value {
    let Value::Object(map) = value else {
        return value;
    };
    let mut out = serde_json::Map::new();
    if !map.contains_key("listen") {
        out.insert("public_address".into(), address.into());
    }
    for (key, v) in map {
        let after = key == "listen";
        out.insert(key, v);
        if after {
            out.insert("public_address".into(), address.into());
        }
    }
    Value::Object(out)
}

fn websocket_server_value(client: &str, secret_text: &str, ws: &WebSocketChoice) -> Value {
    let mut websocket = json!({ "listen": ws.listen, "path": ws.path });
    if let Some((cert, key)) = &ws.tls {
        websocket["tls"] = json!({ "cert": cert, "key": key });
    }
    json!({
        "websocket": websocket,
        "clients": { client: { "secret": secret_text } },
        "services": {
            "web": { "client": client, "bind": DEFAULT_SERVICE_BIND, "target": "127.0.0.1:3000" }
        }
    })
}

pub fn client_value(server: &str, name: &str, secret_text: &str, allow: &[String]) -> Value {
    json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "allow": allow,
    })
}

pub fn init_server(opts: &InitOptions, out: &mut dyn Write) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Server, opts)?;
    let client = opts.client_name.as_deref().unwrap_or(DEFAULT_CLIENT_NAME);
    check_name("--client-name", client)?;
    let ws = opts
        .websocket
        .as_ref()
        .map(WebSocketInit::resolve)
        .transpose()?;
    let client_server = match &opts.server {
        Some(s) if ws.is_none() && s.contains("://") => {
            return Err(
                Failure::usage("--server is a URL but the server config is TCP")
                    .hint("add --ws to accept WebSocket clients, or pass the host name alone"),
            );
        }
        Some(s) => {
            let listen = ws
                .is_none()
                .then(|| DEFAULT_LISTEN.parse().expect("valid default"));
            let endpoint = crate::config::address::client_address(
                s,
                listen,
                ws.as_ref().map(WebSocketChoice::shape),
                &mut Vec::new(),
            )
            .map_err(|e| Failure::usage(format!("invalid --server: {}", e.message)).hint(e.hint))?;
            Some(endpoint.to_string())
        }
        None => None,
    };
    let secret = Secret::generate();
    let secret_text = match &opts.env_secret {
        Some(var) => {
            check_env_name(var)?;
            format!("${{{var}}}")
        }
        None => secret.encode(),
    };
    let value = match &ws {
        Some(ws) => websocket_server_value(client, &secret_text, ws),
        None => server_value(client, &secret_text),
    };
    let value = match &opts.server {
        Some(address) => with_public_address(value, address.trim()),
        None => value,
    };
    write_new(&path, &render(&value, format), opts.force)?;

    let w = |out: &mut dyn Write, s: String| {
        writeln!(out, "{s}").map_err(|e| Failure::new(format!("cannot write output: {e}")))
    };
    let bind = value["services"]["web"]["bind"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    w(
        out,
        format!(
            "wrote {} with client \"{client}\" and service \"web\" ({bind} -> 127.0.0.1:3000)",
            path.display()
        ),
    )?;
    if let Some(ws) = &ws {
        let note = match &ws.tls {
            Some((cert, key)) => format!(
                "clients connect with wss:// to {} using {cert} and {key}; no TCP control port is opened",
                ws.listen
            ),
            None => format!(
                "WebSocket clients are accepted on ws://{}{}; point your TLS proxy (nginx, Caddy, Cloudflare) at it, see README. No TCP control port is opened",
                ws.listen, ws.path
            ),
        };
        w(out, note)?;
        if let Some((cert, key)) = &ws.tls {
            for f in [cert, key].into_iter().filter(|f| !Path::new(f).exists()) {
                w(
                    out,
                    format!("note: {f} does not exist yet; create it before starting the server"),
                )?;
            }
        }
    }
    if let Some(var) = &opts.env_secret {
        w(
            out,
            format!(
                "set the secret on both machines before starting:\n\n  export {var}={}\n",
                secret.encode()
            ),
        )?;
    }
    match &client_server {
        Some(server) => {
            let client_cfg = client_value(
                server,
                client,
                &secret_text,
                &["127.0.0.1/32:3000".to_string()],
            );
            let client_path = match format {
                Format::Yaml => "client.yaml",
                Format::Json => "client.json",
            };
            w(
                out,
                format!("\nclient config (save as {client_path} on the client machine):\n"),
            )?;
            w(out, render(&client_cfg, format))?;
        }
        None => {
            let example = match &ws {
                Some(ws) => format!("wss://YOUR_PUBLIC_HOST{}", ws.path),
                None => "YOUR_PUBLIC_HOST".to_string(),
            };
            w(
                out,
                format!(
                    "\nnext, print the client config with:\n\n  {} client-config -c {} --server {example} -o client.yaml",
                    crate::failure::command(),
                    path.display()
                ),
            )?;
        }
    }
    Ok(path)
}

pub fn init_client(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Client, opts)?;
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
            .ask("server host name or IP (port 7835 unless you add :PORT)")
            .ok_or_else(|| missing("server address", "--server HOST"))?,
    };
    let server = Endpoint::parse(&server)
        .map_err(|e| {
            Failure::usage(format!("invalid server address: {e}")).hint(
                "use a host name or IP, for example myserver.com, or myserver.com:2333 for another port",
            )
        })?
        .to_string();
    let name = match &opts.name {
        Some(n) => n.clone(),
        None => prompt
            .ask("client name (as in the server config)")
            .ok_or_else(|| missing("client name", "--name NAME"))?,
    };
    check_name("client name", &name)?;
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
                "copy it from the server config, or run on the server: {} client-config NAME -o client.yaml",
                crate::failure::command()
            ))
        })?;
    }
    let value = client_value(&server, &name, &secret_text, &["127.0.0.0/8".to_string()]);
    write_new(&path, &render(&value, format), opts.force)?;
    writeln!(
        out,
        "wrote {} (allows 127.0.0.0/8; edit \"allow\" to expose other networks)",
        path.display()
    )
    .map_err(|e| Failure::new(format!("cannot write output: {e}")))?;
    Ok(path)
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

pub struct ClientConfigOutput {
    pub text: String,
    pub notes: Vec<String>,
}

pub fn client_config(
    name: Option<&str>,
    server_config: &Path,
    server: Option<&str>,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ClientConfigOutput, Failure> {
    let outcome = load_file(server_config, Some(Role::Server), env).map_err(|e| match &e {
        crate::config::load::LoadError::Read(io) if io.kind() == std::io::ErrorKind::NotFound => {
            Failure::usage(format!(
                "server config {} does not exist",
                server_config.display()
            ))
            .hint(format!(
                "pass the server config with -c, or create one with: {}",
                crate::failure::cmd("init server")
            ))
        }
        _ => {
            let (m, h) = e.describe(server_config);
            Failure::new(m).hint(h)
        }
    })?;
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        return Err(
            Failure::new(format!("{} has errors", server_config.display())).hint(format!(
                "see them with: {} validate -c {}",
                crate::failure::command(),
                server_config.display()
            )),
        );
    };
    let names: Vec<&str> = settings.clients.keys().map(String::as_str).collect();
    let name = match (name, names.as_slice()) {
        (Some(n), _) => n,
        (None, [only]) => only,
        (None, []) => {
            return Err(
                Failure::new(format!("{} defines no clients", server_config.display())).hint(
                    format!(
                        "add one under \"clients\", with a secret from: {} gensecret",
                        crate::failure::command()
                    ),
                ),
            );
        }
        (None, _) => {
            let mut example = format!(
                "{} client-config {} -c {}",
                crate::failure::command(),
                names[0],
                server_config.display()
            );
            if let Some(s) = server {
                example.push_str(&format!(" --server {s}"));
            }
            return Err(Failure::usage(format!(
                "{} defines {} clients; which one?",
                server_config.display(),
                names.len()
            ))
            .hint(format!(
                "pass its name, for example: {example} (defined clients: {})",
                names.join(", ")
            )));
        }
    };
    let Some(_) = settings.clients.get(name) else {
        let hint = match crate::suggest::closest(name, names.iter().copied()) {
            Some(s) => format!(
                "did you mean \"{s}\"? defined clients: {}",
                names.join(", ")
            ),
            None => format!("defined clients: {}", names.join(", ")),
        };
        return Err(Failure::new(format!(
            "no client named \"{name}\" in {}",
            server_config.display()
        ))
        .hint(hint));
    };
    let shown = server_config.display();
    let (given, from) = match server.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => (s.to_string(), "--server"),
        None => match &settings.public_address {
            Some(p) => (p.clone(), "public_address"),
            None => {
                let port = settings
                    .listen
                    .map(|l| format!(" (the port, {}, comes from listen)", l.port()))
                    .unwrap_or_default();
                return Err(Failure::usage(
                    "client-config needs the address clients connect to",
                )
                .hint(format!(
                    "pass --server myserver.example.com, or add \"public_address: myserver.example.com\" to {shown}{port}"
                )));
            }
        },
    };
    let mut notes = Vec::new();
    let server = crate::config::address::client_address(
        &given,
        settings.listen,
        settings.websocket.as_ref().map(Into::into),
        &mut notes,
    )
    .map_err(|e| Failure::usage(format!("invalid {from}: {}", e.message)).hint(e.hint))?
    .to_string();
    let server = server.as_str();

    let raw = std::fs::read_to_string(server_config).unwrap_or_default();
    let raw_secret = tree::parse(&raw).ok().and_then(|root| {
        let node = root.get("clients")?.get(name)?.get("secret")?.clone();
        match node.value {
            TreeValue::Str(s) => Some(s),
            TreeValue::Seq(items) => items.last().and_then(|n| n.as_str().map(str::to_string)),
            _ => None,
        }
    });
    let secret_text = match raw_secret {
        Some(s) if s.contains("${") => s,
        _ => settings.clients[name]
            .secrets
            .last()
            .map(Secret::encode)
            .unwrap_or_default(),
    };

    let mut allow = Vec::new();
    for (svc_name, svc) in settings.services.iter().filter(|(_, s)| s.client == name) {
        let Ok((host, port)) = parse_host_port(&svc.target) else {
            continue;
        };
        let host_ip = host.parse::<IpAddr>().map(|ip| match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        });
        let entries: Vec<String> = match host_ip {
            Ok(IpAddr::V4(ip)) => vec![format!("{ip}/32:{port}")],
            Ok(IpAddr::V6(ip)) => vec![format!("[{ip}/128]:{port}")],
            Err(_) if host.eq_ignore_ascii_case("localhost") => {
                vec![format!("127.0.0.1/32:{port}"), format!("[::1/128]:{port}")]
            }
            Err(_) => {
                notes.push(format!(
                    "service \"{svc_name}\" targets host name {host}; add its IP address to \"allow\" on the client"
                ));
                Vec::new()
            }
        };
        for e in entries {
            if !allow.contains(&e) {
                allow.push(e);
            }
        }
    }
    if allow.is_empty() {
        notes.push("no IP targets found for this client; edit \"allow\" before starting it".into());
    }
    let mut value = client_value(server, name, &secret_text, &allow);
    if settings.insecure_plaintext {
        value["insecure_plaintext"] = Value::Bool(true);
    }
    Ok(ClientConfigOutput {
        text: render(&value, format),
        notes,
    })
}
