use std::path::PathBuf;

use clap::{Args, Subcommand};

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
        default_value_t = crate::service::DEFAULT_LOG_LINES,
        help = "Number of lines to show"
    )]
    pub lines: usize,
}
