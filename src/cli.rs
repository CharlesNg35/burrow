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
    #[command(about = "Run burrow as a system service: install, status, logs, uninstall")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    #[command(
        about = "Install and start a service for a server or client config (needs admin rights)"
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
    Restart(ServiceName),
    #[command(about = "Reload the config of a running service")]
    Reload(ServiceName),
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
        help = "Config file; ./server.yaml or ./client.yaml when omitted"
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
        requires_all = ["target", "local"],
        help = "Reach a target on this client directly (needs reach on the server)"
    )]
    pub client: Option<String>,
    #[arg(
        long,
        value_name = "HOST:PORT",
        requires = "client",
        help = "Target as seen from the client, or a port alone for 127.0.0.1"
    )]
    pub target: Option<String>,
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
        help = "Output path (default: server.yaml or client.yaml)"
    )]
    pub output: Option<PathBuf>,
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
