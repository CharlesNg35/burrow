#![cfg(test)]

use super::*;

#[test]
fn a_new_drop_in_name_says_what_is_wrong_with_it() {
    let have = ["server.yaml", "server.d/apps.yaml"];
    assert_eq!(
        new_name_problem(&have, "server.d", "server.d/web.yaml"),
        None
    );
    assert!(
        new_name_problem(&have, "server.d", "server.d/apps.yaml")
            .is_some_and(|m| m.contains("already exists"))
    );
    assert!(
        new_name_problem(&have, "server.d", "server.d/Apps.yaml")
            .is_some_and(|m| m.contains("differ only in upper and lower case"))
    );
    assert!(
        new_name_problem(&have, "server.d", "server.d/my apps.yaml")
            .is_some_and(|m| m.contains("use only letters, digits, dots, dashes and underscores"))
    );
}

#[test]
fn a_drop_in_name_is_one_plain_file_in_the_folder() {
    assert_eq!(folder_name("server.yaml"), "server.d");
    assert_eq!(folder_name("home.json"), "home.d");
    assert_eq!(
        file_in("server.d", "server.d/my apps.yaml"),
        Some("my apps.yaml")
    );
    for bad in [
        "server.d/../x.yaml",
        "server.d/a/b.yaml",
        "server.d/a\\b.yaml",
        "/etc/cron.d/x.yaml",
        "server.d/..",
        "server.d/.hidden.yaml",
        "server.d/x.yml",
        "server.d/c:x.yaml",
        "other.d/x.yaml",
        "server.d",
        "server.d/a\nb.yaml",
        "server.d/a\tb.yaml",
        "server.d/a\u{7f}b.yaml",
    ] {
        assert_eq!(file_in("server.d", bad), None, "{bad}");
    }
    assert!(new_name_ok("server.d", "server.d/blog.yaml"));
    assert!(!new_name_ok("server.d", "server.d/my apps.yaml"));
    assert!(!new_name_ok("server.d", "server.d/com1.yaml"));
}

use NameProblem::*;

const ROWS: &[(&str, Option<NameProblem>, &str)] = &[
    ("server.d/blog.yaml", None, "plain"),
    (
        "server.d/team-a_1.v2.yaml",
        None,
        "dots, dashes, underscores inside",
    ),
    (
        "server.d/Blog.yaml",
        None,
        "upper case is fine; clashes are checked on disk",
    ),
    (
        "server.d/con.yaml",
        Some(Reserved),
        "Windows: CON is a device in any case",
    ),
    (
        "server.d/CON.backup.yaml",
        Some(Reserved),
        "Windows: a device name with any extension",
    ),
    ("server.d/Com1.yaml", Some(Reserved), "Windows: COM1-9"),
    (
        "server.d/com0.yaml",
        Some(Reserved),
        "Windows: COM0 is a device link (Global??)",
    ),
    (
        "server.d/LPT0.yaml",
        Some(Reserved),
        "Windows: LPT0 refused too, as COM0",
    ),
    ("server.d/lpt9.x.yaml", Some(Reserved), "Windows: LPT1-9"),
    ("server.d/prn.yaml", Some(Reserved), "Windows: PRN"),
    ("server.d/aux.yaml", Some(Reserved), "Windows: AUX"),
    (
        "server.d/nul.tar.yaml",
        Some(Reserved),
        "Windows: NUL with extensions",
    ),
    (
        "server.d/com10.yaml",
        None,
        "Windows: COM10 is not reserved",
    ),
    (
        "server.d/console.yaml",
        None,
        "Windows: only the whole first part counts",
    ),
    (
        "server.d/CONIN$.yaml",
        Some(Characters),
        "Windows: CONIN$ is refused by its $",
    ),
    (
        "server.d/CONOUT$.yaml",
        Some(Characters),
        "Windows: CONOUT$ is refused by its $",
    ),
    (
        "server.d/COM\u{b9}.yaml",
        Some(Characters),
        "Windows: COM with a superscript digit",
    ),
    (
        "server.d/x..yaml",
        Some(Characters),
        "Windows: a trailing dot before .yaml",
    ),
    (
        "server.d/x .yaml",
        Some(Characters),
        "Windows: a trailing space",
    ),
    (
        "server.d/a b.yaml",
        Some(Characters),
        "spaces: hard to quote in shells",
    ),
    (
        "server.d/a:b.yaml",
        Some(Characters),
        "Windows: : names an alternate data stream",
    ),
    ("server.d/a<b.yaml", Some(Characters), "Windows reserves <"),
    ("server.d/a>b.yaml", Some(Characters), "Windows reserves >"),
    (
        "server.d/a\"b.yaml",
        Some(Characters),
        "Windows reserves \"",
    ),
    (
        "server.d/a\\b.yaml",
        Some(Characters),
        "Windows: backslash separates paths",
    ),
    ("server.d/a|b.yaml", Some(Characters), "Windows reserves |"),
    ("server.d/a?b.yaml", Some(Characters), "Windows reserves ?"),
    ("server.d/a*b.yaml", Some(Characters), "Windows reserves *"),
    (
        "server.d/a\u{1}b.yaml",
        Some(Characters),
        "Windows: control characters 1-31",
    ),
    ("server.d/a\u{7f}b.yaml", Some(Characters), "DEL"),
    (
        "server.d/a\0b.yaml",
        Some(Characters),
        "Linux, BSD, macOS, Windows: NUL ends a name",
    ),
    (
        "server.d/a/b.yaml",
        Some(Characters),
        "Linux, BSD, macOS: / separates paths",
    ),
    ("server.d/...yaml", Some(Characters), "only dots"),
    ("server.d/..yaml", Some(Characters), "a dot alone"),
    ("server.d/.yaml", Some(Characters), "nothing before .yaml"),
    (
        "server.d/.hidden.yaml",
        Some(Characters),
        "a hidden file: the reader skips it",
    ),
    (
        "server.d/-x.yaml",
        Some(Characters),
        "a leading dash reads as an option in shells",
    ),
    (
        "server.d/caf\u{e9}.yaml",
        Some(Characters),
        "non-ASCII: macOS normalizes it (NFD)",
    ),
    (
        "server.d/\u{441}\u{435}rver.yaml",
        Some(Characters),
        "Cyrillic look-alikes",
    ),
    (
        "server.d/\u{ff42}log.yaml",
        Some(Characters),
        "a full-width letter",
    ),
    (
        "server.d/blo\u{200b}g.yaml",
        Some(Characters),
        "a zero-width space",
    ),
    (
        "server.d/x.yml",
        Some(NotInFolder),
        "the reader reads .yaml only",
    ),
    (
        "server.d/x.YAML",
        Some(NotInFolder),
        "the reader reads lower-case .yaml only",
    ),
    ("server.d/x", Some(NotInFolder), "no .yaml"),
    ("other.d/x.yaml", Some(NotInFolder), "another folder"),
    ("server.d/../x.yaml", Some(Characters), "out of the folder"),
    ("/etc/x.yaml", Some(NotInFolder), "an absolute path"),
    ("server.yaml", Some(NotInFolder), "the main file"),
];

