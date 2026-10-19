use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use windows_service::define_windows_service;
use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceDependency, ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
    ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::system::run;
use super::{Backend, Manifest, Platform, Status};
use crate::failure::Failure;
use crate::run::{Env, Hosted, Logging};

const SYSTEM: &str = "*S-1-5-18";
const ADMINS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";
const ALL: ServiceAccess = ServiceAccess::QUERY_STATUS
    .union(ServiceAccess::START)
    .union(ServiceAccess::STOP)
    .union(ServiceAccess::DELETE)
    .union(ServiceAccess::CHANGE_CONFIG)
    .union(ServiceAccess::QUERY_CONFIG);

pub struct WindowsService;

fn scm_error(what: &str, e: windows_service::Error) -> Failure {
    let detail = match &e {
        windows_service::Error::Winapi(io) => io.to_string(),
        other => other.to_string(),
    };
    Failure::new(format!("cannot {what}: {detail}"))
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, Failure> {
    ServiceManager::local_computer(None::<&str>, access)
        .map_err(|e| scm_error("open the Windows service manager", e))
}

fn open(m: &Manifest) -> Result<Service, Failure> {
    manager(ServiceManagerAccess::CONNECT)?
        .open_service(&m.name, ALL)
        .map_err(|e| scm_error(&format!("open service {}", m.name), e))
}

fn system_account(account: &str) -> bool {
    matches!(
        account.to_ascii_lowercase().as_str(),
        "localsystem" | r"nt authority\system" | "system"
    )
}

fn info(m: &Manifest) -> ServiceInfo {
    let account = (!system_account(&m.account)).then(|| OsString::from(&m.account));
    ServiceInfo {
        name: OsString::from(&m.name),
        display_name: OsString::from(format!("burrow {} {}", m.role, m.short())),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: m.binary.clone(),
        launch_arguments: vec!["service".into(), "run".into(), OsString::from(&m.name)],
        dependencies: vec![ServiceDependency::Service("Tcpip".into())],
        account_name: account,
        account_password: None,
    }
}

fn icacls(path: &Path, args: &[&str]) -> Result<(), Failure> {
    let p = path.to_string_lossy();
    let mut all = vec![p.as_ref()];
    all.extend_from_slice(args);
    run("icacls", &all).map(drop)
}

const TRUSTED_OWNERS: [&str; 3] = [
    "S-1-5-32-544",
    "S-1-5-18",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

fn owner_sid(path: &Path) -> Option<String> {
    let script = format!(
        "(Get-Acl -LiteralPath '{}').GetOwner([System.Security.Principal.SecurityIdentifier]).Value",
        path.display().to_string().replace('\'', "''")
    );
    let out = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .ok()?;
    let sid = out.trim();
    sid.starts_with("S-1-").then(|| sid.to_string())
}

fn is_link(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & REPARSE_POINT != 0)
}

fn grant(path: &Path, account: &str, rights: &str) -> Result<(), Failure> {
    if system_account(account) || !path.exists() {
        return Ok(());
    }
    icacls(path, &["/grant", &format!("{account}:{rights}")])
}

fn broad(account: &str) -> bool {
    let name = account.trim().trim_start_matches('*').to_ascii_lowercase();
    let name = name.rsplit('\\').next().unwrap_or(&name);
    [
        "everyone",
        "users",
        "authenticated users",
        "guests",
        "interactive",
        "network",
        "anonymous logon",
        "s-1-1-0",
        "s-1-5-2",
        "s-1-5-4",
        "s-1-5-7",
        "s-1-5-11",
        "s-1-5-32-545",
        "s-1-5-32-546",
    ]
    .contains(&name)
}

fn grant_dirs(account: &str) -> Result<(), Failure> {
    let p = Platform::Windows;
    grant(&p.state_dir(), account, "(OI)(CI)RX")?;
    grant(&p.log_dir(), account, "(OI)(CI)M")?;
    grant(&crate::instance::default_dir(), account, "(OI)(CI)M")
}

fn wait_for(
    service: &Service,
    done: impl Fn(ServiceState) -> bool,
    limit: Duration,
) -> Option<ServiceStatus> {
    let start = Instant::now();
    loop {
        let st = service.query_status().ok()?;
        if done(st.current_state) || start.elapsed() > limit {
            return Some(st);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn read_env(path: Option<&PathBuf>) -> BTreeMap<String, String> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

impl Backend for WindowsService {
    fn kind(&self) -> &'static str {
        "Windows"
    }

    fn render(&self, m: &Manifest) -> String {
        format!(
            "name:     {}\ncommand:  \"{}\" service run {}\naccount:  {}\nstart:    automatic, after Tcpip\nrecovery: restart after 5 s, 30 s from the third failure, reset after 1 day\nconfig:   {}\nlogs:     {}\n",
            m.name,
            m.binary.display(),
            m.name,
            m.account,
            m.config.display(),
            m.log_file.display()
        )
    }

    fn ensure_account(&self, _account: &str) -> Result<bool, Failure> {
        Ok(false)
    }

    fn remove_account(&self, _account: &str) -> Result<(), Failure> {
        Ok(())
    }

    fn cannot_use(&self, m: &Manifest, file: &Path) -> Option<String> {
        if file.is_relative() {
            Some("a service needs an absolute path".into())
        } else if !m.files.iter().any(|f| f == file) {
            Some(format!(
                "its account {} was given read access only to the files of the installed config",
                m.account
            ))
        } else {
            None
        }
    }

    fn can_read(&self, _account: &str, path: &Path) -> bool {
        path.exists()
    }

    fn secure_file(&self, path: &Path, _account: &str) -> Result<(), Failure> {
        icacls(
            path,
            &[
                "/inheritance:r",
                "/grant:r",
                &format!("{SYSTEM}:F"),
                &format!("{ADMINS}:F"),
            ],
        )
    }

    fn prepare_dirs(&self, _account: &str) -> Result<(), Failure> {
        let p = Platform::Windows;
        let root = p.config_dir();
        let instances = crate::instance::default_dir();
        let inner = [p.state_dir(), p.log_dir(), instances.clone(), p.jobs_dir()];
        for dir in std::iter::once(&root).chain(&inner) {
            if dir.exists()
                && let Some(owner) = owner_sid(dir)
                && !TRUSTED_OWNERS.contains(&owner.as_str())
            {
                return Err(Failure::new(format!(
                    "{} already exists and belongs to {owner}, not to administrators",
                    dir.display()
                ))
                .hint("another user may have created it to take over the service; delete it and run the command again"));
            }
            std::fs::create_dir_all(dir)
                .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
            if is_link(dir) {
                return Err(Failure::new(format!(
                    "{} is a link or junction, not a folder",
                    dir.display()
                ))
                .hint("remove it and run the command again; burrow only changes permissions on its own folders"));
            }
        }
        icacls(&root, &["/setowner", ADMINS, "/C", "/Q"])?;
        icacls(&root, &["/reset"])?;
        icacls(
            &root,
            &[
                "/inheritance:r",
                "/grant:r",
                &format!("{SYSTEM}:(OI)(CI)F"),
                &format!("{ADMINS}:(OI)(CI)F"),
                &format!("{USERS}:(RX)"),
            ],
        )?;
        for dir in inner.iter().filter(|d| d.starts_with(&root)) {
            icacls(dir, &["/setowner", ADMINS, "/C", "/Q"])?;
            icacls(dir, &["/reset"])?;
        }
        if instances.starts_with(&root) {
            icacls(&instances, &["/grant", &format!("{USERS}:(OI)(CI)M")])?;
        }
        for m in crate::service::all_manifests() {
            if !broad(&m.account) {
                grant_dirs(&m.account)?;
            }
        }
        Ok(())
    }

    fn read_env(&self, m: &Manifest) -> BTreeMap<String, String> {
        read_env(m.env_file.as_ref())
    }

    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
        let Some(path) = &m.env_file else {
            return Ok(());
        };
        let _ = std::fs::remove_file(path);
        std::fs::write(path, "")
            .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
        self.secure_file(path, &m.account)?;
        std::fs::write(
            path,
            serde_json::to_string_pretty(env).expect("env serializes"),
        )
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))
    }

    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        let scm = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let service = match scm.open_service(&m.name, ALL) {
            Ok(s) => {
                s.change_config(&info(m))
                    .map_err(|e| scm_error(&format!("update service {}", m.name), e))?;
                s
            }
            Err(_) => scm
                .create_service(&info(m), ALL)
                .map_err(|e| scm_error(&format!("create service {}", m.name), e))?,
        };
        let _ = service.set_description(format!(
            "burrow {} tunnel, config {}",
            m.role,
            m.config.display()
        ));
        service
            .update_failure_actions(ServiceFailureActions {
                reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
                reboot_msg: None,
                command: None,
                actions: Some(vec![
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(5),
                    },
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(5),
                    },
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(30),
                    },
                ]),
            })
            .map_err(|e| scm_error("set the restart policy", e))?;
        service
            .set_failure_actions_on_non_crash_failures(true)
            .map_err(|e| scm_error("set the restart policy", e))?;
        grant(&m.config, &m.account, "R")?;
        if let Some(env) = &m.env_file {
            grant(env, &m.account, "R")?;
        }
        grant_dirs(&m.account)?;
        for f in &m.files {
            grant(f, &m.account, "R")?;
        }
        Ok(())
    }

    fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
        let Ok(service) = open(m) else { return Ok(()) };
        service
            .delete()
            .map_err(|e| scm_error(&format!("remove service {}", m.name), e))
    }

    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        let service = open(m)?;
        grant(&m.config, &m.account, "R")?;
        if let Some(env) = &m.env_file {
            grant(env, &m.account, "R")?;
        }
        for f in &m.files {
            grant(f, &m.account, "R")?;
        }
        if service
            .query_status()
            .is_ok_and(|s| s.current_state == ServiceState::Running)
        {
            return Ok(());
        }
        service.start::<&OsStr>(&[]).map_err(|e| {
            scm_error(&format!("start {}", m.name), e).hint(format!(
                "see why with: {}",
                crate::failure::admin(&format!("service logs {}", m.name))
            ))
        })?;
        wait_for(
            &service,
            |s| s != ServiceState::StartPending,
            Duration::from_secs(20),
        );
        Ok(())
    }

    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        let service = open(m)?;
        if service
            .query_status()
            .is_ok_and(|s| s.current_state == ServiceState::Stopped)
        {
            return Ok(());
        }
        service
            .stop()
            .map_err(|e| scm_error(&format!("stop {}", m.name), e))?;
        wait_for(
            &service,
            |s| s == ServiceState::Stopped,
            Duration::from_secs(30),
        );
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }

    fn reload(&self, m: &Manifest) -> Result<(), Failure> {
        let _ = self.stop(m);
        self.start(m)
    }

    fn status(&self, m: &Manifest) -> Status {
        let Ok(service) = open(m) else {
            return Status {
                installed: false,
                running: false,
                pid: None,
                detail: "not installed".into(),
            };
        };
        let Ok(st) = service.query_status() else {
            return Status {
                installed: true,
                running: false,
                pid: None,
                detail: "status unknown".into(),
            };
        };
        let state = format!("{:?}", st.current_state).to_lowercase();
        let detail = match st.exit_code {
            ServiceExitCode::ServiceSpecific(3) => {
                format!(
                    "the config has errors or cannot be read (exit 3); fix it, then: {}",
                    crate::failure::admin(&format!("service start {}", m.name))
                )
            }
            ServiceExitCode::ServiceSpecific(c) if c != 0 => {
                format!("{state} (last exit code {c})")
            }
            _ => state,
        };
        Status {
            installed: true,
            running: st.current_state == ServiceState::Running,
            pid: st.process_id,
            detail,
        }
    }
}

