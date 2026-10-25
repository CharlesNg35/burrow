pub mod http;
pub mod plans;
pub mod setup;
pub mod stdio;
pub mod token_file;
pub mod tools;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use burrow_admin_proto::msg::AdminLevel;
use tokio::sync::watch;

use crate::admin_link::client::{Link, LinkError};
use crate::base::failure::Failure;
use crate::config::settings::VisitorSettings;

pub const CLIENT: &str = "burrow-mcp";

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub port: u16,
    pub show_token: bool,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub allow_write: bool,
    pub http: Option<HttpOptions>,
    pub token_file: Option<PathBuf>,
    pub new_token: bool,
    pub print_token: bool,
    pub print_config: Option<setup::App>,
    pub config_given: bool,
}

fn token_path(config: &Path, opts: &Options) -> Result<PathBuf, Failure> {
    match &opts.token_file {
        Some(p) => Ok(p.clone()),
        None => token_file::default_path(config),
    }
}

fn print_config(app: setup::App, config: &Path, opts: &Options) -> Result<(), Failure> {
    let s = match &opts.http {
        None => setup::stdio(app, config),
        Some(h) => {
            let token = token_file::load_or_create(&token_path(config, opts)?)?;
            setup::http(app, h.port, &token)
        }
    };
    if app == setup::App::Cursor {
        crate::say_errln!("{}", s.header);
    } else {
        println!("{}", s.header);
    }
    println!("{}", s.text);
    if let Some(after) = s.after {
        crate::say_errln!("\n{after}");
    }
    Ok(())
}

pub fn before_start(config: &Path, opts: &Options) -> Result<bool, Failure> {
    if opts.new_token {
        token_file::replace(&token_path(config, opts)?)?;
        crate::say_errln!("made a new token; the old one no longer works");
    }
    if opts.print_token {
        let token = token_file::load_or_create(&token_path(config, opts)?)?;
        println!("{}", token.as_str());
        return Ok(false);
    }
    if let Some(app) = opts.print_config {
        print_config(app, config, opts)?;
        return Ok(false);
    }
    Ok(true)
}

pub async fn serve(
    settings: VisitorSettings,
    config: &Path,
    opts: Options,
    stop: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let (visitor, server) = (settings.name.clone(), settings.server.clone());
    let link = Arc::new(Link::new(settings, CLIENT));
    let welcome = link.welcome().await.map_err(|e| match e {
        LinkError::Open(f) => f,
        other => Failure::new(other.to_string()),
    })?;
    let level = welcome.you.level;
    if opts.allow_write && level != AdminLevel::Write {
        crate::say_errln!(
            "visitor \"{visitor}\" has read admin rights on {server}, so burrow mcp offers no tools that change the config"
        );
    }
    let tools = tools::Tools::new(link.clone(), level, opts.allow_write);
    let shown_config = opts.config_given.then_some(config);
    let served = match &opts.http {
        None => {
            if std::io::stdin().is_terminal() {
                crate::say_err!("{}", setup::by_hand(shown_config));
            }
            stdio::serve(tools, stop).await
        }
        Some(h) => {
            let token = token_path(config, &opts)?;
            let shown = http::Started {
                token: &token,
                show_token: h.show_token,
                config: shown_config,
            };
            http::serve(tools, h, &shown, stop).await
        }
    };
    link.close().await;
    served
}
