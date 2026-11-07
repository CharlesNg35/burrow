use std::collections::BTreeMap;

use super::{Absent, Undo};
use crate::install::{Manifest, Platform};
use crate::service::tests::{Recorder, record};

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
    let new = Manifest {
        config: config.clone(),
        env_file: Some(new_env.clone()),
        account: "new".into(),
        created_account: true,
        admin_writable: true,
        ..record(Platform::Linux)
    };
    let absent = Absent::before(&old, &new);
    std::fs::write(&new_env, "SECRET=y").unwrap();
    let binary = dir.path().join("burrow");
    let kept = dir.path().join("burrow.previous");
    std::fs::write(&binary, "new binary").unwrap();
    std::fs::write(&kept, "old binary").unwrap();
    let undo = Undo {
        manifest: old.clone(),
        config: Some(b"old config".to_vec()),
        drop_ins: Default::default(),
        env: [("SECRET".to_string(), "x".to_string())].into(),
        binary: Some((kept.clone(), binary.clone())),
        absent,
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
        drop_ins: Default::default(),
        env: BTreeMap::new(),
        binary: None,
        absent: Absent::default(),
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

#[test]
fn a_put_back_removes_only_the_paths_the_reinstall_made() {
    let dir = tempfile::tempdir().unwrap();
    let old = Manifest {
        config: dir.path().join("client.yaml"),
        env_file: None,
        web_state: None,
        ..record(Platform::Linux)
    };
    std::fs::write(&old.config, "old config").unwrap();
    let new = Manifest {
        config: dir.path().join("server.yaml"),
        env_file: Some(dir.path().join("server.env")),
        web_state: Some(dir.path().join("state")),
        ..old.clone()
    };
    let there = dir.path().join("server.d");
    let state = dir.path().join("state");
    let made = |m: &Manifest| {
        std::fs::write(&m.config, "new config").unwrap();
        std::fs::write(m.env_file.as_ref().unwrap(), "SECRET=y").unwrap();
        for d in [&there, &state] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("a.yaml"), "services: {}").unwrap();
        }
    };
    for made_before in [false, true] {
        if made_before {
            made(&new);
        }
        let undo = Undo {
            manifest: old.clone(),
            config: Some(b"old config".to_vec()),
            drop_ins: Default::default(),
            env: BTreeMap::new(),
            binary: None,
            absent: Absent::before(&old, &new),
        };
        made(&new);
        let rec = Recorder(Default::default(), true);
        let _ = undo.put_back(&rec, &new, "exited", |_| Ok(()), &mut Vec::new());
        for p in [&new.config, new.env_file.as_ref().unwrap(), &there, &state] {
            assert_eq!(p.exists(), made_before, "{}", p.display());
        }
        assert_eq!(std::fs::read_to_string(&old.config).unwrap(), "old config");
    }
}
