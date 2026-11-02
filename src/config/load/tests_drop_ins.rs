use std::path::{Path, PathBuf};

use burrow_admin_proto::codes;

use super::*;
use crate::base::style::Style;

const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";
const OTHER: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";

fn main_text() -> String {
    format!(
        "listen: 7835\nclients:\n  home: {{ secret: \"{SECRET}\" }}\nservices:\n  web: {{ client: home, target: 80, bind: 7900 }}\n"
    )
}

struct Dir {
    _tmp: tempfile::TempDir,
    main: PathBuf,
}

impl Dir {
    fn new() -> Dir {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("server.yaml");
        crate::base::fs::write_private(&main, &main_text()).unwrap();
        Dir { _tmp: tmp, main }
    }

    fn drop_in(&self, name: &str, text: &str) -> PathBuf {
        let folder = self.main.parent().unwrap().join("server.d");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        crate::base::fs::write_private(&path, text).unwrap();
        path
    }

    fn load(&self) -> Outcome {
        let nothing = |_: &str| None;
        load_file(&self.main, Some(Role::Server), &nothing).unwrap()
    }
}

fn shown(o: &Outcome) -> String {
    o.report.render_text(Style { color: false })
}

fn codes_of(o: &Outcome) -> Vec<&'static str> {
    o.report.diagnostics.iter().map(|d| d.code).collect()
}

fn services(o: &Outcome) -> Vec<String> {
    match &o.loaded {
        Some(Loaded::Server(s, _)) => s.services.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

#[test]
fn without_the_folder_a_config_loads_exactly_as_one_text() {
    let d = Dir::new();
    std::fs::create_dir(d.main.parent().unwrap().join("other.d")).unwrap();
    let nothing = |_: &str| None;
    let from_file = d.load();
    let mut from_text = load_str_at(
        (
            d.main.parent().unwrap(),
            Some(&crate::install::state_dir_for(&d.main)),
        ),
        &d.main.display().to_string(),
        &main_text(),
        Format::Yaml,
        Some(Role::Server),
        &nothing,
    );
    if let Some(Loaded::Server(s, _)) = from_text.loaded.as_mut()
        && let Some(w) = s.web.as_mut()
    {
        w.history = Some(crate::config::save::history_dir_of(&d.main));
    }
    assert_eq!(shown(&from_file), shown(&from_text));
    assert_eq!(
        from_file.report.render_json(),
        from_text.report.render_json()
    );
    assert_eq!(services(&from_file), ["web"]);
    assert_eq!(from_file.report.sources.len(), 1);
}

#[test]
fn services_clients_and_visitors_of_drop_ins_join_the_config_in_name_order() {
    let d = Dir::new();
    d.drop_in(
        "b.yaml",
        "services:\n  wiki: { client: office, target: 81, bind: 7902 }\n",
    );
    d.drop_in(
        "a.yaml",
        &format!("clients:\n  office: {{ secret: \"{OTHER}\" }}\nservices:\n  blog: {{ client: home, target: 82, bind: 7901 }}\n"),
    );
    let o = d.load();
    assert!(!o.report.has_errors(), "{}", shown(&o));
    let mut names = services(&o);
    names.sort();
    assert_eq!(names, ["blog", "web", "wiki"]);
    let Some(Loaded::Server(s, _)) = &o.loaded else {
        panic!()
    };
    assert_eq!(
        s.service_files.get("wiki").map(String::as_str),
        Some("server.d/b.yaml")
    );
    assert_eq!(s.service_files.get("web"), None);
    let files: Vec<&str> = o.report.sources.iter().map(|s| s.name.as_str()).collect();
    assert!(
        files[1].ends_with("a.yaml") && files[2].ends_with("b.yaml"),
        "{files:?}"
    );
}

#[test]
fn a_server_wide_setting_in_a_drop_in_is_refused_at_its_file_and_line() {
    let d = Dir::new();
    d.drop_in("apps.yaml", "services: {}\nlisten: 9000\n");
    let o = d.load();
    assert!(o.loaded.is_none());
    let text = shown(&o);
    assert!(
        text.contains("\"listen\" cannot be set in a drop-in file"),
        "{text}"
    );
    assert!(text.contains("server.d/apps.yaml:2:1"), "{text}");
    assert!(text.contains("belong in the main config file"), "{text}");
}

#[test]
fn the_same_name_in_two_files_is_an_error_naming_both() {
    let d = Dir::new();
    d.drop_in(
        "apps.yaml",
        "services:\n  web: { client: home, target: 81 }\n",
    );
    let o = d.load();
    assert!(o.loaded.is_none());
    let text = shown(&o);
    assert!(
        text.contains("service \"web\" is defined twice: at ")
            && text.contains("server.yaml:5 and at ")
            && text.contains("apps.yaml:2"),
        "{text}"
    );
}

#[test]
fn only_files_ending_in_yaml_are_read() {
    let d = Dir::new();
    d.drop_in(
        "on.yaml",
        "services:\n  blog: { client: home, target: 82, bind: 7901 }\n",
    );
    for name in [
        "off.yaml.off",
        "old.bak",
        ".hidden.yaml",
        "notes.txt",
        "x.yaml~",
    ] {
        d.drop_in(name, "listen: 1\n");
    }
    std::fs::create_dir(d.main.parent().unwrap().join("server.d/sub.yaml")).unwrap();
    d.drop_in("legacy.yml", "listen: 1\n");
    let o = d.load();
    assert!(!o.report.has_errors(), "{}", shown(&o));
    let mut names = services(&o);
    names.sort();
    assert_eq!(names, ["blog", "web"]);
    assert_eq!(o.report.sources.len(), 2);
    let ignored: Vec<_> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::DROP_IN_IGNORED)
        .collect();
    assert_eq!(ignored.len(), 1);
    assert!(
        ignored[0].message.contains("legacy.yml"),
        "{}",
        ignored[0].message
    );
}

#[cfg(unix)]
#[test]
fn a_drop_in_or_folder_anyone_can_change_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let d = Dir::new();
    let file = d.drop_in("apps.yaml", "services: {}\n");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o602)).unwrap();
    let o = d.load();
    assert!(o.loaded.is_none());
    assert!(
        codes_of(&o).contains(&codes::DROP_IN_UNSAFE),
        "{}",
        shown(&o)
    );
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(d.load().loaded.is_some());
    let folder = file.parent().unwrap();
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o777)).unwrap();
    let o = d.load();
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(o.loaded.is_none());
    assert!(shown(&o).contains("chmod o-w"), "{}", shown(&o));
}

