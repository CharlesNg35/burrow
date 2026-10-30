#[cfg(target_os = "linux")]
use super::checks::misfits_of;
use super::checks::needs_low_ports;
use super::control::{interrupts, source_note};
use super::install::{burrows_account, name_arg};
#[cfg(unix)]
use super::install::{installs_from, place_binary, recorded_source, service_binary_for};
use super::previous::{StartAgain, Undo, given_note, start_installed, take_over};
use super::uninstall::differs;
use super::uninstall::{Kept, keep_admin_copy};
use super::*;
use crate::install::{in_service_folder, trustworthy, writable_hint};
use std::net::SocketAddr;

#[test]
fn an_installed_config_that_cannot_be_read_is_not_called_different() {
    let dir = tempfile::tempdir().unwrap();
    let (source, installed) = (dir.path().join("a.yaml"), dir.path().join("b.yaml"));
    std::fs::write(&source, "listen: 1\n").unwrap();
    assert!(!differs(&source, &dir.path().join("unreadable.yaml")));
    std::fs::write(&installed, "listen: 1\n").unwrap();
    assert!(!differs(&source, &installed));
    std::fs::write(&installed, "listen: 2\n").unwrap();
    assert!(differs(&source, &installed));
}

#[test]
fn a_delete_hint_names_a_kept_folder_and_not_the_files_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("e2e-admin");
    let config = folder.join("server.yaml");
    let log = dir.path().join("e2e-admin.log");
    let hint = delete_hint(&[config.clone(), log.clone()], Some(&folder));
    let q = |p: &Path| format!("\"{}\"", p.display());
    assert!(hint.contains(&q(&folder)), "{hint}");
    assert!(hint.contains(&q(&log)), "{hint}");
    assert!(!hint.contains(&q(&config)), "{hint}");
    assert!(
        hint.contains(if cfg!(windows) { "-Recurse" } else { "rm -r" }),
        "{hint}"
    );
    let plain = delete_hint(&[log], None);
    assert!(!plain.contains("-r"), "{plain}");
}

#[cfg(unix)]
#[test]
fn a_config_the_account_swapped_for_a_link_is_never_read_into_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("server.yaml");
    let folder = dir.path().join("server");
    std::fs::create_dir(&folder).unwrap();
    let config = folder.join("server.yaml");
    let private = dir.path().join("shadow");
    std::fs::write(&source, "listen: 1\n").unwrap();
    std::fs::write(&private, "root:secret-hash:19000::::::\n").unwrap();
    std::os::unix::fs::symlink(&private, &config).unwrap();
    assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::NotPlain);
    std::fs::remove_file(&config).unwrap();
    std::fs::hard_link(&private, &config).unwrap();
    assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::NotPlain);
    let history = crate::admin::adapters::history_dir_of(&config);
    let kept: Vec<String> = std::fs::read_dir(&history)
        .map(|d| {
            d.flatten()
                .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(!kept.iter().any(|t| t.contains("secret-hash")), "{kept:?}");
}

#[test]
fn a_config_changed_by_burrow_admin_is_kept_before_a_reinstall_replaces_it() {
    use crate::admin::core::ports::Store;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("server.yaml");
    let folder = dir.path().join("server");
    std::fs::create_dir(&folder).unwrap();
    let config = folder.join("server.yaml");
    std::fs::write(&source, "listen: 1\n").unwrap();
    assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Nothing);
    std::fs::write(&config, "listen: 1\n").unwrap();
    assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Nothing);
    let changed = "listen: 1\nclients:\n  laptop: {}\n";
    std::fs::write(&config, changed).unwrap();
    assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Version);
    let store = crate::admin::adapters::FileStore::new(&config);
    let rev = crate::admin::core::text::rev(changed);
    assert!(store.versions().iter().any(|v| v.rev == rev));
    assert_eq!(store.version(&rev).as_deref(), Some(changed));

    let m = Manifest {
        config: config.clone(),
        source_config: source.clone(),
        admin_writable: true,
        ..record(Platform::Linux)
    };
    let note = source_note(&m).unwrap();
    assert!(note.contains("managed by burrow admin"), "{note}");
    assert!(!note.contains("install"), "{note}");
    let fixed = Manifest {
        admin_writable: false,
        ..m.clone()
    };
    let note = source_note(&fixed).unwrap();
    let from = format!(
        "to use it, run: {}",
        crate::base::failure::admin(&format!(
            "service install -c {}",
            crate::base::failure::path_arg(&source)
        ))
    );
    assert!(note.contains(&from), "{note}");
    std::fs::write(&config, "listen: 1\n").unwrap();
    assert!(source_note(&m).is_none());
}

