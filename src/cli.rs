use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "burrow", version, about = "Fast, secure reverse tunnel")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Info, help = "Log verbosity", help_heading = "Logging")]
    pub log_level: LogLevel,
    #[arg(
        long,
        global = true,
        help = "Write logs as JSON lines",
        help_heading = "Logging"
    )]
    pub log_json: bool,
    #[arg(
        long,
        short,
        global = true,
        help = "Only log warnings and errors",
        help_heading = "Logging"
    )]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help = "Write logs to this file (rotated at 10 MB, 5 old files kept)",
        help_heading = "Logging"
    )]
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    Server,
    Client,
    Visitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FileFormat {
    Yaml,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Run the public server")]
    Server(RunArgs),
    #[command(about = "Run the client behind NAT")]
    Client(RunArgs),
    #[command(about = "Forward local ports to private services through the server")]
    Visit(VisitArgs),
    #[command(about = "Check a config file and report every problem")]
    Validate(ValidateArgs),
    #[command(
        about = "Open the admin UI of a server in your browser, as a visitor with admin access"
    )]
    Admin(AdminArgs),
    #[command(
        about = "Show the running burrow servers on this machine: clients, services, visitors and traffic"
    )]
    Status(StatusArgs),
    #[command(
        about = "Open a config in your editor, check it, save it and apply it: one in this folder, of an installed service or of a running instance"
    )]
    Edit(EditArgs),
    #[command(about = "Write a new server, client or visitor config")]
    Init(InitArgs),
    #[command(about = "Print a ready client config for a client defined in the server config")]
    ClientConfig(ClientConfigArgs),
    #[command(about = "Print a ready visitor config for a visitor defined in the server config")]
    VisitorConfig(VisitorConfigArgs),
    #[command(about = "Print a new random 256-bit secret")]
    Gensecret,
    #[command(about = "Print the JSON Schema of a config file")]
    Schema {
        #[arg(value_enum)]
        role: RoleArg,
    },
    #[command(about = "Print shell completions")]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(about = "Print the version")]
    Version,
    #[command(
        about = "Download the latest release, replace this binary and update the installed services"
    )]
    Update(UpdateArgs),
    #[command(about = "Run burrow as a system service: install, status, logs, uninstall")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    #[command(
        about = "Install and start a service for a server, client or visitor config (needs admin rights)"
    )]
    Install(ServiceInstallArgs),
    #[command(about = "Stop and remove a service")]
    Uninstall(ServiceUninstallArgs),
    #[command(
        about = "Replace the installed burrow binary with this one if it is newer, and restart the services"
    )]
    Update(ServiceUpdateArgs),
    #[command(about = "Start a service")]
    Start(ServiceName),
    #[command(about = "Stop a service")]
    Stop(ServiceName),
    #[command(about = "Restart a service")]
    Restart(ServiceControlArgs),
    #[command(about = "Reload the config of a running service")]
    Reload(ServiceControlArgs),
    #[command(about = "Show whether a service is running, and which files it uses")]
    Status(ServiceName),
    #[command(about = "Show the logs of a service")]
    Logs(ServiceLogsArgs),
    #[command(about = "List the burrow services on this machine")]
    List,
    #[command(hide = true)]
    Run(ServiceName),
}

#[derive(Debug, Args)]
pub struct ServiceInstallArgs {
    #[arg(
        short,
        long,
        help = "Config file; found in the current folder when omitted"
    )]
    pub config: Option<PathBuf>,
    #[arg(long, help = "Service name (default: burrow-<config file name>)")]
    pub name: Option<String>,
    #[arg(
        long,
        value_name = "USER",
        help = "Run as this existing account instead of a dedicated one"
    )]
    pub run_as: Option<String>,
    #[arg(long, help = "Install without starting")]
    pub no_start: bool,
    #[arg(long, help = "Print what would be installed, change nothing")]
    pub print: bool,
    #[arg(long, hide = true)]
    pub worker: Option<PathBuf>,
    #[arg(
        long,
        help = "Let a server service change its own config, for burrow admin; on by default when a visitor has admin access"
    )]
    pub admin_writable: bool,
}

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
pub struct ServiceUpdateArgs {
    #[arg(help = "Update only this service; all burrow services when omitted")]
    pub name: Option<String>,
    #[arg(
        long,
        help = "Also replace a newer installed binary (downgrade) or reinstall the same version"
    )]
    pub force: bool,
    #[arg(long, hide = true, requires_all = ["from", "as_version"])]
    pub worker: Option<std::path::PathBuf>,
    #[arg(long, hide = true, requires = "worker")]
    pub from: Option<std::path::PathBuf>,
    #[arg(long, hide = true, requires = "worker")]
    pub as_version: Option<String>,
}

#[derive(Debug, Args)]
pub struct ServiceUninstallArgs {
    #[arg(help = "Service name; optional when only one burrow service is installed")]
    pub name: Option<String>,
    #[arg(long, help = "Also delete the installed config and saved environment")]
    pub purge: bool,
}

#[derive(Debug, Args)]
pub struct ServiceName {
    #[arg(help = "Service name; optional when only one burrow service is installed")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct ServiceControlArgs {
    #[arg(help = "Service name; optional when only one burrow service is installed")]
    pub name: Option<String>,
    #[arg(long, hide = true)]
    pub worker: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ServiceLogsArgs {
    #[arg(help = "Service name; optional when only one burrow service is installed")]
    pub name: Option<String>,
    #[arg(short, long, help = "Keep printing new lines")]
    pub follow: bool,
    #[arg(
        short = 'n',
        long,
        default_value_t = 50,
        help = "Number of lines to show"
    )]
    pub lines: usize,
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
        default_value_t = 0,
        help = "Local port on 127.0.0.1 for the UI; 0 picks a free one"
    )]
    pub port: u16,
    #[arg(long, help = "Do not open the browser")]
    pub no_open: bool,
    #[arg(long, help = "Print the link with its one-time token")]
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
        help = "Visitor only: ask the server again what this visitor may use and update the forwards of visitor.yaml (or -o FILE)"
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
