//! `burrow mcp`: an MCP server, over stdio or HTTP, whose tools check and apply config changes.
//! It answers the app's handshake at once and keeps the server link in `upstream`, which
//! connects in the background and again on the next call; `mode` decides the tools and instructions.

pub mod check;
mod handler;
pub mod handover;
pub mod http;
pub mod journal;
pub mod mode;
pub mod plans;
pub mod setup;
pub mod startup;
pub mod stdio;
pub mod token_file;
pub mod tools;
pub mod upstream;
pub mod validate;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::admin_link::client::Link;
use crate::base::failure::Failure;
use crate::config::settings::VisitorSettings;
use mode::Writes;

pub const CLIENT: &str = "burrow-mcp";
pub const CLOSE_WITHIN: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub port: u16,
    pub show_token: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Serve,
    PrintConfig(Option<setup::App>),
    PrintToken,
    Check,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub writes: Writes,
    pub http: Option<HttpOptions>,
    pub token_file: Option<PathBuf>,
    pub new_token: bool,
    pub task: Task,
}

fn token_path(config: &Path, opts: &Options) -> Result<PathBuf, Failure> {
    match &opts.token_file {
        Some(p) => Ok(p.clone()),
        None => token_file::default_path(config),
    }
}

fn print_config(app: Option<setup::App>, config: &Path, opts: &Options) -> Result<(), Failure> {
    let built = match &opts.http {
        None => setup::Setup::stdio(&setup::program(), config, opts.writes),
        Some(h) => {
            let token = token_file::load_or_create(&token_path(config, opts)?)?;
            setup::Setup::http(h.port, &token)
        }
    };
    let p = built.printed(app);
    match p.head_to {
        setup::Stream::Stdout => println!("{}", p.head),
        setup::Stream::Stderr => crate::say_errln!("{}", p.head),
    }
    println!("{}", p.text);
    if let Some(after) = p.after {
        crate::say_errln!("\n{}", after.trim_end_matches('\n'));
    }
    Ok(())
}

pub fn before_start(config: &Path, opts: &Options) -> Result<bool, Failure> {
    if opts.new_token {
        token_file::replace(&token_path(config, opts)?)?;
        crate::say_errln!("made a new token; the old one no longer works");
    }
    match opts.task {
        Task::PrintToken => {
            let token = token_file::load_or_create(&token_path(config, opts)?)?;
            println!("{}", token.as_str());
            Ok(false)
        }
        Task::PrintConfig(app) => print_config(app, config, opts).map(|()| false),
        Task::Serve | Task::Check => Ok(true),
    }
}

pub async fn serve(
    settings: VisitorSettings,
    config: &Path,
    opts: Options,
    stop: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let journal = journal::Journal::open(Some(config));
    let link = Arc::new(Link::new(settings, CLIENT));
    let up = upstream::Upstream::start(link.clone(), opts.writes, journal.clone());
    let notices = match opts.http {
        None => tools::Notices::ToolChanges,
        Some(_) => tools::Notices::Never,
    };
    let tools = tools::Tools::new(up, notices);
    let served = match &opts.http {
        None => {
            if std::io::stdin().is_terminal() {
                let hints = setup::hints(&setup::program(), config, opts.writes, None);
                crate::say_err!("{}", setup::by_hand(&hints));
            }
            stdio::serve(tools, stop).await
        }
        Some(h) => {
            let token = token_path(config, &opts)?;
            let shown = http::Started {
                token: &token,
                show_token: h.show_token,
                config,
                writes: opts.writes,
            };
            http::serve(tools, h, &shown, stop).await
        }
    };
    let _ = tokio::time::timeout(CLOSE_WITHIN, link.close()).await;
    if let (Err(f), Some(j)) = (&served, &journal) {
        j.write(&format!("stopped: {}", f.message));
    }
    served
}