#[cfg(unix)]
#[test]
fn a_reinstall_from_the_installed_config_is_recognised_through_a_linked_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("private-etc");
    std::fs::create_dir_all(real.join("burrow/server")).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("etc")).unwrap();
    let installed = tmp.path().join("etc/burrow/server/server.yaml");
    std::fs::write(&installed, "listen: 1\n").unwrap();
    let m = Manifest {
        config: installed.clone(),
        source_config: tmp.path().join("gone/server.yaml"),
        ..record(Platform::MacOs)
    };
    let source = crate::base::fs::canonical(&installed).unwrap();
    assert_ne!(source, installed, "the folder is reached through a link");
    assert!(installs_from(&m, &source));
    assert_eq!(recorded_source(Some(&m), &source), m.source_config);
    assert_eq!(recorded_source(None, &source), source);
    let elsewhere = tmp.path().join("other.yaml");
    std::fs::write(&elsewhere, "listen: 1\n").unwrap();
    assert!(!installs_from(
        &m,
        &crate::base::fs::canonical(&elsewhere).unwrap()
    ));
}

#[cfg(unix)]
#[test]
fn a_source_recorded_through_a_linked_parent_is_still_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let real = crate::base::fs::canonical(dir.path())
        .unwrap()
        .join("private-etc");
    std::fs::create_dir_all(real.join("burrow/x")).unwrap();
    std::fs::write(real.join("burrow/x.yaml"), "listen: 1\n").unwrap();
    std::fs::write(real.join("burrow/x/server.yaml"), "listen: 1\n").unwrap();
    let etc = dir.path().join("etc");
    std::os::unix::fs::symlink(&real, &etc).unwrap();
    let source = crate::base::fs::canonical(&etc.join("burrow/x.yaml")).unwrap();
    assert_ne!(source, etc.join("burrow/x.yaml"));
    assert!(is_source(&etc.join("burrow/x.yaml"), &[&source]));
    assert!(!is_source(&etc.join("burrow/x/server.yaml"), &[&source]));
}

