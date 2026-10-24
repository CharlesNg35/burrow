use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::client::client_value;
use super::{
    DEFAULT_CLIENT_NAME, InitOptions, WebSocketChoice, WebSocketInit, check_env_name, check_name,
    default_listen, default_service_bind, resolve_output, web, write_new,
};
use crate::base::failure::Failure;
use crate::base::prompt::{NoPrompt, Prompter};
use crate::config::emit::render;
use crate::config::{Format, Role};
use crate::wire::secret::Secret;

pub fn server_value(client: &str, secret_text: &str) -> Value {
    json!({
        "listen": default_listen(),
        "clients": { client: { "secret": secret_text } },
        "services": {
            "web": { "client": client, "bind": default_service_bind(), "target": "127.0.0.1:3000" }
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
            "web": { "client": client, "bind": default_service_bind(), "target": "127.0.0.1:3000" }
        }
    })
}

fn service(client: &str) -> Value {
    json!({ "client": client, "bind": default_service_bind(), "target": "127.0.0.1:3000" })
}

fn client_server(
    opts: &InitOptions,
    tunnel: Option<&web::Tunnel>,
    ws: Option<&WebSocketChoice>,
) -> Result<Option<String>, Failure> {
    let Some(s) = &opts.server else {
        return Ok(None);
    };
    if let Some(t) = tunnel {
        return Ok(Some(format!("wss://{}{}", t.host, t.path)));
    }
    if ws.is_none() && s.contains("://") {
        return Err(
            Failure::usage("--server is a URL but the server config is TCP")
                .hint("add --ws to accept WebSocket clients, or pass the host name alone"),
        );
    }
    let listen = ws
        .is_none()
        .then(|| default_listen().parse().expect("valid default"));
    let endpoint = burrow_syntax::address::client_address(
        s,
        listen,
        ws.map(WebSocketChoice::shape),
        "clients",
        &mut Vec::new(),
    )
    .map_err(|e| Failure::usage(format!("invalid --server: {}", e.message)).hint(e.hint))?;
    Ok(Some(endpoint.to_string()))
}

pub fn init_server(opts: &InitOptions, out: &mut dyn Write) -> Result<PathBuf, Failure> {
    init_server_asking(opts, &mut NoPrompt, out)
}

pub fn init_server_asking(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Server, opts)?;
    let client = opts.client_name.as_deref().unwrap_or(DEFAULT_CLIENT_NAME);
    check_name("--client-name", client)?;
    if opts.email.is_some() && opts.websocket.is_none() && opts.web.is_none() {
        return Err(Failure::usage(
            "--email is for websites or a tunnel with an automatic certificate",
        )
        .hint("add --web blog.example.com, or --ws and --server tunnel.example.com"));
    }
    let tunnel = web::tunnel_choice(opts)?;
    let ws = match tunnel {
        Some(_) => None,
        None => opts
            .websocket
            .as_ref()
            .map(WebSocketInit::resolve)
            .transpose()?,
    };
    let to_server = client_server(opts, tunnel.as_ref(), ws.as_ref())?;
    let site = web::site_choice(opts, client, prompt)?;
    let secret = Secret::generate();
    let secret_text = match &opts.env_secret {
        Some(var) => {
            check_env_name(var)?;
            format!("${{{var}}}")
        }
        None => secret.encode(),
    };
    let value = match (&tunnel, &ws) {
        (Some(t), _) => web::tunnel_value(t, client, &secret_text, service(client)),
        (None, Some(ws)) => websocket_server_value(client, &secret_text, ws),
        (None, None) => server_value(client, &secret_text),
    };
    let value = match &opts.server {
        Some(address) if tunnel.is_none() => with_public_address(value, address.trim()),
        _ => value,
    };
    let value = match &site {
        Some(site) => web::with_site(value, site),
        None => value,
    };
    write_new(&path, &render(&value, format), opts.force)?;
    let done = Written {
        path: &path,
        format,
        client,
        secret_text: &secret_text,
    };
    report(&done, (tunnel.as_ref(), ws.as_ref(), site.as_ref()), out)?;
    if let Some(var) = &opts.env_secret {
        line(
            out,
            format!(
                "set the secret on both machines before starting:\n\n  export {var}={}\n",
                secret.encode()
            ),
        )?;
    }
    next_steps(&done, to_server.as_deref(), ws.as_ref(), site.as_ref(), out)?;
    Ok(path)
}

struct Written<'a> {
    path: &'a Path,
    format: Format,
    client: &'a str,
    secret_text: &'a str,
}

fn line(out: &mut dyn Write, s: String) -> Result<(), Failure> {
    writeln!(out, "{s}").map_err(|e| Failure::new(format!("cannot write output: {e}")))
}

fn report(
    done: &Written,
    (tunnel, ws, site): (
        Option<&web::Tunnel>,
        Option<&WebSocketChoice>,
        Option<&web::Site>,
    ),
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let (path, client) = (done.path.display(), done.client);
    match site {
        Some(s) => line(
            out,
            format!(
                "wrote {path} with client \"{client}\" and the website {} (service \"{}\" -> {})",
                s.domain, s.service, s.target
            ),
        )?,
        None => line(
            out,
            format!(
                "wrote {path} with client \"{client}\" and service \"web\" ({} -> 127.0.0.1:3000)",
                default_service_bind()
            ),
        )?,
    }
    if let Some(s) = site {
        line(out, web::site_note(s))?;
    }
    if let Some(t) = tunnel {
        line(out, web::tunnel_note(t))?;
    }
    let Some(ws) = ws else {
        return Ok(());
    };
    let note = match &ws.tls {
        Some((cert, key)) => format!(
            "the server serves wss:// on {} with {cert} and {key}; no TCP control port is opened",
            ws.listen
        ),
        None => format!(
            "WebSocket clients are accepted on ws://{}{}; point your TLS proxy (nginx, Caddy, Cloudflare) at it, see https://github.com/charlesng35/burrow/blob/main/docs/websocket.md. No TCP control port is opened",
            ws.listen, ws.path
        ),
    };
    line(out, note)?;
    if let Some((cert, key)) = &ws.tls {
        for f in [cert, key].into_iter().filter(|f| !Path::new(f).exists()) {
            line(
                out,
                format!("note: {f} does not exist yet; create it before starting the server"),
            )?;
        }
    }
    Ok(())
}

fn next_steps(
    done: &Written,
    to_server: Option<&str>,
    ws: Option<&WebSocketChoice>,
    site: Option<&web::Site>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let Some(server) = to_server else {
        let example = match ws {
            Some(ws) => format!("wss://YOUR_PUBLIC_HOST{}", ws.path),
            None => "YOUR_PUBLIC_HOST".to_string(),
        };
        return line(
            out,
            format!(
                "\nnext, print the client config with:\n\n  {} client-config -c {} --server {example} -o client.yaml",
                crate::base::failure::command(),
                done.path.display()
            ),
        );
    };
    let allow = site.map_or_else(
        || "127.0.0.1/32:3000".to_string(),
        |s| web::allow_for(&s.target),
    );
    let client_cfg = client_value(server, done.client, done.secret_text, &[allow]);
    let client_path = match done.format {
        Format::Yaml => "client.yaml",
        Format::Json => "client.json",
    };
    line(
        out,
        format!("\nclient config (save as {client_path} on the client machine):\n"),
    )?;
    line(out, render(&client_cfg, done.format))
}
