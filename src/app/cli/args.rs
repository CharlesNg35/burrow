use std::path::PathBuf;

use clap::Args;

use super::{FileFormat, OutputFormat, RoleArg};

#[derive(Debug, Args)]
pub struct EditArgs {
    #[arg(
        value_name = "WHAT",
        help = "A service name, a config file, or server, client or visitor when only one is found; a list to pick from when omitted"
    )]
    pub target: Option<String>,
    #[arg(
        long,
        value_name = "COMMAND",
        help = "Editor to use, for example \"code --wait\"; VISUAL or EDITOR when omitted"
    )]
    pub editor: Option<String>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(
        value_name = "SERVICE",
        conflicts_with = "config",
        help = "Only this installed service, for example burrow-server"
    )]
    pub name: Option<String>,
    #[arg(short, long, help = "Only the instance running this config file")]
    pub config: Option<PathBuf>,
    #[arg(long, help = "Print the raw status as JSON")]
    pub json: bool,
    #[arg(short, long, help = "Keep refreshing, with live rates, until Ctrl-C")]
    pub follow: bool,
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 1.0,
        requires = "follow",
        help = "Seconds between refreshes with --follow"
    )]
    pub interval: f64,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long, help = "Only tell whether a newer release exists")]
    pub check: bool,
    #[arg(
        long,
        value_name = "VERSION",
        help = "Install this release instead of the latest (also older ones)"
    )]
    pub to: Option<String>,
    #[arg(long, help = "Reinstall even when this version is already installed")]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(
        short,
        long,
        help = "Config file (.yaml, .yml or .json); found in the current folder when omitted"
    )]
    pub config: Option<PathBuf>,
    #[arg(long, hide = true, value_name = "FILE")]
    pub env_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ServerArgs {
    #[command(flatten)]
    pub run: RunArgs,
    #[arg(
        long,
        value_name = "DIR",
        help = "Folder for the web state: the Let's Encrypt account and certificates, the local CA and uploads; burrow service install sets it"
    )]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct McpArgs {
    #[arg(
        short,
        long,
        help = "Visitor config file; ./visitor.yaml, or the one installed visitor service, when omitted"
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with = "config",
        help = "Use the config of this installed visitor service"
    )]
    pub service: Option<String>,
    #[arg(
        long,
        help = "Offer the tools that change the config (apply_change, revert_to_version); the visitor also needs admin: write"
    )]
    pub allow_write: bool,
    #[arg(
        long,
        help = "Serve MCP over HTTP on 127.0.0.1 with a bearer token, instead of stdio"
    )]
    pub http: bool,
    #[arg(
        long,
        requires = "http",
        help = "Local port on 127.0.0.1 for --http; 7838 when omitted, 0 picks a free one"
    )]
    pub port: Option<u16>,
    #[arg(
        long,
        requires = "http",
        help = "Print only the URL when the HTTP server starts, not the header with the token; for services and logs"
    )]
    pub no_print_token: bool,
    #[arg(
        long,
        conflicts_with = "print_config",
        help = "Print the bearer token, and exit"
    )]
    pub print_token: bool,
    #[arg(
        long,
        help = "Replace the bearer token; the old one stops working at once"
    )]
    pub new_token: bool,
    #[arg(
        long,
        value_name = "FILE",
        help = "Keep the bearer token in FILE, readable by you only; by default it is kept per visitor config in burrow's private state folder"
    )]
    pub token_file: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        value_name = "APP",
        help = "Print a ready setup for this AI app, and exit; over stdio, or over HTTP with --http"
    )]
    pub print_config: Option<McpApp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum McpApp {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Args)]