#[cfg(unix)]
#[test]
fn a_binary_in_a_private_folder_is_copied_into_a_missing_service_folder_once() {
    system::tests::under_umask_077(
        "service::tests::a_binary_in_a_private_folder_is_copied_into_a_missing_service_folder_once",
        || {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            let private = dir.path().join("root");
            std::fs::create_dir(&private).unwrap();
            let current = private.join("burrow");
            std::fs::write(&current, "the burrow binary").unwrap();
            std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
            let usual = dir.path().join("bin/burrow");
            assert!(!usual.parent().unwrap().exists());
            place_binary(&current, &usual, &usual).unwrap();
            let folder = std::fs::metadata(usual.parent().unwrap()).unwrap();
            assert_eq!(folder.permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::read(&usual).unwrap(), b"the burrow binary");
            assert!(std::fs::metadata(&usual).unwrap().permissions().mode() & 0o001 != 0);
            assert!(
                system::same_content(&current, &usual),
                "a reinstall from the same file has nothing to copy"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn a_trusted_binary_elsewhere_is_never_used_in_place() {
    let trusted = Path::new("/usr/bin/true");
    assert!(
        crate::install::trust::trusted_binary(trusted),
        "/usr/bin/true is root-only here"
    );
    let p = Platform::current();
    let (binary, copied) = service_binary_for(trusted, p);
    assert_eq!(binary, p.service_binary());
    assert!(copied);
    #[cfg(target_os = "linux")]
    {
        let unit = systemd::Systemd.render(&Manifest {
            binary: binary.clone(),
            ..record(Platform::Linux)
        });
        assert!(
            unit.contains(&format!("ExecStart={} ", binary.display())),
            "{unit}"
        );
        assert!(!unit.contains("/usr/bin/true"), "{unit}");
    }
}

#[test]
fn commands_that_interrupt_a_service_go_to_the_background() {
    assert!(interrupts("restart"));
    assert!(!interrupts("start"));
    assert!(!interrupts("stop"));
    assert_eq!(interrupts("reload"), cfg!(windows));
}

struct Recorder(std::cell::RefCell<Vec<String>>, bool);

impl Recorder {
    fn note(&self, what: &str, m: &Manifest) -> Result<(), Failure> {
        self.0.borrow_mut().push(format!("{what} {}", m.account));
        Ok(())
    }
}

impl Backend for Recorder {
    fn kind(&self) -> &'static str {
        "recorder"
    }
    fn render(&self, _: &Manifest) -> String {
        String::new()
    }
    fn ensure_account(&self, _: &str) -> Result<bool, Failure> {
        Ok(false)
    }
    fn remove_account(&self, account: &str) -> Result<(), Failure> {
        self.0.borrow_mut().push(format!("remove {account}"));
        Ok(())
    }
    fn can_read(&self, _: &str, _: &Path) -> bool {
        true
    }
    fn secure_file(&self, _: &Path, _: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn config_folder(
        &self,
        _: &Path,
        _: &Path,
        account: &str,
        writable: bool,
    ) -> Result<(), Failure> {
        self.0
            .borrow_mut()
            .push(format!("folder writable={writable} {account}"));
        Ok(())
    }
    fn prepare_dirs(&self, _: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
        self.note(&format!("env {}", env.len()), m)
    }
    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        self.note("install", m)
    }
    fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
        self.note("uninstall", m)
    }
    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        self.note("start", m)
    }
    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        self.note("stop", m)
    }
    fn reload(&self, _: &Manifest) -> Result<(), Failure> {
        Ok(())
    }
    fn status(&self, _: &Manifest) -> Status {
        Status {
            installed: true,
            running: self.1,
            pid: None,
            detail: String::new(),
        }
    }
}

#[test]
fn the_hints_after_an_install_name_the_service_when_anything_else_is_installed() {
    assert_eq!(name_arg("burrow-home", 1, 0), "");
    assert_eq!(name_arg("burrow-home", 2, 0), " burrow-home");
    assert_eq!(name_arg("burrow-home", 1, 1), " burrow-home");
}

