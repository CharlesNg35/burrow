#![cfg(all(test, unix))]
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;

use super::place_files;
use crate::base::failure::Failure;
use crate::install::{Manifest, Platform};
use crate::service::drop_ins::Snapshot;
use crate::service::previous::{Absent, Undo};
use crate::service::{Backend, Status};

struct FailAt {
    step: &'static str,
    failed: Cell<bool>,
    log: RefCell<Vec<String>>,
}

impl FailAt {
    fn new(step: &'static str) -> FailAt {
        FailAt {
            step,
            failed: Cell::new(false),
            log: RefCell::default(),
        }
    }

    fn run(&self, what: &str, account: &str) -> Result<(), Failure> {
        self.log.borrow_mut().push(format!("{what} {account}"));
        if what == self.step && !self.failed.replace(true) {
            return Err(Failure::new(format!("{what} failed")));
        }
        Ok(())
    }

    fn ran(&self, what: &str) -> bool {
        self.log.borrow().iter().any(|l| l == what)
    }
}

impl Backend for FailAt {
    fn kind(&self) -> &'static str {
        "fail-at"
    }
    fn render(&self, _: &Manifest) -> String {
        String::new()
    }
    fn ensure_account(&self, _: &str) -> Result<bool, Failure> {
        Ok(false)
    }
    fn remove_account(&self, account: &str) -> Result<(), Failure> {
        self.run("remove", account)
    }
    fn can_read(&self, _: &str, _: &Path) -> bool {
        true
    }
    fn secure_file(&self, _: &Path, account: &str) -> Result<(), Failure> {
        self.run("secure", account)
    }
    fn config_folder(&self, _: &Path, _: &Path, account: &str, _: bool) -> Result<(), Failure> {
        self.run("folder", account)
    }
    fn prepare_dirs(&self, _: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn write_env(&self, m: &Manifest, _: &BTreeMap<String, String>) -> Result<(), Failure> {
        self.run("env", &m.account)
    }
    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        self.run("install", &m.account)
    }
    fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
        self.run("uninstall", &m.account)
    }
    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        self.run("start", &m.account)
    }
    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        self.run("stop", &m.account)
    }
    fn reload(&self, _: &Manifest) -> Result<(), Failure> {
        Ok(())
    }
    fn restore_updater(&self, old: &Manifest, _: &Manifest) -> Result<(), Failure> {
        self.run("updater", &old.account)
    }
    fn status(&self, _: &Manifest) -> Status {
        Status {
            installed: true,
            running: false,
            pid: None,
            detail: String::new(),
        }
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::base::fs::write_private(path, text).unwrap();
}

#[test]
fn a_reinstall_that_fails_after_the_copy_puts_the_previous_files_back() {
    for step in ["secure", "folder", "env", "install", "finish", "none"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("mine.yaml");
        write(&source, "new config\n");
        write(&dir.path().join("mine.d/a.yaml"), "# the new a\n");
        let folder = dir.path().join("etc");
        let config = folder.join("server.yaml");
        write(&config, "old config\n");
        write(&folder.join("server.d/a.yaml"), "# the old a\n");
        let (binary, kept) = (dir.path().join("burrow"), dir.path().join("burrow.kept"));
        write(&binary, "new binary");
        write(&kept, "old binary");
        let old = Manifest {
            config: config.clone(),
            env_file: Some(dir.path().join("server.env")),
            account: "old".into(),
            ..crate::service::tests::record(Platform::Linux)
        };
        let new = Manifest {
            account: "new".into(),
            ..old.clone()
        };
        let undo = Undo {
            manifest: old,
            config: crate::service::system::read_plain_file(&config),
            drop_ins: Snapshot::take(&config),
            env: [("SECRET".to_string(), "old".to_string())].into(),
            binary: Some((kept.clone(), binary.clone())),
            absent: Absent::default(),
        };
        let backend = FailAt::new(step);
        let env = [("SECRET".to_string(), "new".to_string())].into();
        let placed = place_files(
            &backend,
            Some(&undo),
            (&source, &folder),
            (&new, &env, &[]),
            |_| backend.run("finish", "new"),
            &mut Vec::new(),
        );
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        if step == "none" {
            placed.unwrap();
            assert_eq!(read(&config), "new config\n");
            assert_eq!(read(&folder.join("server.d/a.yaml")), "# the new a\n");
            assert_eq!(read(&binary), "new binary");
            assert!(!backend.ran("install old"));
            assert!(!backend.ran("updater old"));
            continue;
        }
        assert!(placed.is_err(), "{step}");
        assert_eq!(read(&config), "old config\n", "{step}");
        assert_eq!(
            read(&folder.join("server.d/a.yaml")),
            "# the old a\n",
            "{step}"
        );
        assert_eq!(read(&binary), "old binary", "{step}");
        assert!(!kept.exists(), "{step}");
        assert!(backend.ran("secure old"), "{step}");
        let env_written = ["env", "install", "finish"].contains(&step);
        assert_eq!(backend.ran("env old"), env_written, "{step}");
        let unit_written = ["install", "finish"].contains(&step);
        assert_eq!(backend.ran("install old"), unit_written, "{step}");
        assert_eq!(backend.ran("updater old"), step == "finish", "{step}");
    }
}

