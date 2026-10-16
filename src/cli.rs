use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "burrow",
    version,
    about = "Fast, secure reverse tunnel",
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Info, help = "Log verbosity")]
    pub log_level: LogLevel,
    #[arg(long, global = true, help = "Write logs as JSON lines")]
    pub log_json: bool,
    #[arg(long, short, global = true, help = "Only log warnings and errors")]
    pub quiet: bool,
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
    #[command(about = "Check a config file and report every problem")]
    Validate(ValidateArgs),
    #[command(about = "Write a new server or client config")]
    Init(InitArgs),
    #[command(about = "Print a ready client config for a client defined in the server config")]
    ClientConfig(ClientConfigArgs),
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
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(short, long, help = "Config file (.yaml, .yml or .json)")]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    #[arg(short, long, help = "Config file (.yaml, .yml or .json)")]
    pub config: PathBuf,
    #[arg(
        long,
        value_enum,
        help = "Treat the file as this role instead of detecting it"
    )]
    pub role: Option<RoleArg>,
    #[arg(long, help = "Fail on warnings too")]
    pub strict: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(value_enum)]
    pub role: RoleArg,
    #[arg(long, value_enum, default_value_t = FileFormat::Yaml)]
    pub format: FileFormat,
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
        value_name = "HOST:PORT",
        help = "Server address (client: where to connect; server: shown in the printed client config)"
    )]
    pub server: Option<String>,
    #[arg(long, value_name = "NAME", help = "Client name (client config)")]
    pub name: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Name of the first client (server config)"
    )]
    pub client_name: Option<String>,
    #[arg(
        long,
        help = "Secret for the client config (prompted on a TTY when missing)"
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
    #[arg(help = "Client name as defined in the server config")]
    pub name: String,
    #[arg(short, long, help = "Server config file")]
    pub config: PathBuf,
    #[arg(
        long,
        value_name = "HOST:PORT",
        help = "Public address clients use to reach the server"
    )]
    pub server: String,
    #[arg(long, value_enum, default_value_t = FileFormat::Yaml)]
    pub format: FileFormat,
}