#[test]
fn an_install_never_says_it_started_a_service_that_was_already_running() {
    let m = record(Platform::Linux);
    let rec = Recorder(Default::default(), true);
    let mut out = Vec::new();
    let mut again = StartAgain {
        backend: &rec,
        manifest: None,
    };
    start_installed(&rec, &m, "", &mut again, None, |_| Ok(()), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with("warning: burrow-server was already running and was not restarted"),
        "{out}"
    );
    assert!(out.contains("service restart"), "{out}");
    assert!(!out.contains("started burrow-server"), "{out}");
    assert!(rec.0.borrow().is_empty());
    let stopped = Recorder(Default::default(), false);
    let mut out = Vec::new();
    let mut again = StartAgain {
        backend: &stopped,
        manifest: None,
    };
    start_installed(&stopped, &m, "", &mut again, None, |_| Ok(()), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(*stopped.0.borrow(), ["start burrow"]);
    assert!(
        out.starts_with("warning: burrow-server is not running"),
        "{out}"
    );
    assert!(!out.contains("already running"), "{out}");
}

#[test]
fn an_install_records_burrows_own_account_that_an_earlier_install_left_behind() {
    let p = Platform::Linux;
    let ours = p.default_account("burrow-server");
    let (shaped, other) = (|| true, || false);
    let previous = Manifest {
        created_account: true,
        ..record(p)
    };
    let elsewhere = Manifest {
        account: "www".into(),
        ..previous.clone()
    };
    assert!(burrows_account(true, &ours, &ours, None, other));
    assert!(burrows_account(false, &ours, &ours, Some(&previous), other));
    assert!(burrows_account(false, &ours, &ours, None, shaped));
    assert!(!burrows_account(false, &ours, &ours, None, other));
    assert!(!burrows_account(false, "www", &ours, None, shaped));
    assert!(!burrows_account(
        false,
        &ours,
        &ours,
        Some(&elsewhere),
        other
    ));
}

#[test]
fn an_install_refuses_a_running_unit_it_has_no_record_of_and_replaces_a_stopped_one() {
    let s = stray::Stray {
        name: "burrow-client".into(),
        label: "burrow-client.service".into(),
        config: Some(PathBuf::from("/etc/burrow/client.yaml")),
        running: true,
        pid: Some(42),
        stop: "sudo systemctl stop burrow-client".into(),
    };
    let new = Path::new("/etc/burrow/client/client.yaml");
    assert_eq!(take_over(Some(&s), new).unwrap_err(), s.failure());
    let stopped = stray::Stray {
        running: false,
        pid: None,
        ..s.clone()
    };
    let taken = take_over(Some(&stopped), new).unwrap().unwrap();
    assert_eq!(
        taken.note(&[]),
        "replaced burrow-client.service, which burrow had no record of; its old config was left as it was: /etc/burrow/client.yaml"
    );
    let old = PathBuf::from("/etc/burrow/client.yaml");
    assert_eq!(
        taken.note(std::slice::from_ref(&old)),
        "replaced burrow-client.service, which burrow had no record of; its old config was left as it was, now owned by root: /etc/burrow/client.yaml"
    );
    assert_eq!(taken.old_files(), vec![old.clone()]);
    let in_place = stray::Stray {
        config: Some(new.to_path_buf()),
        ..stopped.clone()
    };
    let taken = take_over(Some(&in_place), new).unwrap().unwrap();
    assert_eq!(
        taken.note(&[]),
        "replaced burrow-client.service, which burrow had no record of"
    );
    assert!(taken.old_files().is_empty());
    assert!(take_over(None, new).unwrap().is_none());
    let env = PathBuf::from("/etc/burrow/client.env");
    assert_eq!(given_note("burrow-client", &[old.clone(), env.clone()], Some(&old)), Some("gave /etc/burrow/client.env, left by an older install of burrow-client, to root, since the service does not use it".to_string()));
    assert_eq!(
        given_note("burrow-client", std::slice::from_ref(&old), Some(&old)),
        None
    );
    assert_eq!(given_note("burrow-client", &[], None), None);
}

#[test]
fn a_reinstall_that_does_not_run_puts_the_previous_one_back() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("home.yaml");
    std::fs::write(&config, "new config").unwrap();
    let old = Manifest {
        config: config.clone(),
        env_file: Some(dir.path().join("home.env")),
        account: "old".into(),
        ..record(Platform::Linux)
    };
    let new_env = dir.path().join("home-new.env");
    std::fs::write(&new_env, "SECRET=y").unwrap();
    let new = Manifest {
        config: config.clone(),
        env_file: Some(new_env.clone()),
        account: "new".into(),
        created_account: true,
        admin_writable: true,
        ..record(Platform::Linux)
    };
    let binary = dir.path().join("burrow");
    let kept = dir.path().join("burrow.previous");
    std::fs::write(&binary, "new binary").unwrap();
    std::fs::write(&kept, "old binary").unwrap();
    let undo = Undo {
        manifest: old.clone(),
        config: Some(b"old config".to_vec()),
        env: [("SECRET".to_string(), "x".to_string())].into(),
        binary: Some((kept.clone(), binary.clone())),
    };
    let rec = Recorder(Default::default(), true);
    let saved = std::cell::RefCell::new(Vec::new());
    let mut out = Vec::new();
    let f = undo.put_back(
        &rec,
        &new,
        "exited with status 3",
        |m| {
            saved.borrow_mut().push(m.account.clone());
            Ok(())
        },
        &mut out,
    );
    assert!(f.message.contains("runs again"), "{}", f.message);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "old config");
    assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old binary");
    assert!(!kept.exists());
    assert!(!new_env.exists());
    assert_eq!(
        *rec.0.borrow(),
        [
            "stop new",
            "folder writable=false old",
            "env 1 old",
            "install old",
            "start old",
            "remove new"
        ]
    );
    assert_eq!(*saved.borrow(), ["old"]);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("exited with status 3")
    );
}