struct NewPath {
    dir: tempfile::TempDir,
    old: Manifest,
    new: Manifest,
}

impl NewPath {
    fn new() -> NewPath {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("mine.yaml"), "new config\n");
        write(&dir.path().join("mine.d/a.yaml"), "# the new a\n");
        let folder = dir.path().join("etc");
        write(&folder.join("client.yaml"), "old config\n");
        let old = Manifest {
            config: folder.join("client.yaml"),
            env_file: None,
            account: "old".into(),
            ..crate::service::tests::record(Platform::Linux)
        };
        let new = Manifest {
            config: folder.join("server.yaml"),
            account: "new".into(),
            ..old.clone()
        };
        NewPath { dir, old, new }
    }

    fn undo(&self) -> Undo {
        Undo {
            manifest: self.old.clone(),
            config: crate::service::system::read_plain_file(&self.old.config),
            drop_ins: Snapshot::take(&self.old.config),
            env: BTreeMap::new(),
            binary: None,
            absent: Absent::before(&self.old, &self.new),
        }
    }

    fn place(&self, backend: &FailAt, undo: &Undo) -> Result<(), Failure> {
        place_files(
            backend,
            Some(undo),
            (
                &self.dir.path().join("mine.yaml"),
                &self.dir.path().join("etc"),
            ),
            (&self.new, &BTreeMap::new(), &[]),
            |_| backend.run("finish", "new"),
            &mut Vec::new(),
        )
    }
}

#[test]
fn a_failed_reinstall_removes_the_config_it_made_at_a_new_path_and_keeps_what_was_there() {
    for step in ["secure", "install", "finish"] {
        let t = NewPath::new();
        let undo = t.undo();
        let backend = FailAt::new(step);
        assert!(t.place(&backend, &undo).is_err(), "{step}");
        let etc = t.dir.path().join("etc");
        assert_eq!(
            std::fs::read_to_string(&t.old.config).unwrap(),
            "old config\n"
        );
        assert!(!t.new.config.exists(), "{step}");
        assert!(!etc.join("server.d").exists(), "{step}");
    }
    let t = NewPath::new();
    let etc = t.dir.path().join("etc");
    write(&t.new.config, "left by an earlier install\n");
    write(&etc.join("server.d/kept.yaml"), "# kept by hand\n");
    let undo = t.undo();
    assert!(t.place(&FailAt::new("install"), &undo).is_err());
    assert!(t.new.config.is_file(), "a file that was there stays");
    assert!(etc.join("server.d/kept.yaml").is_file());
    let t = NewPath::new();
    let undo = t.undo();
    t.place(&FailAt::new("none"), &undo).unwrap();
    let etc = t.dir.path().join("etc");
    assert_eq!(
        std::fs::read_to_string(&t.new.config).unwrap(),
        "new config\n"
    );
    assert_eq!(
        std::fs::read_to_string(etc.join("server.d/a.yaml")).unwrap(),
        "# the new a\n"
    );
}

#[test]
fn a_put_back_after_a_failed_start_undoes_the_updater_and_the_config_at_a_new_path() {
    let t = NewPath::new();
    let undo = t.undo();
    t.place(&FailAt::new("none"), &undo).unwrap();
    assert!(t.new.config.is_file());
    let backend = FailAt::new("none");
    let _ = undo.put_back(&backend, &t.new, "exited", |_| Ok(()), &mut Vec::new());
    assert!(backend.ran("updater old"));
    assert!(backend.ran("start old"));
    assert!(!t.new.config.exists());
    assert!(!t.dir.path().join("etc/server.d").exists());
    assert_eq!(
        std::fs::read_to_string(&t.old.config).unwrap(),
        "old config\n"
    );
}

