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

pub const DEFAULT_CLIENT_NAME: &str = "home";
pub const DEFAULT_LISTEN: &str = "0.0.0.0:2333";

pub trait Prompter {
    fn ask(&mut self, question: &str) -> Option<String>;
}

pub struct NoPrompt;

impl Prompter for NoPrompt {
    fn ask(&mut self, _: &str) -> Option<String> {
        None
    }
}

pub struct TtyPrompt;

impl Prompter for TtyPrompt {
    fn ask(&mut self, question: &str) -> Option<String> {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
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
}

fn resolve_output(role: Role, opts: &InitOptions) -> Result<(PathBuf, Format), Failure> {
    let format = opts.format.unwrap_or(Format::Yaml);
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
            "web": { "client": client, "bind": "0.0.0.0:8080", "target": "127.0.0.1:3000" }
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
    if let Some(s) = &opts.server {
        parse_host_port(s).map_err(|e| {
            Failure::usage(format!("invalid --server: {e}"))
                .hint("use host:port, for example myserver.com:2333")
        })?;
    }
    let secret = Secret::generate();
    let secret_text = match &opts.env_secret {
        Some(var) => {
            check_env_name(var)?;
            format!("${{{var}}}")
        }
        None => secret.encode(),
    };
    write_new(
        &path,
        &render(&server_value(client, &secret_text), format),
        opts.force,
    )?;

    let w = |out: &mut dyn Write, s: String| {
        writeln!(out, "{s}").map_err(|e| Failure::new(format!("cannot write output: {e}")))
    };
    w(
        out,
        format!(
            "wrote {} with client \"{client}\" and service \"web\" (0.0.0.0:8080 -> 127.0.0.1:3000)",
            path.display()
        ),
    )?;
    if let Some(var) = &opts.env_secret {
        w(
            out,
            format!(
                "set the secret on both machines before starting:\n\n  export {var}={}\n",
                secret.encode()
            ),
        )?;
    }
    match &opts.server {
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
            w(
                out,
                format!(
                    "\nnext, print the client config with:\n\n  burrow client-config {client} -c {} --server YOUR_PUBLIC_HOST:2333 > client.yaml\n",
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
            .ask("server address (host:port)")
            .ok_or_else(|| missing("server address", "--server HOST:PORT"))?,
    };
    parse_host_port(&server).map_err(|e| {
        Failure::usage(format!("invalid server address: {e}"))
            .hint("use host:port, for example myserver.com:2333")
    })?;
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
            Failure::usage(format!("the secret {}", e.describe())).hint(
                "copy it from the server config, or run: burrow client-config NAME on the server",
            )
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

pub struct ClientConfigOutput {
    pub text: String,
    pub notes: Vec<String>,
}

pub fn client_config(
    name: &str,
    server_config: &Path,
    server: &str,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ClientConfigOutput, Failure> {
    parse_host_port(server).map_err(|e| {
        Failure::usage(format!("invalid --server: {e}"))
            .hint("use the public host and control port, for example myserver.com:2333")
    })?;
    let outcome = load_file(server_config, Some(Role::Server), env).map_err(|e| {
        let (m, h) = e.describe(server_config);
        Failure::new(m).hint(h)
    })?;
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        return Err(
            Failure::new(format!("{} has errors", server_config.display())).hint(format!(
                "see them with: burrow validate -c {}",
                server_config.display()
            )),
        );
    };
    let Some(_) = settings.clients.get(name) else {
        let names: Vec<&str> = settings.clients.keys().map(String::as_str).collect();
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
    let mut notes = Vec::new();
    for (svc_name, svc) in settings.services.iter().filter(|(_, s)| s.client == name) {
        let Ok((host, port)) = parse_host_port(&svc.target) else {
            continue;
        };
        let entries: Vec<String> = match host.parse::<IpAddr>() {
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
