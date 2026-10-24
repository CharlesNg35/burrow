
use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn only_a_burrow_binary_running_a_role_is_a_burrow_command() {
    let config = Some(PathBuf::from("/etc/burrow/client.yaml"));
    for line in [
        &[
            "/usr/local/bin/burrow",
            "client",
            "-c",
            "/etc/burrow/client.yaml",
        ][..],
        &[
            "/usr/local/bin/burrow",
            "client",
            "--config",
            "/etc/burrow/client.yaml",
        ],
        &[
            "/usr/local/bin/burrow",
            "client",
            "--config=/etc/burrow/client.yaml",
        ],
        &[
            r"C:\Program Files\burrow\burrow.exe",
            "client",
            "-c",
            "/etc/burrow/client.yaml",
        ],
    ] {
        assert_eq!(
            burrow_command(&args(line)),
            Some(config.clone()),
            "{line:?}"
        );
    }
    assert_eq!(
        burrow_command(&args(&["/usr/local/bin/burrow", "visit"])),
        Some(None)
    );
    for line in [
        &["/usr/local/bin/burrow", "admin"][..],
        &["/usr/local/bin/burrowd", "client", "-c", "x.yaml"],
        &["/usr/bin/python3", "/opt/burrow", "client"],
        &["/usr/local/bin/burrow"],
        &[],
    ] {
        assert_eq!(burrow_command(&args(line)), None, "{line:?}");
    }
}

const OLD_UNIT: &str = "[Unit]\nDescription=burrow client \"client\"\n[Service]\nUser=burrow\nExecStart=/usr/local/bin/burrow client -c /etc/burrow/client.yaml --log-file /var/log/burrow/client/client.log\nExecReload=/bin/kill -HUP $MAINPID\n";

#[test]
fn a_systemd_unit_counts_only_when_it_runs_burrow() {
    assert_eq!(
        systemd_unit(OLD_UNIT),
        Some(Some(PathBuf::from("/etc/burrow/client.yaml")))
    );
    assert_eq!(
        systemd_unit(
            "[Service]\nExecStart=-\"/opt/my tools/burrow\" server -c \"/etc/my burrow/s.yaml\"\n"
        ),
        Some(Some(PathBuf::from("/etc/my burrow/s.yaml")))
    );
    assert_eq!(
        systemd_unit("[Service]\nExecStart=/usr/bin/burrow-exporter --port 9100\n"),
        None
    );
    assert_eq!(
        systemd_unit("[Service]\nExecStart=/usr/bin/burrow status\n"),
        None
    );
    assert_eq!(systemd_unit("[Unit]\nDescription=burrow\n"), None);
}

#[test]
fn a_launchd_job_counts_only_when_it_runs_burrow() {
    let job = |program: &str| {
        format!(
            "<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>com.github.charlesng35.burrow-home</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{program}</string>\n    <string>client</string>\n    <string>-c</string>\n    <string>/etc/burrow/home &amp; more.yaml</string>\n  </array>\n</dict>\n</plist>\n"
        )
    };
    assert_eq!(
        launchd_job(&job("/usr/local/bin/burrow")),
        Some(Some(PathBuf::from("/etc/burrow/home & more.yaml")))
    );
    assert_eq!(launchd_job(&job("/usr/local/bin/other")), None);
    assert_eq!(launchd_job("<plist></plist>"), None);
}

