use super::lock::held_elsewhere;
#[cfg(unix)]
use super::lock::planted;
#[cfg(unix)]
use super::note::{open_shared, trusted_note, write_note};
use super::*;

#[test]
fn files_of_a_running_instance_stay_fresh_for_tmp_cleaners() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.status");
    std::fs::write(&file, "x").unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    let age = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
    let set_old = || {
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(old)
                    .set_modified(old),
            )
            .unwrap()
    };
    set_old();
    let fresh = keep_fresh_every(vec![file.clone()], std::time::Duration::from_millis(20));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while age(&file) == old && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_ne!(age(&file), old);
    drop(fresh);
    std::thread::sleep(std::time::Duration::from_millis(50));
    set_old();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(age(&file), old, "a dropped guard kept touching");
    std::fs::remove_file(&file).unwrap();
    let fresh = keep_fresh_every(vec![file.clone()], std::time::Duration::from_millis(20));
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(fresh);
    assert!(!file.exists(), "touching must not create a removed file");
}

#[cfg(unix)]
fn soon(mut done: impl FnMut() -> bool) -> bool {
    (0..100).any(|_| {
        done() || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            false
        }
    })
}

#[cfg(unix)]
#[test]
fn the_install_check_only_reads_the_lock_folders() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    let id = Identity::server(&cfg);
    let missing = dir.path().join("missing");
    let used = dir.path().join("used");
    std::fs::create_dir(&used).unwrap();
    let dirs = [missing.clone(), used.clone()];
    assert!(held_in(&dirs, &id).is_none());
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(&used).unwrap().count(), 0);
    let lock = acquire_in(&used, &id, &cfg).unwrap();
    let f = held_in(&dirs, &id).expect("a held lock is found");
    assert!(f.message.contains("is already running"), "{}", f.message);
    drop(lock);
    assert!(held_in(&dirs, &id).is_none());
}

#[cfg(unix)]
#[test]
fn only_locks_and_status_files_nobody_uses_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    let held = acquire_in(dir.path(), &Identity::server(&cfg), &cfg).unwrap();
    let unused = acquire_in(dir.path(), &Identity::client("example.com", "home"), &cfg).unwrap();
    let unused_lock = unused.path().to_path_buf();
    drop(unused);
    let stale = dir.path().join("999999999.status");
    let live = dir.path().join(format!("{}.status", std::process::id()));
    std::fs::write(&stale, "{}").unwrap();
    std::fs::write(&live, "{}").unwrap();
    assert!(soon(|| {
        remove_unused(dir.path());
        !unused_lock.exists() && !unused_lock.with_extension("info").exists()
    }));
    assert!(held.path().exists() && held.path().with_extension("info").exists());
    assert!(!stale.exists() && live.exists());
    std::fs::remove_file(&live).unwrap();
    let held_path = held.path().to_path_buf();
    drop(held);
    assert!(soon(|| {
        remove_unused(dir.path());
        !held_path.exists()
    }));
    assert!(!dir.path().exists(), "an empty folder is removed");
}

#[cfg(unix)]
#[test]
fn a_second_start_waits_for_the_note_of_one_that_just_took_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("client.yaml");
    let id = Identity::client("example.com", "home");
    let lock = dir.path().join(format!("{}.lock", id.key()));
    let (held, _) = open_shared(&lock).unwrap();
    held.try_lock().unwrap();
    let info = lock.with_extension("info");
    let note = format!(
        "pid {}, config {}",
        std::process::id(),
        crate::base::fs::canonical_or_same(&cfg).display()
    );
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(write_note(&info, &note));
    });
    let second = acquire_in(dir.path(), &id, &cfg).err().unwrap();
    writer.join().unwrap();
    assert!(
        second.message.contains("already running"),
        "{}",
        second.message
    );
}

