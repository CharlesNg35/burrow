use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ClientsArgs {
    #[command(subcommand)]
    pub action: Option<ClientsAction>,
    #[arg(
        long,
        global = true,
        value_name = "SERVICE",
        conflicts_with = "config",
        help = "The installed server service, for example burrow-server"
    )]
    pub service: Option<String>,
    #[arg(
        short,
        long,
        global = true,
        help = "The running server that uses this config file"
    )]
    pub config: Option<PathBuf>,
    #[arg(long, global = true, help = "Print JSON for scripts")]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum ClientsAction {
    #[command(
        about = "Ask clients to update to the latest release, one at a time, and follow them until they are back"
    )]
    Update(ClientsUpdateArgs),
}

#[derive(Debug, Args)]
pub struct ClientsUpdateArgs {
    #[arg(
        value_name = "NAME",
        required_unless_present = "all",
        conflicts_with = "all",
        help = "The clients to update"
    )]
    pub names: Vec<String>,
    #[arg(long, help = "Every client that allows remote updates")]
    pub all: bool,
    #[arg(short, long, help = "Do not ask before updating")]
    pub yes: bool,
}
