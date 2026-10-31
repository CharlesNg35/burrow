use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum CaAction {
    #[command(
        about = "Print or save the certificate of burrow's local CA (tls: internal), to install on your devices"
    )]
    Export(CaExportArgs),
}

#[derive(Debug, Args)]
pub struct CaExportArgs {
    #[arg(
        value_name = "SERVICE",
        conflicts_with_all = ["config", "state_dir"],
        help = "The installed server service, for example burrow-server"
    )]
    pub name: Option<String>,
    #[arg(short, long, help = "The server's config file")]
    pub config: Option<PathBuf>,
    #[arg(long, value_name = "DIR", help = "The server's state folder")]
    pub state_dir: Option<PathBuf>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Write the certificate to this file instead of printing it"
    )]
    pub out: Option<PathBuf>,
    #[arg(
        long,
        help = "DER instead of PEM (Windows and Android accept both; some tools want DER)"
    )]
    pub der: bool,
}