pub struct AdminArgs {
    #[arg(
        short,
        long,
        help = "Visitor config file; ./visitor.yaml, or the one installed visitor service, when omitted"
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with = "config",
        help = "Use the config of this installed visitor service"
    )]
    pub service: Option<String>,
    #[arg(
        long,
        help = "Local port on 127.0.0.1 for the UI; the kept session's port or 7839 when free, otherwise any free port; 0 always picks a free one"
    )]
    pub port: Option<u16>,
    #[arg(long, help = "Do not open the browser")]
    pub no_open: bool,
    #[arg(
        long,
        help = "Print the link with its token, which is kept for up to 12 hours per visitor config"
    )]
    pub print_url: bool,
    #[arg(
        long,
        value_name = "FILE",
        help = "Write the link with its token to FILE, readable by you only"
    )]
    pub url_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "DURATION",
        default_value = "30m",
        value_parser = parse_duration,
        help = "Stop after this long without a browser connected, like 90s, 30m or 2h"
    )]
    pub idle_exit: std::time::Duration,
    #[arg(long, help = "Print the version and size of the built-in UI, and exit")]
    pub ui_info: bool,
    #[arg(long, value_name = "DIR", hide = true)]
    pub ui_dir: Option<PathBuf>,
}

pub fn parse_duration(text: &str) -> Result<std::time::Duration, String> {
    let t = text.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (n, unit) = t.split_at(split);
    let n: u64 = n
        .parse()
        .map_err(|_| format!("\"{text}\" is not a duration like 90s, 30m or 2h"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n.saturating_mul(60),
        "h" => n.saturating_mul(3600),
        _ => return Err(format!("\"{text}\" is not a duration like 90s, 30m or 2h")),
    };
    if secs == 0 {
        return Err("the duration must be more than 0".into());
    }
    Ok(std::time::Duration::from_secs(secs))
}

#[derive(Debug, Args)]
pub struct VisitArgs {
    #[arg(
        value_name = "FORWARD",
        conflicts_with_all = ["service", "client"],
        help = "Run only this forward from the config; all of them when omitted"
    )]
    pub forward: Option<String>,
    #[arg(short, long, help = "Visitor config file; ./visitor.yaml when omitted")]
    pub config: Option<PathBuf>,
    #[arg(long, hide = true, value_name = "FILE")]
    pub env_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with = "client",
        requires = "local",
        help = "Reach this private service instead of the forwards in the config"
    )]
    pub service: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Reach targets on this client directly, with --target or --socks (needs reach on the server)"
    )]
    pub client: Option<String>,
    #[arg(
        long,
        value_name = "HOST:PORT",
        requires_all = ["client", "local"],
        help = "Target as seen from the client, or a port alone for 127.0.0.1"
    )]
    pub target: Option<String>,
    #[arg(
        long,
        value_name = "PORT",
        requires = "client",
        conflicts_with_all = ["target", "local", "service"],
        help = "Listen as a SOCKS5 proxy into the client, on this port (127.0.0.1) or ip:port"
    )]
    pub socks: Option<String>,
    #[arg(
        long,
        value_name = "PORT",
        help = "Local port to listen on (127.0.0.1), or ip:port"
    )]
    pub local: Option<String>,
    #[arg(
        long,
        conflicts_with_all = ["socks", "forward"],
        help = "Carry UDP: listen on a local UDP port for --service or --client --target"
    )]
    pub udp: bool,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    #[arg(
        short,
        long,
        help = "Config file (.yaml, .yml or .json); found in the current folder when omitted"
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        help = "Treat the file as this role instead of detecting it"
    )]
    pub role: Option<RoleArg>,
    #[arg(long, help = "Fail on warnings too")]
    pub strict: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, help = "Report format")]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(
        value_enum,
        help = "server for the public machine, client for the machine behind NAT, visitor for a machine that uses private services; asked when omitted"
    )]
    pub role: Option<RoleArg>,
    #[arg(
        long,
        value_enum,
        help = "Config format [default: yaml, or taken from the -o file extension]"
    )]
    pub format: Option<FileFormat>,
    #[arg(
        short,
        long,
        help = "Output path (default: server.yaml, client.yaml or visitor.yaml)"
    )]
    pub output: Option<PathBuf>,
    #[arg(
        long,
        conflicts_with_all = ["force", "server", "name", "secret", "env_secret"],
        help = "Visitor only: ask the server again what this visitor may use, TCP and UDP, and update the forwards of visitor.yaml (or -o FILE): adds new grants, removes revoked ones, kept forwards keep their local ports"
    )]
    pub update: bool,
    #[arg(
        long,
        requires = "update",
        help = "With --update: write the changes without asking"
    )]
    pub yes: bool,
    #[arg(long, help = "Overwrite an existing file")]
    pub force: bool,
    #[arg(
        long,
        value_name = "HOST",
        help = "Server host name or IP, port 7835 unless you add :PORT (server: saved as public_address)"
    )]
    pub server: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Client or visitor name (client and visitor configs)"
    )]
    pub name: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Name of the first client (server config)"
    )]
    pub client_name: Option<String>,
    #[arg(
        long,
        help = "Secret for the client or visitor config (prompted on a TTY when missing)"
    )]
    pub secret: Option<String>,
    #[arg(
        long,
        value_name = "VAR",
        conflicts_with = "secret",
        help = "Write the secret as ${VAR} instead of a literal"
    )]
    pub env_secret: Option<String>,
    #[arg(
        long,
        help = "Server: accept clients over WebSocket only (no TCP control port)"
    )]
    pub ws: bool,
    #[arg(
        long,
        value_name = "IP:PORT",
        help = "Server: WebSocket listen address (default 127.0.0.1:7836, or 0.0.0.0:7837 with TLS files)"
    )]
    pub ws_listen: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Server: WebSocket URL path (default /burrow)"
    )]
    pub ws_path: Option<String>,
    #[arg(
        long,
        value_name = "FILE",
        requires = "ws_tls_key",
        help = "Server: serve wss:// directly with this certificate chain (PEM)"
    )]
    pub ws_tls_cert: Option<String>,
    #[arg(
        long,
        value_name = "FILE",
        requires = "ws_tls_cert",
        help = "Server: private key for --ws-tls-cert (PEM)"
    )]
    pub ws_tls_key: Option<String>,
    #[arg(
        long,
        value_name = "ADDRESS",
        help = "Server: Let's Encrypt contact; with --ws and --server NAME the tunnel shares port 443 with an automatic certificate (accepts Let's Encrypt's terms)"
    )]
    pub email: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Server: serve a first website with automatic HTTPS, like blog.example.com (needs --email)"
    )]
    pub web: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        requires = "web",
        help = "Server: the client that runs the website's app (the config's first client)"
    )]
    pub web_client: Option<String>,
    #[arg(
        long,
        value_name = "TARGET",
        requires = "web",
        help = "Server: the website's app on the client, like 3000 or 127.0.0.1:8080 (default 3000)"
    )]
    pub web_target: Option<String>,
}

#[derive(Debug, Args)]
pub struct ClientConfigArgs {
    #[arg(help = "Client name as defined in the server config; optional when it defines only one")]
    pub name: Option<String>,
    #[command(flatten)]
    pub export: ExportArgs,
}

#[derive(Debug, Args)]
pub struct VisitorConfigArgs {
    #[arg(
        help = "Visitor name as defined in the server config; optional when it defines only one"
    )]
    pub name: Option<String>,
    #[command(flatten)]
    pub export: ExportArgs,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    #[arg(short, long, help = "Server config file; ./server.yaml when omitted")]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        value_name = "HOST",
        help = "Host name or IP of this server; port from listen [default: public_address in the server config]"
    )]
    pub server: Option<String>,
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Write the config to this file (readable by you only) instead of printing it"
    )]
    pub output: Option<PathBuf>,
    #[arg(long, requires = "output", help = "Overwrite the -o file if it exists")]
    pub force: bool,
    #[arg(
        long,
        value_enum,
        help = "Config format [default: yaml, or taken from the -o file extension]"
    )]
    pub format: Option<FileFormat>,
}
