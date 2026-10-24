use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::client::client_value;
use super::{
    DEFAULT_CLIENT_NAME, InitOptions, WebSocketChoice, WebSocketInit, check_env_name, check_name,
    default_listen, default_service_bind, resolve_output, web, write_new,
};
use crate::base::failure::Failure;
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

pub fn init_server(opts: &InitOptions, out: &mut dyn Write) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Server, opts)?;
    let client = opts.client_name.as_deref().unwrap_or(DEFAULT_CLIENT_NAME);
    check_name("--client-name", client)?;
    if opts.email.is_some() && opts.websocket.is_none() {
        return Err(
            Failure::usage("--email is for a tunnel with an automatic certificate")
                .hint("add --ws and --server tunnel.example.com"),
        );
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
    let client_server = match &opts.server {
        Some(_) if let Some(t) = &tunnel => Some(format!("wss://{}{}", t.host, t.path)),
        Some(s) if ws.is_none() && s.contains("://") => {
            return Err(
                Failure::usage("--server is a URL but the server config is TCP")
                    .hint("add --ws to accept WebSocket clients, or pass the host name alone"),
            );
        }
        Some(s) => {
            let listen = ws
                .is_none()
                .then(|| default_listen().parse().expect("valid default"));
            let endpoint = burrow_syntax::address::client_address(
                s,
                listen,
                ws.as_ref().map(WebSocketChoice::shape),
                "clients",
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
    let value = match (&tunnel, &ws) {
        (Some(t), _) => web::tunnel_value(t, client, &secret_text, service(client)),
        (None, Some(ws)) => websocket_server_value(client, &secret_text, ws),
        (None, None) => server_value(client, &secret_text),
    };
    let value = match &opts.server {
        Some(address) if tunnel.is_none() => with_public_address(value, address.trim()),
        _ => value,
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
    if let Some(t) = &tunnel {
        w(out, web::tunnel_note(t))?;
    }
    if let Some(ws) = &ws {
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
                    crate::base::failure::command(),
                    path.display()
                ),
            )?;
        }
    }
    Ok(path)
}
