use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::{Helper, Manifest, helper_name};
use crate::base::failure::Failure;
use crate::base::process::output_of;

pub struct StartableService;

const START_AND_QUERY: &str = "RPLCRC";

fn sc() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join(r"System32\sc.exe")
}

fn sid_of(account: &str) -> Result<String, Failure> {
    let script = format!(
        "(New-Object System.Security.Principal.NTAccount('{}')).Translate([System.Security.Principal.SecurityIdentifier]).Value",
        account.replace('\'', "''")
    );
    let exe = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let sid = output_of(
        Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .env_remove("PSModulePath"),
    )?;
    let sid = sid.trim().to_string();
    if !sid.starts_with("S-1-") || sid.contains([';', ')', '(']) {
        return Err(Failure::new(format!(
            "{account} has no usable security id: {sid}"
        )));
    }
    Ok(sid)
}

pub(super) fn with_start_for(sddl: &str, sid: &str) -> String {
    let ace = format!("(A;;{START_AND_QUERY};;;{sid})");
    if sddl.contains(&ace) {
        return sddl.to_string();
    }
    match sddl.find("S:") {
        Some(at) => format!("{}{ace}{}", &sddl[..at], &sddl[at..]),
        None => format!("{sddl}{ace}"),
    }
}

fn info(m: &Manifest) -> ServiceInfo {
    ServiceInfo {
        name: OsString::from(helper_name(m)),
        display_name: OsString::from(format!("burrow {} {} updater", m.role, m.short())),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::OnDemand,
        error_control: ServiceErrorControl::Normal,
        executable_path: m.binary.clone(),
        launch_arguments: vec!["service".into(), "run".into(), helper_name(m).into()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    }
}

impl Helper for StartableService {
    fn install(&self, m: &Manifest, dir: &Path) -> Result<(), Failure> {
        let scm = crate::service::windows::manager(
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;
        let access = crate::service::windows::ALL;
        let service = match scm.open_service(helper_name(m), access) {
            Ok(s) => {
                s.change_config(&info(m))
                    .map_err(|e| crate::service::windows::scm_error("update the updater", e))?;
                s
            }
            Err(_) => scm
                .create_service(&info(m), access)
                .map_err(|e| crate::service::windows::scm_error("create the updater", e))?,
        };
        let _ = service.set_description(format!(
            "updates burrow {} when the server's admin asks and {} allows it",
            m.short(),
            m.config.display()
        ));
        crate::service::windows::grant_update_folder(dir, &m.account)?;
        let sid = sid_of(&m.account)?;
        let name = helper_name(m);
        let now = output_of(Command::new(sc()).args(["sdshow", &name]))?;
        let wanted = with_start_for(now.trim(), &sid);
        output_of(Command::new(sc()).args(["sdset", &name, &wanted]))?;
        Ok(())
    }

    fn remove(&self, m: &Manifest) -> Result<(), Failure> {
        let scm = crate::service::windows::manager(ServiceManagerAccess::CONNECT)?;
        if let Ok(s) = scm.open_service(helper_name(m), ServiceAccess::DELETE) {
            s.delete()
                .map_err(|e| crate::service::windows::scm_error("remove the updater", e))?;
        }
        Ok(())
    }

    fn installed(&self, m: &Manifest) -> bool {
        crate::service::windows::manager(ServiceManagerAccess::CONNECT).is_ok_and(|scm| {
            scm.open_service(helper_name(m), ServiceAccess::QUERY_STATUS)
                .is_ok()
        })
    }

    fn fire(&self, m: &Manifest) -> Result<(), String> {
        let scm = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| format!("cannot reach the service manager: {e}"))?;
        let s = scm
            .open_service(helper_name(m), ServiceAccess::START)
            .map_err(|e| format!("cannot open {}: {e}", helper_name(m)))?;
        const ALREADY_RUNNING: i32 = 1056;
        match s.start::<&OsStr>(&[]) {
            Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(ALREADY_RUNNING) => {
                Ok(())
            }
            other => other.map_err(|e| format!("cannot start {}: {e}", helper_name(m))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_start_and_status_are_added_for_the_client_account() {
        let sid = "S-1-5-80-1-2-3-4-5";
        let base = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)";
        let got = with_start_for(base, sid);
        assert_eq!(
            got,
            "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;RPLCRC;;;S-1-5-80-1-2-3-4-5)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)"
        );
        assert_eq!(with_start_for(&got, sid), got);
        assert!(
            with_start_for("D:(A;;GA;;;SY)", sid).ends_with("(A;;RPLCRC;;;S-1-5-80-1-2-3-4-5)")
        );
    }
}