#[test]
fn low_ports_on_one_address_need_root_on_macos_only_away_from_the_wildcard() {
    let at = |a: &str| a.parse::<SocketAddr>().unwrap();
    let rec = Recorder(Default::default(), true);
    assert!(needs_low_ports(&rec, &at("0.0.0.0:80")));
    assert!(needs_low_ports(&rec, &at("127.0.0.1:80")));
    assert!(!needs_low_ports(&rec, &at("127.0.0.1:8080")));
    #[cfg(target_os = "macos")]
    {
        let mac = crate::service::launchd::Launchd;
        assert!(!needs_low_ports(&mac, &at("0.0.0.0:80")));
        assert!(needs_low_ports(&mac, &at("127.0.0.1:80")));
        let m = Manifest {
            account: "_burrow".into(),
            ..record(Platform::MacOs)
        };
        assert!(!mac.low_ports(&m));
    }
}

#[test]
fn a_put_back_that_does_not_run_either_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let old = Manifest {
        config: dir.path().join("home.yaml"),
        env_file: None,
        ..record(Platform::Linux)
    };
    let undo = Undo {
        manifest: old.clone(),
        config: Some(b"old config".to_vec()),
        env: BTreeMap::new(),
        binary: None,
    };
    let f = undo.put_back(
        &Recorder(Default::default(), false),
        &old,
        "exited with status 3",
        |_| Ok(()),
        &mut Vec::new(),
    );
    assert!(f.message.contains("does not run either"), "{}", f.message);
}