#[test]
fn a_failed_fresh_install_gives_what_it_gave_the_new_account_to_root_before_removing_it() {
    use std::os::unix::fs::{MetadataExt, chown};
    if !rustix::process::geteuid().is_root() {
        return;
    }
    let Ok(ids) = crate::base::process::account_ids("nobody") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("etc");
    let (config, drop_ins) = (folder.join("server.yaml"), folder.join("server.d"));
    let state = dir.path().join("state");
    write(&config, "listen: 1\n");
    write(&drop_ins.join("apps.yaml"), "services: {}\n");
    write(&drop_ins.join("theirs.yaml"), "services: {}\n");
    write(&state.join("ca.key"), "key\n");
    let given = [
        config.clone(),
        drop_ins.join("apps.yaml"),
        drop_ins.clone(),
        folder.clone(),
        state.clone(),
        state.join("ca.key"),
    ];
    for p in &given {
        chown(p, Some(ids.0), Some(ids.1)).unwrap();
    }
    chown(drop_ins.join("theirs.yaml"), Some(1), Some(1)).unwrap();
    let backend = FailAt::new("none");
    drop(super::FreshAccount {
        backend: &backend,
        account: Some("nobody"),
        files: Manifest {
            config: config.clone(),
            account: "nobody".into(),
            web_state: Some(state.clone()),
            ..crate::service::tests::record(Platform::Linux)
        },
    });
    let owner = |p: &Path| {
        let meta = std::fs::symlink_metadata(p).unwrap();
        (meta.uid(), meta.gid())
    };
    for p in &given {
        assert_eq!(owner(p), (0, 0), "{}", p.display());
    }
    assert_eq!(owner(&drop_ins.join("theirs.yaml")), (1, 1));
    let note = std::fs::read_to_string(drop_ins.join(".burrow-given")).unwrap();
    assert!(
        note.contains("ug apps.yaml\n") && note.contains("ug .\n"),
        "{note}"
    );
    assert!(backend.ran("remove nobody"));
}

#[test]
fn a_failed_install_as_another_account_gives_the_previous_account_its_drop_ins_and_web_state() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
    if !rustix::process::geteuid().is_root() {
        return;
    }
    let (Ok(a), Ok(b)) = (
        crate::base::process::account_ids("nobody"),
        crate::base::process::account_ids("daemon"),
    ) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("etc/server.yaml");
    let drop_ins = dir.path().join("etc/server.d");
    let state = dir.path().join("state");
    write(&config, "listen: 1\n");
    write(&drop_ins.join("apps.yaml"), "services: {}\n");
    write(&drop_ins.join("locked.yaml"), "clients: {}\n");
    write(&state.join("ca/ca.key"), "key\n");
    let locked = drop_ins.join("locked.yaml");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o440)).unwrap();
    let give = |p: &Path, (uid, gid): (u32, u32)| chown(p, Some(uid), Some(gid)).unwrap();
    give(&drop_ins.join("apps.yaml"), a);
    give(&locked, (0, a.1));
    for p in [state.clone(), state.join("ca"), state.join("ca/ca.key")] {
        give(&p, a);
    }
    let old = Manifest {
        config: config.clone(),
        account: "nobody".into(),
        env_file: None,
        web_state: Some(state.clone()),
        ..crate::service::tests::record(Platform::Linux)
    };
    let new = Manifest {
        account: "daemon".into(),
        ..old.clone()
    };
    let undo = Undo {
        manifest: old,
        config: crate::service::system::read_plain_file(&config),
        drop_ins: Snapshot::take(&config),
        env: BTreeMap::new(),
        binary: None,
        absent: Absent::default(),
    };
    give(&drop_ins.join("apps.yaml"), b);
    give(&locked, (0, b.1));
    for p in [state.clone(), state.join("ca"), state.join("ca/ca.key")] {
        give(&p, b);
    }
    let steps = undo.restore_setup(&FailAt::new("none"), &new, super::Reached::Files);
    assert!(steps.iter().all(Result::is_ok), "{steps:?}");
    let owner = |p: &Path| {
        let meta = std::fs::symlink_metadata(p).unwrap();
        (meta.uid(), meta.gid())
    };
    assert_eq!(owner(&drop_ins.join("apps.yaml")), a);
    assert_eq!(owner(&locked), (0, a.1));
    for p in [state.clone(), state.join("ca"), state.join("ca/ca.key")] {
        assert_eq!(owner(&p), a, "{}", p.display());
    }
}
