use super::tests::{errors, load, settings};

fn shared(tls: &str, path: &str) -> String {
    format!(
        "web: {{ https: 443, http: 80 }}\nwebsocket: {{ listen: 443, host: blog.example.com, path: {path} }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: {{ hosts: [blog.example.com], tls: {tls} }}\n"
    )
}

const FILES: &str = "{ cert: certs/blog.pem, key: certs/blog.key }";

#[test]
fn the_tunnel_may_take_a_path_on_a_terminated_sites_name_without_its_own_file() {
    let o = load(&shared(FILES, "/burrow"));
    assert!(errors(&o).is_empty(), "{:?}", errors(&o));
    let s = settings(o);
    assert_eq!(
        s.websocket.unwrap().host.as_deref(),
        Some("blog.example.com")
    );
    assert_eq!(s.web.unwrap().tunnel.as_deref(), Some("blog.example.com"));
}

#[test]
fn the_tunnel_cannot_share_a_passthrough_or_plain_sites_name() {
    let pass = errors(&load(&shared("passthrough", "/burrow")));
    assert!(
        pass.contains(&"websocket.host \"blog.example.com\" is a passthrough site; burrow cannot see its requests".to_string()),
        "{pass:?}"
    );
    let plain = errors(&load(&shared("off", "/burrow")));
    assert!(
        plain.contains(&"websocket.host \"blog.example.com\" is a plain-HTTP site (tls: off); the tunnel needs HTTPS".to_string()),
        "{plain:?}"
    );
}

#[test]
fn the_tunnel_path_must_leave_the_site_the_rest_of_its_name() {
    let e = errors(&load(&shared(FILES, "/")));
    assert!(
        e.contains(&"websocket.path \"/\" on \"blog.example.com\" is also served by service \"blog\"; give the tunnel another path".to_string()),
        "{e:?}"
    );
}