#[test]
fn each_kind_of_problem_in_a_drop_in_points_at_that_file_and_line() {
    for (text, code, line) in [
        (
            "services:\n  blog: { client: home, target: 82, bnd: 7901 }\n",
            "unknown-field",
            2,
        ),
        (
            "services:\n  blog:\n    client: nobody\n    target: 82\n    bind: 7901\n",
            "unknown-client",
            3,
        ),
        (
            "services:\n  blog:\n    client: home\n    target: \"${NOT_SET_HERE}\"\n    bind: 7901\n",
            codes::ENV_UNSET,
            4,
        ),
        (
            "services:\n  blog: { client: home, target: 82, bind: 70000 }\n",
            codes::OUT_OF_RANGE,
            2,
        ),
        ("services:\n  blog: [\n", codes::SYNTAX, 3),
    ] {
        let d = Dir::new();
        d.drop_in("apps.yaml", text);
        let o = d.load();
        let found = o
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == code && d.span.is_some_and(|s| s.file.index() == 1))
            .unwrap_or_else(|| panic!("{code}: {}", shown(&o)));
        assert_eq!(
            found.span.unwrap().start.line,
            line,
            "{code}: {}",
            shown(&o)
        );
        let place = format!("apps.yaml:{line}:");
        assert!(shown(&o).contains(&place), "{code}: {}", shown(&o));
    }
}

#[test]
fn clients_and_visitors_never_read_a_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("client.yaml");
    std::fs::write(
        &main,
        format!(
            "server: example.com:7835\nname: home\nsecret: \"{SECRET}\"\nallow: [127.0.0.0/8]\n"
        ),
    )
    .unwrap();
    std::fs::create_dir(tmp.path().join("client.d")).unwrap();
    std::fs::write(tmp.path().join("client.d/x.yaml"), "listen: 1\n").unwrap();
    let nothing = |_: &str| None;
    let o = load_file(Path::new(&main), Some(Role::Client), &nothing).unwrap();
    assert!(!o.report.has_errors(), "{}", shown(&o));
    assert_eq!(o.report.sources.len(), 1);
}

#[test]
fn validate_lists_the_files_read_their_items_and_which_are_read_only() {
    let d = Dir::new();
    let file = d.drop_in(
        "apps.yaml",
        "services:\n  blog: { client: home, target: 82, bind: 7901 }\n  wiki: { client: home, target: 83, bind: 7902 }\n",
    );
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&file, perms).unwrap();
    let o = d.load();
    let text = crate::config::validate::render_text(&o, false, Style { color: false });
    #[cfg(windows)]
    {
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
    }
    assert!(text.contains("read 2 files:\n"), "{text}");
    assert!(
        text.contains("  server.yaml: clients home; services web\n"),
        "{text}"
    );
    assert!(
        text.contains("  server.d/apps.yaml (read-only: it has ")
            && text.contains("): services blog, wiki\n"),
        "{text}"
    );
    assert!(
        text.contains("valid server config (1 client, 3 services)"),
        "{text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&crate::config::validate::render_json(&o, false)).unwrap();
    assert_eq!(json["files"][1]["file"], "server.d/apps.yaml");
    assert_eq!(
        json["files"][1]["services"],
        serde_json::json!(["blog", "wiki"])
    );
    let alone = Dir::new();
    let text = crate::config::validate::render_text(&alone.load(), false, Style { color: false });
    assert!(!text.contains("files"), "{text}");
}

#[test]
fn an_edit_of_the_main_file_is_checked_together_with_the_folder() {
    let d = Dir::new();
    d.drop_in(
        "apps.yaml",
        "services:\n  blog: { client: home, target: 82, bind: 7901 }\n",
    );
    let nothing = |_: &str| None;
    let edited = main_text().replace("web:", "blog:");
    let o = crate::config::save::check(&d.main, Format::Yaml, Role::Server, &edited, &nothing);
    assert!(
        codes_of(&o).contains(&codes::DUPLICATE_NAME),
        "{}",
        shown(&o)
    );
    let fine =
        crate::config::save::check(&d.main, Format::Yaml, Role::Server, &main_text(), &nothing);
    assert!(!fine.report.has_errors(), "{}", shown(&fine));
    assert_eq!(services(&fine).len(), 2);
}
