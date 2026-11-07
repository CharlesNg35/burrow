use std::path::{Path, PathBuf};

use super::{Helper, Manifest, helper_name};
use crate::base::failure::Failure;
use crate::base::process::run;

pub struct PathUnit;

fn unit(m: &Manifest, kind: &str) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/system/{}.{kind}", helper_name(m)))
}

fn state(m: &Manifest) -> PathBuf {
    Path::new("/var/lib/burrow").join(m.short())
}

fn updaters(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    let temp = name
        .strip_prefix('.')
        .and_then(|n| n.strip_suffix(crate::base::fs::TEMP_EXTENSION));
    [super::TRIGGER, super::ASKED]
        .into_iter()
        .any(|own| name == own || temp.is_some_and(|t| t.starts_with(&format!("{own}."))))
}

fn clear(dir: &Path) {
    crate::service::nofollow::remove_entries(dir, updaters);
    let _ = std::fs::remove_dir(dir);
}

pub(super) fn render_path(m: &Manifest, dir: &Path) -> String {
    format!(
        "[Unit]\nDescription=burrow {} \"{}\": update requests from the server's admin\n\n[Path]\nPathExists={}\nUnit={}.service\n\n[Install]\nWantedBy=paths.target\n",
        m.role,
        m.short(),
        crate::service::systemd::quote(&dir.join(super::TRIGGER)),
        helper_name(m)
    )
}

pub(super) fn render_service(m: &Manifest) -> String {
    let mut s = format!(
        "[Unit]\nDescription=burrow {} \"{}\": update to a release the server's admin asked for\nDocumentation=https://github.com/charlesng35/burrow/blob/main/docs/updates.md\n\n[Service]\nType=oneshot\nExecStart={} service remote-update {}\nTimeoutStartSec=20min\n",
        m.role,
        m.short(),
        crate::service::systemd::quote_exec(&m.binary),
        m.name
    );
    s.push_str("UMask=0022\nPrivateTmp=yes\nNoNewPrivileges=yes\n");
    s
}

impl Helper for PathUnit {
    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        crate::service::system::write_file(&unit(m, "service"), render_service(m).as_bytes())?;
        crate::service::system::write_file(&unit(m, "path"), render_path(m, &state(m)).as_bytes())?;
        run("systemctl", &["daemon-reload"])?;
        let path = format!("{}.path", helper_name(m));
        run("systemctl", &["enable", "--quiet", "--now", &path])?;
        Ok(())
    }

    fn remove(&self, m: &Manifest) -> Result<(), Failure> {
        let path = format!("{}.path", helper_name(m));
        let _ = run("systemctl", &["disable", "--quiet", "--now", &path]);
        for kind in ["path", "service"] {
            let f = unit(m, kind);
            if f.exists() {
                std::fs::remove_file(&f)
                    .map_err(|e| Failure::new(format!("cannot remove {}: {e}", f.display())))?;
            }
        }
        run("systemctl", &["daemon-reload"])?;
        let service = format!("{}.service", helper_name(m));
        let _ = run("systemctl", &["reset-failed", &path, &service]);
        clear(&state(m));
        Ok(())
    }

    fn installed(&self, m: &Manifest) -> bool {
        unit(m, "path").exists()
    }

    fn trigger(&self, m: &Manifest) -> Option<PathBuf> {
        Some(state(m).join(super::TRIGGER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_the_updater_removes_only_its_own_files_and_keeps_web_state_left_there() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, "keep").unwrap();
        let dir = root.path().join("home");
        std::fs::create_dir_all(dir.join("letsencrypt")).unwrap();
        std::fs::write(dir.join("letsencrypt/account.key"), "k").unwrap();
        std::fs::write(dir.join("ca.key"), "k").unwrap();
        std::fs::write(dir.join(super::super::TRIGGER), "").unwrap();
        std::fs::write(dir.join(".asked.json.7-0.tmp"), "{}").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(super::super::ASKED)).unwrap();
        clear(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["ca.key", "letsencrypt"]);
        assert!(dir.join("letsencrypt/account.key").exists());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        std::fs::remove_dir_all(dir.join("letsencrypt")).unwrap();
        std::fs::remove_file(dir.join("ca.key")).unwrap();
        std::fs::write(dir.join(super::super::ASKED), "{}").unwrap();
        clear(&dir);
        assert!(
            !dir.exists(),
            "a folder that held only the updater's files goes"
        );
    }

    #[test]
    fn the_path_unit_starts_the_root_updater_when_the_client_writes_a_request() {
        let mut m = crate::service::stray::bare("burrow-nas");
        m.role = crate::install::Role::Client;
        m.binary = PathBuf::from("/usr/local/bin/burrow");
        m.env_file = Some(PathBuf::from("/etc/burrow/nas.env"));
        let path = render_path(&m, Path::new("/var/lib/burrow/nas"));
        assert!(
            path.contains("PathExists=/var/lib/burrow/nas/update-now\n"),
            "{path}"
        );
        assert!(path.contains("Unit=burrow-nas-update.service\n"), "{path}");
        let service = render_service(&m);
        assert!(service.contains("Type=oneshot\n"), "{service}");
        assert!(
            service.contains("ExecStart=/usr/local/bin/burrow service remote-update burrow-nas\n"),
            "{service}"
        );
        assert!(
            !service.contains("EnvironmentFile"),
            "the updater reads the env file itself, after checking who owns it: {service}"
        );
        assert!(
            !service.contains("User="),
            "the updater runs as root: {service}"
        );
    }
}
