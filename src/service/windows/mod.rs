mod acl;
mod dispatch;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceDependency,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::{Backend, Manifest, Platform, Status};
use crate::base::failure::Failure;
use crate::base::windows::{ADMINS, SYSTEM, USERS};
use acl::{
    broad, grant, grant_dirs, holds_only_hand_run_folders, icacls, is_link, owner_and_me, revoke,
    trusted_owner,
};
pub use dispatch::dispatch;

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

    fn config_folder(
        &self,
        dir: &Path,
        _config: &Path,
        _account: &str,
        _writable: bool,
    ) -> Result<(), Failure> {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
        if is_link(dir) {
            return Err(Failure::new(format!(
                "{} is a link or junction, not a folder",
                dir.display()
            )));
        }
        icacls(dir, &["/setowner", ADMINS, "/C", "/Q"])?;
        icacls(
            dir,
            &[
                "/inheritance:r",
                "/grant:r",
                &format!("{SYSTEM}:(OI)(CI)F"),
                &format!("{ADMINS}:(OI)(CI)F"),
            ],
        )
    }

    fn state_folder(&self, m: &Manifest) -> Result<Vec<PathBuf>, Failure> {
        let Some(dir) = &m.web_state else {
            return Ok(Vec::new());
        };
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
        if is_link(dir) {
            return Err(Failure::new(format!(
                "{} is a link or junction, not a folder",
                dir.display()
            )));
        }
        icacls(dir, &["/setowner", ADMINS, "/C", "/Q"])?;
        icacls(
            dir,
            &[
                "/inheritance:r",
                "/grant:r",
                &format!("{SYSTEM}:(OI)(CI)F"),
                &format!("{ADMINS}:(OI)(CI)F"),
            ],
        )?;
        Ok(Vec::new())
    }

    fn prepare_dirs(&self, _account: &str) -> Result<(), Failure> {
        let p = Platform::Windows;
        let root = p.config_dir();
        let instances = crate::instance::default_dir();
        let inner = [p.state_dir(), p.log_dir(), instances.clone(), p.jobs_dir()];
        for dir in std::iter::once(&root).chain(&inner) {
            if *dir != instances
                && dir.exists()
                && !(*dir == root && holds_only_hand_run_folders(&root))
            {
                let Some((owner, me)) = owner_and_me(dir) else {
                    return Err(Failure::new(format!(
                        "cannot read who owns {}, so it cannot be checked before burrow takes it over",
                        dir.display()
                    ))
                    .hint("run the command again from an elevated Windows PowerShell; if it keeps failing, delete the folder and run it again"));
                };
                if owner != me && !trusted_owner(&owner) {
                    return Err(Failure::new(format!(
                        "{} already exists and belongs to {owner}, not to administrators",
                        dir.display()
                    ))
                    .hint("another user may have created it to take over the service; delete it and run the command again"));
                }
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
        for m in crate::install::all_manifests() {
            if !broad(&m.account) {
                grant_dirs(&m.account)?;
            }
        }
        Ok(())
    }

    fn read_env(&self, m: &Manifest) -> BTreeMap<String, String> {
        super::read_json_env(m)
    }

    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
        let Some(path) = &m.env_file else {
            return Ok(());
        };
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(Failure::new(format!("cannot replace {}: {e}", path.display()))
                    .hint("another program may hold it open; delete the file and run the command again"));
            }
            _ => {}
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
        self.secure_file(path, &m.account)?;
        file.write_all(
            serde_json::to_string_pretty(env)
                .expect("env serializes")
                .as_bytes(),
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
        if !m.admin_writable {
            revoke(m.folder(), &m.account)?;
            revoke(&m.config, &m.account)?;
            grant(m.folder(), &m.account, "RX")?;
        }
        grant(&m.config, &m.account, "R")?;
        if let Some(env) = &m.env_file {
            grant(env, &m.account, "R")?;
        }
        grant_dirs(&m.account)?;
        if let Some(dir) = m.web_state.as_ref().filter(|_| !broad(&m.account)) {
            grant(dir, &m.account, "(OI)(CI)M")?;
        }
        for f in &m.files {
            grant(f, &m.account, read_rights(f))?;
        }
        if m.admin_writable {
            grant(m.folder(), &m.account, "(OI)(CI)M")?;
            grant(&m.config, &m.account, "M")?;
            let history = crate::config::save::history_dir_of(&m.config);
            if !is_link(&history) {
                for kept in std::fs::read_dir(&history).into_iter().flatten().flatten() {
                    let path = kept.path();
                    if !is_link(&path) && path.is_file() {
                        icacls(&path, &["/reset"])?;
                    }
                }
            }
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
            grant(f, &m.account, read_rights(f))?;
        }
        if service
            .query_status()
            .is_ok_and(|s| s.current_state == ServiceState::Running)
        {
            return Ok(());
        }
        service.start::<&OsStr>(&[]).map_err(|e| {
            scm_error(&format!("start {}", m.name), e).hint(super::logs_hint(&m.name))
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

    fn manual_stop(&self, name: &str) -> String {
        format!("Stop-Service {name} (as administrator)")
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
            ServiceExitCode::ServiceSpecific(c)
                if c == u32::from(crate::base::failure::EXIT_CONFIG) =>
            {
                super::config_exit_detail(&m.name)
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

fn read_rights(path: &Path) -> &'static str {
    if path.is_dir() { "(OI)(CI)R" } else { "R" }
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
    fn the_owner_check_works_from_powershell_7() {
        if std::env::var_os("BURROW_OWNER_CHECK_CHILD").is_some() {
            let dir = tempfile::tempdir().unwrap();
            let (owner, me) =
                owner_and_me(dir.path()).expect("the owner of a folder this test made is read");
            assert!(owner == me || trusted_owner(&owner), "{owner} {me}");
            return;
        }
        let pf = PathBuf::from(std::env::var_os("ProgramFiles").unwrap());
        let modules = format!(
            "{};{}",
            pf.join(r"PowerShell\Modules").display(),
            pf.join(r"PowerShell\7\Modules").display()
        );
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::windows::tests::the_owner_check_works_from_powershell_7",
                "--nocapture",
            ])
            .env("BURROW_OWNER_CHECK_CHILD", "1")
            .env("PSModulePath", modules)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "under the module path of PowerShell 7: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn only_burrow_under_program_files_is_trusted() {
        let pf = PathBuf::from(std::env::var_os("ProgramFiles").unwrap());
        assert!(crate::install::trust::trusted_binary(
            &pf.join("Tools").join("burrow.exe")
        ));
        assert!(!crate::install::trust::trusted_binary(
            &pf.join("Tools").join("setup.exe")
        ));
        let around = pf
            .join("..")
            .join("Users")
            .join("Public")
            .join("burrow.exe");
        assert!(!crate::install::trust::trusted_binary(&around));
        let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        assert!(!crate::install::trust::trusted_binary(
            &windows.join("Temp").join("burrow.exe")
        ));
    }
}