#[test]
fn units_are_read_from_their_folder_and_only_burrow_s_own_count() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    write("burrow-client.service", OLD_UNIT);
    write("burrow.service", OLD_UNIT);
    write(
        "burrow-exporter.service",
        "[Service]\nExecStart=/usr/bin/exporter\n",
    );
    write("nginx.service", OLD_UNIT);
    write("burrow-client.service.bak", OLD_UNIT);
    std::fs::create_dir(dir.path().join("burrow-x.service")).unwrap();
    let mut found = read_units(dir.path(), "", ".service", systemd_unit);
    found.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<(&str, &str)> = found
        .iter()
        .map(|u| (u.name.as_str(), u.label.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("burrow", "burrow.service"),
            ("burrow-client", "burrow-client.service")
        ]
    );
    assert_eq!(
        found[1].config.as_deref(),
        Some(Path::new("/etc/burrow/client.yaml"))
    );
    let jobs = read_units(dir.path(), "com.github.charlesng35.", ".plist", launchd_job);
    assert!(jobs.is_empty());
    assert!(read_units(&dir.path().join("missing"), "", ".service", systemd_unit).is_empty());
}

fn unit(name: &str, config: Option<&str>) -> Unit {
    Unit {
        name: name.into(),
        label: format!("{name}.service"),
        config: config.map(PathBuf::from),
    }
}

#[test]
fn strays_are_the_units_and_records_burrow_cannot_use() {
    let known = [bare("burrow-server")];
    let records = vec![
        (
            "burrow-client".to_string(),
            Some(PathBuf::from("/old/client.yaml")),
        ),
        ("burrow-gone".to_string(), None),
        ("not-burrow".to_string(), None),
    ];
    let units = vec![
        unit("burrow-server", Some("/etc/burrow/server/server.yaml")),
        unit("burrow-client", Some("/etc/burrow/client.yaml")),
        unit("burrow-hand", None),
    ];
    let found = merge(&known, records, units);
    let names: Vec<(&str, &str, Option<&Path>)> = found
        .iter()
        .map(|u| (u.name.as_str(), u.label.as_str(), u.config.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            (
                "burrow-client",
                "burrow-client.service",
                Some(Path::new("/etc/burrow/client.yaml"))
            ),
            ("burrow-gone", "burrow-gone", None),
            ("burrow-hand", "burrow-hand.service", None),
        ]
    );
}

fn stray(name: &str, running: bool, config: Option<&str>) -> Stray {
    Stray {
        name: name.into(),
        label: format!("{name}.service"),
        config: config.map(PathBuf::from),
        running,
        pid: running.then_some(42),
        stop: format!("sudo systemctl stop {name}"),
    }
}

#[test]
fn a_stray_names_the_exact_commands_that_take_it_over() {
    let f = stray("burrow-client", true, Some("/etc/burrow/client.yaml")).failure();
    assert_eq!(
        f.message,
        "burrow-client.service is running, but burrow has no usable record of it (installed by an older burrow or by hand)"
    );
    let admin = crate::base::failure::admin;
    assert_eq!(
        f.hint.unwrap(),
        format!(
            "take it over: sudo systemctl stop burrow-client, then {}",
            admin("service install -c /etc/burrow/client.yaml")
        )
    );
    let f = stray("burrow-home", false, Some("/etc/burrow/home/client.yaml")).failure();
    assert!(
        f.message.starts_with("burrow-home.service is stopped, but"),
        "{}",
        f.message
    );
    let hint = f.hint.unwrap();
    assert!(!hint.contains("systemctl"), "{hint}");
    assert!(
        hint.ends_with(&admin(
            "service install -c /etc/burrow/home/client.yaml --name home"
        )),
        "{hint}"
    );
    let hint = stray("burrow-hand", false, None).failure().hint.unwrap();
    assert!(
        hint.ends_with(&admin("service install -c CONFIG --name hand")),
        "{hint}"
    );
}

fn manifest(name: &str) -> Manifest {
    bare(name)
}