#[cfg(unix)]
#[test]
fn instances_racing_over_a_linked_lock_still_start_only_once() {
    for _ in 0..40 {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("client.yaml");
        let id = Identity::client("example.com", "home");
        drop(acquire_in(dir.path(), &id, &cfg).unwrap());
        let lock = dir.path().join(format!("{}.lock", id.key()));
        std::fs::hard_link(&lock, dir.path().join("planted")).unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(4));
        let racers: Vec<_> = (0..4)
            .map(|_| {
                let (dir, cfg, id, start) = (
                    dir.path().to_path_buf(),
                    cfg.clone(),
                    id.clone(),
                    start.clone(),
                );
                std::thread::spawn(move || {
                    start.wait();
                    acquire_in(&dir, &id, &cfg).ok()
                })
            })
            .collect();
        let held: Vec<_> = racers
            .into_iter()
            .filter_map(|r| r.join().unwrap())
            .collect();
        assert_eq!(held.len(), 1, "{} instances started", held.len());
    }
}

#[cfg(unix)]
#[test]
fn a_hard_link_to_the_lock_cannot_keep_an_instance_from_starting() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    let id = Identity::server(&cfg);
    let held = acquire_in(dir.path(), &id, &cfg).unwrap();
    let lock = held.path().to_path_buf();
    std::fs::hard_link(&lock, dir.path().join("planted")).unwrap();
    let again = acquire_in(dir.path(), &id, &cfg).err().unwrap();
    assert!(
        again.message.contains("already running"),
        "{}",
        again.message
    );
    drop(held);
    let restarted = acquire_in(dir.path(), &id, &cfg).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::fs::metadata(restarted.path()).unwrap().nlink(), 1);
    let planted = std::fs::File::open(dir.path().join("planted")).unwrap();
    assert!(soon(|| planted.try_lock().is_ok()));
    drop(restarted);
    let ignored = acquire_in(dir.path(), &id, &cfg);
    assert!(
        ignored.is_ok(),
        "an outsider holding the old file must not lock it out"
    );
}

#[cfg(unix)]
#[test]
fn running_instances_are_read_from_their_notes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("client.yaml");
    let lock = acquire_in(dir.path(), &Identity::client("example.com", "home"), &cfg).unwrap();
    std::fs::write(
        dir.path().join("dead.info"),
        "pid 999999999, config /x/server.yaml",
    )
    .unwrap();
    std::fs::write(dir.path().join("odd.info"), "something else").unwrap();
    let found = running_in(dir.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, std::process::id());
    assert!(found[0].config.ends_with("client.yaml"));
    drop(lock);
}

#[cfg(unix)]
#[test]
fn only_entries_of_another_user_count_as_planted() {
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("x.lock");
    std::fs::write(&mine, "").unwrap();
    assert!(!planted(&mine));
    assert!(!planted(&dir.path().join("missing.lock")));
    assert!(!planted(Path::new("/")));
}

#[test]
fn only_old_drafts_are_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir
        .path()
        .join(format!(".1.status.1-0.{}", crate::base::fs::TEMP_EXTENSION));
    std::fs::write(&fresh, "x").unwrap();
    let old = dir
        .path()
        .join(format!(".2.status.1-1.{}", crate::base::fs::TEMP_EXTENSION));
    std::fs::write(&old, "x").unwrap();
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    remove_unused(dir.path());
    assert!(fresh.exists());
    assert!(!old.exists());
}

#[test]
fn a_tunnel_locked_in_the_other_instance_folder_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let id = Identity::server(&cfg);
    assert!(held_elsewhere(dir.path(), &id).is_none());
    let lock = acquire_in(dir.path(), &id, &cfg).unwrap();
    let found = held_elsewhere(dir.path(), &id).expect("the held lock is found");
    assert!(
        found.message.contains("is already running"),
        "{}",
        found.message
    );
    drop(lock);
    assert!(held_elsewhere(dir.path(), &id).is_none());
}

#[cfg(unix)]
#[test]
fn a_pipe_in_place_of_a_note_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("x.info");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&note)
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(trusted_note(&note).is_none()).unwrap());
    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
}

