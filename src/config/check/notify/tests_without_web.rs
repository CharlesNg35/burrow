use crate::config::{Format, Role, load_str};

#[test]
fn a_build_without_notifications_says_so_for_notify() {
    let text = "listen: 7835\nclients:\n  home: { secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }\nservices:\n  ssh: { client: home, target: 22, bind: 2222 }\nnotify:\n  - ntfy: https://ntfy.sh/t\n";
    let o = load_str(
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &|_: &str| None,
    );
    let errors: Vec<&str> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        errors,
        ["this build of burrow has no notifications, so notify: cannot be used"]
    );
}