#[test]
fn a_stray_is_counted_when_a_service_is_picked_without_a_name() {
    let one = manifest("burrow-server");
    assert_eq!(pick(vec![one.clone()], &[]).unwrap().name, "burrow-server");
    let f = pick(vec![], &[]).unwrap_err();
    assert_eq!(f.message, "no burrow service is installed");
    let s = stray("burrow-client", true, Some("/etc/burrow/client.yaml"));
    let f = pick(vec![], std::slice::from_ref(&s)).unwrap_err();
    assert_eq!(f, s.failure());
    let other = stray("burrow-home", false, None);
    let f = pick(vec![], &[s.clone(), other.clone()]).unwrap_err();
    assert_eq!(
        f.message,
        "burrow-client.service, burrow-home.service are installed, but burrow has no usable record of them (installed by an older burrow or by hand)"
    );
    assert_eq!(none_usable(&[s.clone(), other]), f);
    let f = pick(vec![one], std::slice::from_ref(&s)).unwrap_err();
    assert!(f.message.contains("more than one"), "{}", f.message);
    assert_eq!(
        f.hint.unwrap(),
        "installed: burrow-server, burrow-client (not managed by this burrow)"
    );
}

#[test]
fn an_update_notes_a_stray_it_left_alone_once() {
    let mut out = Vec::new();
    stray("burrow-client", true, Some("/etc/burrow/client.yaml"))
        .note(&mut out)
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
            out.starts_with(&format!(
                "note: burrow-client.service (running) was not restarted, because burrow has no usable record of it; take it over: sudo systemctl stop burrow-client, then {}",
                crate::base::failure::admin("service install -c /etc/burrow/client.yaml")
            )),
            "{out}"
        );
    assert_eq!(out.matches("burrow-client.service").count(), 1, "{out}");
}

#[test]
fn what_a_stray_may_still_use_is_kept_and_named() {
    let bin = Path::new("/usr/local/bin/burrow");
    assert_eq!(keeps(&[], bin.display()), None);
    assert_eq!(
        keeps(&[stray("burrow-stray", true, None)], bin.display()).as_deref(),
        Some("kept /usr/local/bin/burrow: burrow-stray.service still uses it")
    );
    assert_eq!(
        keeps(
            &[
                stray("burrow-a", false, None),
                stray("burrow-b", true, None)
            ],
            "service account burrow"
        )
        .as_deref(),
        Some("kept service account burrow: burrow-a.service, burrow-b.service still use it")
    );
}

#[test]
fn the_files_an_unusable_record_names_are_its_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("burrow-old.json"),
            r#"{"name":"burrow-old","config":"/etc/burrow/old.yaml","env_file":"/etc/burrow/old.env","log_file":"/var/log/burrow/old/old.log"}"#,
        )
        .unwrap();
    std::fs::write(
            dir.path().join("burrow-other.json"),
            r#"{"name":"burrow-other","config":"/etc/burrow/other.yaml","env_file":null,"log_file":""}"#,
        )
        .unwrap();
    std::fs::write(dir.path().join("burrow-junk.json"), "not json").unwrap();
    let mut old: Vec<PathBuf> = ["/etc/burrow/old.yaml", "/etc/burrow/old.env"]
        .into_iter()
        .map(PathBuf::from)
        .chain(
            std::iter::once("/var/log/burrow/old/old.log".to_string())
                .chain(
                    (1..=crate::base::logfile::KEEP)
                        .map(|n| format!("/var/log/burrow/old/old.log.{n}")),
                )
                .map(PathBuf::from),
        )
        .collect();
    old.sort();
    let mut found = leftovers_in(dir.path(), Some("burrow-old"));
    found.sort();
    assert_eq!(found, old);
    let mut all = leftovers_in(dir.path(), None);
    all.sort();
    old.push(PathBuf::from("/etc/burrow/other.yaml"));
    old.sort();
    assert_eq!(all, old);
    assert!(leftovers_in(&dir.path().join("missing"), None).is_empty());
}

#[test]
fn a_stray_in_the_list_says_how_to_learn_more() {
    let line = stray("burrow-client", true, None).line();
    assert!(line.starts_with("burrow-client"), "{line}");
    assert!(
        line.contains("running  not managed by this burrow (older layout or hand-made); see: "),
        "{line}"
    );
    assert!(line.ends_with("service status burrow-client"), "{line}");
}
