use std::ffi::OsString;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

use super::{read_env, scm_error};
use crate::app::run::{Hosted, Logging};
use crate::base::failure::Failure;
use crate::config::env::Env;

static NAME: OnceLock<String> = OnceLock::new();
static EXIT: AtomicU8 = AtomicU8::new(0);

define_windows_service!(ffi_service_main, service_main);

pub fn dispatch(name: &str) -> Result<u8, Failure> {
    let _ = NAME.set(name.to_string());
    service_dispatcher::start(name, ffi_service_main).map_err(|e| {
        scm_error("talk to the Windows service manager", e).hint(format!(
            "this command is run by Windows; start the service with: {}",
            crate::base::failure::admin(&format!("service start {name}"))
        ))
    })?;
    Ok(EXIT.load(Ordering::SeqCst))
}

pub(super) fn service_main(_args: Vec<OsString>) {
    let name = NAME.get().cloned().unwrap_or_default();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
            let _ = tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(handle) = service_control_handler::register(&name, handler) else {
        return;
    };
    let report = |state: ServiceState, code: u8| {
        let _ = handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: if code == 0 {
                ServiceExitCode::NO_ERROR
            } else {
                ServiceExitCode::ServiceSpecific(code.into())
            },
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        });
    };
    report(ServiceState::Running, 0);
    let code = match serve(&name, rx) {
        Ok(c) => c,
        Err(f) => f.exit,
    };
    EXIT.store(code, Ordering::SeqCst);
    report(ServiceState::Stopped, code);
}

fn serve(name: &str, stop: tokio::sync::watch::Receiver<bool>) -> Result<u8, Failure> {
    let m = crate::service::load_manifest(name)?;
    crate::instance::running_as_service();
    let logging = Logging {
        level: tracing::Level::INFO,
        json: false,
        file: Some(m.log_file.clone()),
    };
    let hosted = Hosted {
        stop: Some(stop),
        env: Env::with(read_env(m.env_file.as_ref())),
    };
    match m.role.as_str() {
        "server" => crate::app::run::server_until(&m.config, logging, Some(hosted)),
        "visitor" => crate::app::run::visit_until(&m.config, logging, Some(hosted)),
        _ => crate::app::run::client_until(&m.config, logging, Some(hosted)),
    }
}