#[test]
fn a_new_name_is_safe_on_every_system_and_says_why_not() {
    use NameProblem::*;
    let long = format!("server.d/{}.yaml", "a".repeat(MAX_STEM));
    let too_long = format!("server.d/{}.yaml", "a".repeat(MAX_STEM + 1));
    let mut rows: Vec<(String, Option<NameProblem>, &str)> = ROWS
        .iter()
        .map(|(n, p, w)| (n.to_string(), *p, *w))
        .collect();
    rows.push((
        long,
        None,
        "100 characters: 105 bytes, under NAME_MAX 255 on Linux, BSD and macOS",
    ));
    rows.push((too_long, Some(TooLong), "over burrow's 100-character limit"));
    for (name, want, why) in &rows {
        let name = name.as_str();
        assert_eq!(name_problem("server.d", name), *want, "{name}: {why}");
        if want.is_none() {
            assert!(
                file_in("server.d", name).is_some(),
                "{name} is made but not read"
            );
        } else {
            let message = want.unwrap().message("server.d", name);
            assert!(message.starts_with('"') && !message.is_empty(), "{message}");
        }
    }
    assert_eq!(
        Reserved.message("server.d", "server.d/Com1.yaml"),
        "\"server.d/Com1.yaml\": COM1 is a device name on Windows and cannot be a file name there; pick another name"
    );
}

#[test]
fn every_name_burrow_may_make_is_one_it_reads() {
    let alphabet = ['a', 'Z', '0', '.', '-', '_', ' ', '$', '\u{e9}'];
    let mut made = 0;
    for n in 0..alphabet.len().pow(4) {
        let mut stem = String::new();
        let mut k = n;
        for _ in 0..4 {
            stem.push(alphabet[k % alphabet.len()]);
            k /= alphabet.len();
        }
        let name = format!("server.d/{stem}.yaml");
        if new_name_ok("server.d", &name) {
            made += 1;
            assert!(file_in("server.d", &name).is_some(), "{name}");
        }
    }
    assert!(made > 500, "{made}");
}