#[test]
fn a_released_lock_takes_its_note_away() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let id = Identity::server(&cfg);
    let lock = acquire_in(dir.path(), &id, &cfg).unwrap();
    let note = lock.path().with_extension("info");
    assert!(note.exists());
    drop(lock);
    assert!(!note.exists());
}

#[test]
fn a_lock_held_without_a_trusted_note_does_not_stop_a_start() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let id = Identity::server(&cfg);
    let first = acquire_in(dir.path(), &id, &cfg).unwrap();
    std::fs::remove_file(first.path().with_extension("info")).unwrap();
    assert!(acquire_in(dir.path(), &id, &cfg).is_ok());
    drop(first);
}

#[cfg(unix)]
#[test]
fn second_instance_of_the_same_identity_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let id = Identity::server(&cfg);
    let first = acquire_in(dir.path(), &id, &cfg).unwrap();
    let err = acquire_in(dir.path(), &id, &cfg).err().unwrap();
    assert!(
        err.message.contains("is already running"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains(&format!("pid {}", std::process::id())),
        "{}",
        err.message
    );
    drop(first);
    assert!(
        soon(|| acquire_in(dir.path(), &id, &cfg).is_ok()),
        "lock is released when the holder ends"
    );
}

#[test]
fn different_identities_run_side_by_side() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("c.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let a = acquire_in(
        dir.path(),
        &Identity::client("tunnel.example.com:7835", "home"),
        &cfg,
    )
    .unwrap();
    let b = acquire_in(
        dir.path(),
        &Identity::client("tunnel.example.com:7835", "office"),
        &cfg,
    )
    .unwrap();
    let c = acquire_in(
        dir.path(),
        &Identity::client("other.example.com:7835", "home"),
        &cfg,
    )
    .unwrap();
    let same = acquire_in(
        dir.path(),
        &Identity::client("TUNNEL.example.com:7835", "home"),
        &cfg,
    );
    assert!(same.is_err(), "host names compare case-insensitively");
    drop((a, b, c));
}

#[cfg(unix)]
#[test]
fn a_folder_everyone_can_write_without_the_sticky_bit_gets_no_lock() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("instances");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let config = base.path().join("server.yaml");
    std::fs::write(&config, "").unwrap();
    let lock = acquire_in(&dir, &Identity::server(&config), &config).unwrap();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    drop(lock);
    assert!(!safe_dir(&dir));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777)).unwrap();
    assert!(safe_dir(&dir));
}

#[cfg(unix)]
#[test]
fn planted_links_are_not_followed_or_changed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let id = Identity::server(&cfg);
    let victim = dir.path().join("victim");
    std::fs::write(&victim, "keep").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();
    let lock = dir.path().join(format!("{}.lock", id.key()));

    std::os::unix::fs::symlink(&victim, &lock).unwrap();
    assert!(acquire_in(dir.path(), &id, &cfg).is_err());
    std::fs::remove_file(&lock).unwrap();

    std::fs::hard_link(&victim, &lock).unwrap();
    let replaced = acquire_in(dir.path(), &id, &cfg).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), "");
    drop(replaced);
    std::fs::remove_file(&lock).unwrap();
    let _ = std::fs::remove_file(lock.with_extension("info"));

    std::os::unix::fs::symlink(&victim, lock.with_extension("info")).unwrap();
    let held = acquire_in(dir.path(), &id, &cfg).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(
        std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(held);
}

#[test]
fn server_identity_follows_the_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.yaml");
    std::fs::write(&cfg, "x").unwrap();
    let direct = Identity::server(&cfg);
    let via_dot = Identity::server(&dir.path().join(".").join("server.yaml"));
    assert_eq!(direct, via_dot);
}

#[test]
fn a_lock_whose_file_keeps_being_replaced_is_given_up_not_kept() {
    use super::lock::{Locked, after_lock};
    assert_eq!(after_lock(1, true), Locked::Keep);
    assert_eq!(after_lock(1, false), Locked::Again);
    assert_eq!(after_lock(4, false), Locked::Again);
    assert_eq!(after_lock(5, false), Locked::Moved);
    assert_eq!(after_lock(5, true), Locked::Keep);
}
