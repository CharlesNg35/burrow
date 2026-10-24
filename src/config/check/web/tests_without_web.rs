use crate::config::{Format, Role, load_str};

#[test]
fn a_build_without_the_web_proxy_says_so_for_web() {
    let text = "listen: 7835\nclients:\n  home: { secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }\nweb: { https: 443 }\nservices:\n  nas:\n    client: home\n    target: 443\n    web: { hosts: [nas.example.com], tls: passthrough }\n";
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
        ["this build of burrow has no web proxy, so web: cannot be used"]
    );
}