static NAME: OnceLock<String> = OnceLock::new();
static EXIT: AtomicU8 = AtomicU8::new(0);

define_windows_service!(ffi_service_main, service_main);

pub fn dispatch(name: &str) -> Result<u8, Failure> {
    let _ = NAME.set(name.to_string());
    service_dispatcher::start(name, ffi_service_main).map_err(|e| {
        scm_error("talk to the Windows service manager", e).hint(format!(
            "this command is run by Windows; start the service with: {}",
            crate::failure::admin(&format!("service start {name}"))
        ))
    })?;
    Ok(EXIT.load(Ordering::SeqCst))
}

fn service_main(_args: Vec<OsString>) {
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
    let m = super::load_manifest(name)?;
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
        "server" => crate::run::server_until(&m.config, logging, Some(hosted)),
        "visitor" => crate::run::visit_until(&m.config, logging, Some(hosted)),
        _ => crate::run::client_until(&m.config, logging, Some(hosted)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broad_groups_named_in_a_record_get_no_access() {
        for name in [
            "Everyone",
            "BUILTIN\\Users",
            "*S-1-1-0",
            "NT AUTHORITY\\Authenticated Users",
        ] {
            assert!(broad(name), "{name}");
        }
        for name in ["NT SERVICE\\burrow-home", "LocalService", ".\\svc-burrow"] {
            assert!(!broad(name), "{name}");
        }
    }

    #[test]
    fn only_burrow_under_program_files_is_trusted() {
        let pf = PathBuf::from(std::env::var_os("ProgramFiles").unwrap());
        assert!(crate::service::system::trusted_binary(
            &pf.join("Tools").join("burrow.exe")
        ));
        assert!(!crate::service::system::trusted_binary(
            &pf.join("Tools").join("setup.exe")
        ));
        let around = pf
            .join("..")
            .join("Users")
            .join("Public")
            .join("burrow.exe");
        assert!(!crate::service::system::trusted_binary(&around));
        let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        assert!(!crate::service::system::trusted_binary(
            &windows.join("Temp").join("burrow.exe")
        ));
    }
}
