use std::path::{Path, PathBuf};

use super::{Helper, Manifest, helper_name};
use crate::base::failure::Failure;
use crate::base::process::run;
use crate::service::launchd::xml;

pub struct QueueJob;

fn label(m: &Manifest) -> String {
    format!("com.github.charlesng35.{}", helper_name(m))
}

fn plist_path(m: &Manifest) -> PathBuf {
    PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", label(m)))
}

fn queue(m: &Manifest) -> PathBuf {
    super::dir(m).join("queue")
}

pub(super) fn plist(m: &Manifest, queue: &Path) -> String {
    let args = [
        m.binary.display().to_string(),
        "service".into(),
        "remote-update".into(),
        m.name.clone(),
    ];
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s.push_str(&format!(
        "  <key>Label</key>\n  <string>{}</string>\n  <key>ProgramArguments</key>\n  <array>\n",
        xml(&label(m))
    ));
    for a in &args {
        s.push_str(&format!("    <string>{}</string>\n", xml(a)));
    }
    s.push_str(&format!(
        "  </array>\n  <key>QueueDirectories</key>\n  <array>\n    <string>{}</string>\n  </array>\n",
        xml(&queue.display().to_string())
    ));
    s.push_str("  <key>ThrottleInterval</key>\n  <integer>10</integer>\n  <key>Umask</key>\n  <integer>18</integer>\n</dict>\n</plist>\n");
    s
}

impl Helper for QueueJob {
    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        let owner = match m.account.as_str() {
            "root" => None,
            a => Some(crate::base::process::account_ids(a)?),
        };
        crate::service::nofollow::subfolder(&super::dir(m), "queue", 0o700, owner)?;
        let target = format!("system/{}", label(m));
        let _ = run("launchctl", &["bootout", &target]);
        let path = plist_path(m);
        crate::service::system::write_private(&path, &plist(m, &queue(m)))?;
        crate::service::system::private_file(&path, None)?;
        run("launchctl", &["enable", &target])?;
        run(
            "launchctl",
            &["bootstrap", "system", &path.display().to_string()],
        )?;
        Ok(())
    }

    fn remove(&self, m: &Manifest) -> Result<(), Failure> {
        let _ = run("launchctl", &["bootout", &format!("system/{}", label(m))]);
        let path = plist_path(m);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| Failure::new(format!("cannot remove {}: {e}", path.display())))?;
        }
        Ok(())
    }

    fn installed(&self, m: &Manifest) -> bool {
        plist_path(m).exists()
    }

    fn trigger(&self, m: &Manifest) -> Option<PathBuf> {
        Some(queue(m).join(super::TRIGGER))
    }

    fn take_trigger(&self, m: &Manifest) {
        crate::service::nofollow::empty_folder(&queue(m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_runs_as_root_while_the_queue_holds_a_request() {
        let mut m = crate::service::stray::bare("burrow-nas");
        m.binary = PathBuf::from("/usr/local/bin/burrow");
        let p = plist(
            &m,
            Path::new("/Library/Application Support/burrow/updates/nas/queue"),
        );
        assert!(
            p.contains("<string>com.github.charlesng35.burrow-nas-update</string>"),
            "{p}"
        );
        assert!(p.contains("<key>QueueDirectories</key>"), "{p}");
        assert!(
            p.contains("<string>remote-update</string>\n    <string>burrow-nas</string>"),
            "{p}"
        );
        assert!(!p.contains("UserName"), "{p}");
    }
}