fn record(p: Platform) -> Manifest {
    Manifest {
        name: "burrow-server".into(),
        role: "server".into(),
        config: p.config_dir().join("server").join("server.yaml"),
        source_config: PathBuf::from("server.yaml"),
        env_file: Some(p.config_dir().join("server.env")),
        log_file: p.log_file("server"),
        binary: p.service_binary(),
        copied_binary: true,
        account: p.default_account("burrow-server"),
        created_account: true,
        low_ports: false,
        files: Vec::new(),
        version: "0.1.0".into(),
        admin_writable: false,
        web_state: None,
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_edit_the_installed_unit_cannot_run_is_named_with_the_way_out() {
    let secret = crate::wire::secret::Secret::generate().encode();
    let load = |text: String| {
        let nothing = |_: &str| None;
        let o =
            crate::config::load_str("c.yaml", &text, crate::config::Format::Yaml, None, &nothing);
        let shown = o
            .report
            .render_text(crate::base::style::Style { color: false });
        o.loaded.unwrap_or_else(|| panic!("{shown}"))
    };
    let server = |listen: &str| {
        load(format!(
            "listen: {listen}\nclients:\n  home: {{ secret: {secret} }}\nservices:\n  web: {{ client: home, bind: 8080, target: 80 }}\n"
        ))
    };
    let m = Manifest {
        name: "burrow-edge".into(),
        source_config: PathBuf::from("/srv/edge.yaml"),
        ..record(Platform::Linux)
    };
    let backend = systemd::Systemd;
    assert!(misfits_of(&backend, &m, &server("7835")).is_empty());
    let low = misfits_of(&backend, &m, &server("443"));
    assert_eq!(low.len(), 1);
    assert!(
        low[0].message.contains("listen on port 443"),
        "{}",
        low[0].message
    );
    assert!(
        low[0].help.as_deref().unwrap().contains("/srv/edge.yaml")
            && low[0]
                .help
                .as_deref()
                .unwrap()
                .contains("service install -c /srv/edge.yaml"),
        "{:?}",
        low[0].help
    );
    let allowed = Manifest {
        low_ports: true,
        ..m.clone()
    };
    assert!(misfits_of(&backend, &allowed, &server("443")).is_empty());
    let web = load(format!(
        "listen: 7835\nclients:\n  home: {{ secret: {secret} }}\nweb: {{ https: 443 }}\nservices:\n  nas: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: passthrough }} }}\n"
    ));
    let stateful = Manifest {
        web_state: Some(PathBuf::from("/var/lib/burrow/edge")),
        ..m.clone()
    };
    let low = misfits_of(&backend, &stateful, &web);
    assert_eq!(low.len(), 1, "web.https counts as a low port");
    assert!(low[0].message.contains("port 443"), "{}", low[0].message);
    let home = backend.cannot_use(&m, Path::new("/home/someone/ca.pem"));
    assert!(home.unwrap().contains("home folder"));
    assert!(
        backend
            .cannot_use(&m, Path::new("ca.pem"))
            .unwrap()
            .contains("absolute")
    );
    let private = backend.cannot_use(&m, Path::new("/tmp/ca.pem"));
    assert!(private.unwrap().contains("private"));
    assert!(
        backend
            .cannot_use(&m, Path::new("/var/tmp/x/ca.pem"))
            .is_some()
    );
    let as_root = Manifest {
        files: vec![PathBuf::from("/home/someone/old.pem")],
        account: "root".into(),
        ..m.clone()
    };
    assert_eq!(
        backend
            .cannot_use(&as_root, Path::new("/home/someone/ca.pem"))
            .map(|_| ()),
        Some(())
    );
    assert_eq!(
        backend.cannot_use(&as_root, Path::new("/etc/hostname")),
        None
    );
    assert!(
        backend
            .cannot_use(&as_root, Path::new("/etc/burrow-no-such.pem"))
            .unwrap()
            .contains("cannot read it")
    );
}

#[test]
fn a_config_is_trusted_only_in_the_service_s_own_folder_whether_writable_or_not() {
    let p = Platform::current();
    let file = manifest_path("burrow-server");
    let own = p.config_dir().join("server");
    for writable in [false, true] {
        let m = Manifest {
            admin_writable: writable,
            ..record(p)
        };
        assert_eq!(m.folder(), own);
        assert!(trustworthy(&m, &file));
        let planted = [
            std::env::temp_dir().join("server").join("server.yaml"),
            p.config_dir().join("other").join("server.yaml"),
            p.config_dir().join("server.yaml"),
            own.join("deeper").join("server.yaml"),
        ];
        for config in planted {
            let m = Manifest {
                config,
                admin_writable: writable,
                ..record(p)
            };
            assert!(!trustworthy(&m, &file), "{m:?}");
        }
    }
}

#[test]
fn the_install_command_for_a_service_is_its_installed_config() {
    let p = Platform::current();
    let m = record(p);
    let config = crate::base::failure::path_arg(&m.config);
    assert_eq!(
        install_args(&m, &m.config),
        format!("service install -c {config}")
    );
    assert_eq!(
        writable_args(&m),
        format!("service install -c {config} --admin-writable")
    );
    let client = Manifest {
        name: "burrow-home".into(),
        role: "client".into(),
        config: p.config_dir().join("home").join("client.yaml"),
        ..record(p)
    };
    assert_eq!(
        install_args(&client, &client.config),
        format!(
            "service install -c {}",
            crate::base::failure::path_arg(&client.config)
        )
    );
    assert_eq!(
        install_args(&client, Path::new("/srv/office.yaml")),
        "service install -c /srv/office.yaml --name burrow-home"
    );
}

#[test]
fn the_read_only_hint_names_the_command_for_the_installed_config() {
    let config = Platform::current()
        .config_dir()
        .join("edge")
        .join("server.yaml");
    assert!(in_service_folder(&config));
    assert!(!in_service_folder(Path::new("/srv/edge/server.yaml")));
    assert!(!in_service_folder(
        &Platform::current().config_dir().join("server.yaml")
    ));
    let hint = writable_hint(&config);
    let command = crate::base::failure::admin(&format!(
        "service install -c {} --admin-writable",
        crate::base::failure::path_arg(&config)
    ));
    assert!(hint.ends_with(&command), "{hint}");
    assert!(hint.contains("nothing moves"), "{hint}");
}

#[test]
fn installing_from_an_installed_config_reinstalls_its_own_service() {
    let p = Platform::current();
    let home = Manifest {
        name: "burrow-home".into(),
        role: "client".into(),
        config: p.config_dir().join("home").join("client.yaml"),
        ..record(p)
    };
    let all = [record(p), home.clone()];
    assert_eq!(name_for(&home.config, None, &all).unwrap(), "burrow-home");
    assert_eq!(
        name_for(&home.config, Some("home"), &all).unwrap(),
        "burrow-home"
    );
    let other = name_for(&home.config, Some("office"), &all).unwrap_err();
    assert!(
        other
            .message
            .contains("is the installed config of burrow-home"),
        "{}",
        other.message
    );
    assert_eq!(
        name_for(Path::new("/srv/client.yaml"), None, &all).unwrap(),
        "burrow-client"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_admin_visitor_on_a_read_only_install_is_named() {
    let secret = |_| crate::wire::secret::Secret::generate().encode();
    let text = format!(
        "listen: 7835\nclients:\n  home: {{ secret: {} }}\nvisitors:\n  ops: {{ secret: {}, admin: write }}\nservices:\n  web: {{ client: home, bind: 8080, target: 80 }}\n",
        secret(0),
        secret(1)
    );
    let nothing = |_: &str| None;
    let loaded =
        crate::config::load_str("c.yaml", &text, crate::config::Format::Yaml, None, &nothing)
            .loaded
            .unwrap();
    let backend = systemd::Systemd;
    let m = record(Platform::Linux);
    let found = misfits_of(&backend, &m, &loaded);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "service-admin-read-only");
    assert!(!found[0].is_error());
    assert!(found[0].message.contains("visitor ops has admin access"));
    let help = found[0].help.as_deref().unwrap();
    assert!(
        help.contains("service install -c /etc/burrow/server/server.yaml --admin-writable"),
        "{help}"
    );
    let writable = Manifest {
        admin_writable: true,
        ..m
    };
    assert!(misfits_of(&backend, &writable, &loaded).is_empty());
}

#[test]
fn only_records_inside_burrow_folders_are_trusted() {
    let p = Platform::current();
    let file = manifest_path("burrow-server");
    assert!(trustworthy(&record(p), &file));
    let outside = std::env::temp_dir().join("evil");
    let planted: [fn(&mut Manifest, &Path); 5] = [
        |m, o| m.binary = o.to_path_buf(),
        |m, o| m.config = o.to_path_buf(),
        |m, o| m.env_file = Some(o.to_path_buf()),
        |m, o| m.log_file = o.to_path_buf(),
        |m, _| m.name = "other".into(),
    ];
    for change in planted {
        let mut m = record(p);
        change(&mut m, &outside);
        assert!(!trustworthy(&m, &file), "{m:?}");
    }
    assert!(!trustworthy(&record(p), &manifest_path("burrow-client")));
}

#[cfg(unix)]
#[test]
fn a_record_whose_bin_folder_users_can_now_change_is_managed_but_its_binary_never_runs() {
    let p = Platform::current();
    let usual = p.binary_path();
    crate::install::trust::tests::as_if_users_can_change(usual.parent().unwrap(), || {
        assert_ne!(p.service_binary(), usual);
        let file = manifest_path("burrow-server");
        let installed = Manifest {
            binary: usual.clone(),
            ..record(p)
        };
        assert!(trustworthy(&installed, &file));
        assert!(trustworthy(&record(p), &file));
        let mut forged = installed.clone();
        forged.binary = std::env::temp_dir().join("burrow");
        assert!(!trustworthy(&forged, &file));
        assert!(
            recorded_version(&installed).contains("users other than root can change"),
            "{}",
            recorded_version(&installed)
        );
        assert_eq!(service_binary_for(&usual, p).0, p.service_binary());
    });
}

#[test]
fn names_come_from_the_config_file() {
    assert_eq!(
        service_name(Path::new("/x/server.yaml"), None).unwrap(),
        "burrow-server"
    );
    assert_eq!(
        service_name(Path::new("/x/Office Client.json"), None).unwrap(),
        "burrow-office-client"
    );
    assert_eq!(
        service_name(Path::new("/x/server.yaml"), Some("edge")).unwrap(),
        "burrow-edge"
    );
    assert_eq!(
        service_name(Path::new("/x/s.yaml"), Some("burrow-edge")).unwrap(),
        "burrow-edge"
    );
    assert!(service_name(Path::new("/x/s.yaml"), Some("!!!")).is_err());
}

#[test]
fn a_service_never_takes_the_name_of_one_of_burrow_s_own_folders() {
    let p = Platform::Windows;
    let root = p.config_dir();
    let own: Vec<PathBuf> = [
        p.state_dir(),
        p.jobs_dir(),
        p.log_dir(),
        p.state_dir().with_file_name("bin"),
        root.join("instances"),
    ]
    .into_iter()
    .filter(|d| d.parent() == Some(root.as_path()))
    .collect();
    assert_eq!(own.len(), 5);
    for dir in own {
        let short = dir.file_name().unwrap().to_string_lossy().into_owned();
        for given in [
            Path::new("/x").join(format!("{short}.yaml")),
            Path::new("/x").join(format!("{}.yaml", short.to_uppercase())),
        ] {
            let refused = service_name(&given, None).unwrap_err();
            assert!(
                refused.message.contains("burrow's own folders")
                    && refused.message.contains("the config file name"),
                "{short}: {}",
                refused.message
            );
        }
        let named =
            service_name(Path::new("/x/s.yaml"), Some(&format!("burrow-{short}"))).unwrap_err();
        assert!(named.message.contains("--name"), "{}", named.message);
    }
    assert!(service_name(Path::new("/x/services-eu.yaml"), None).is_ok());
}

#[test]
fn versions_compare_numerically() {
    let v = |s: &str| parse_version(s).unwrap();
    assert!(v("0.10.0") > v("0.9.9"));
    assert!(v("1.2.0") > v("1.2.0-rc.1"));
    assert!(v("1.2.0-rc.2") > v("1.2.0-rc.1"));
    assert!(v("1.2.0-rc.10") > v("1.2.0-rc.9"));
    assert!(v("1.2.0-rc.1") > v("1.2.0-rc"));
    assert!(v("1.2.0-beta") > v("1.2.0-alpha.9"));
    assert!(v("1.2.0-rc.1") > v("1.1.9"));
    assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
    assert_eq!(v("1.2.3 (abc)"), v("1.2.3"));
    assert_eq!(parse_version("unknown version"), None);
    assert_eq!(parse_version("1.2"), None);
    assert_eq!(parse_version("1.2.3.4"), None);
}

#[test]
fn versions_follow_semver_strictly() {
    assert_eq!(parse_version("01.2.3"), None);
    assert_eq!(parse_version("1.02.3"), None);
    assert_eq!(parse_version("1.0.0-01"), None);
    let v = |s: &str| parse_version(s).unwrap();
    assert!(v("1.0.0-1") < v("1.0.0-2"));
    assert!(v("1.0.0-1") < v("1.0.0-a"));
    assert_eq!(parse_version("1.0.0-"), None);
}

#[test]
fn the_config_exit_code_reads_the_same_on_every_backend() {
    assert!(is_config_exit("3"));
    assert!(!is_config_exit("203"));
    assert!(!is_config_exit(""));
    let detail = config_exit_detail("burrow-server");
    assert!(detail.contains("(exit 3)"), "{detail}");
    assert!(detail.contains("service start burrow-server"), "{detail}");
    assert!(logs_hint("burrow-server").contains("service logs burrow-server"));
}
