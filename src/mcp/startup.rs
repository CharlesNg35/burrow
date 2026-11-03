use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use rmcp::model::{InitializeRequestParams, InitializeResult, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};

use super::journal::Journal;
use crate::base::failure::Failure;

pub const ANSWER_WITHIN: Duration = Duration::from_secs(60);

struct Broken {
    message: String,
}

impl ServerHandler for Broken {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Err(ErrorData::internal_error(self.message.clone(), None))
    }
}

pub fn for_the_app(f: &Failure, log: Option<&Path>) -> String {
    let mut text = format!("burrow mcp could not start: {}", f.message);
    if let Some(h) = &f.hint {
        text.push_str(&format!("; {h}"));
    }
    if let Some(log) = log {
        text.push_str(&format!("; details are in {}.", log.display()));
    }
    text
}

fn answer(message: String) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    rt.block_on(async {
        let served = Broken { message }.serve((tokio::io::stdin(), tokio::io::stdout()));
        let _ = tokio::time::timeout(ANSWER_WITHIN, served).await;
    });
    rt.shutdown_background();
}

pub fn failed(f: Failure, config: Option<&Path>) -> Failure {
    let journal = Journal::open(config);
    let log = journal.as_ref().map(Journal::path);
    let message = for_the_app(&f, log);
    if let Some(j) = &journal {
        j.write(&message);
    }
    if !std::io::stdin().is_terminal() {
        answer(message);
    }
    match log {
        Some(log) => {
            let hint = match &f.hint {
                Some(h) => format!("{h}\ndetails are in {}", log.display()),
                None => format!("details are in {}", log.display()),
            };
            Failure {
                hint: Some(hint),
                ..f
            }
        }
        None => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_gets_the_message_the_hint_and_the_log() {
        let f = Failure::new("config file /x/visitor.yaml does not exist").hint("check the path");
        assert_eq!(
            for_the_app(&f, Some(Path::new("/x/burrow-mcp.log"))),
            "burrow mcp could not start: config file /x/visitor.yaml does not exist; check the path; details are in /x/burrow-mcp.log."
        );
        assert_eq!(
            for_the_app(&Failure::new("boom"), None),
            "burrow mcp could not start: boom"
        );
    }
}
