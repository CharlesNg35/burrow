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

fn plain_by_path(site: &str) -> Vec<String> {
    let o = load(&format!(
        "listen: 7835\nweb: {{ http: 80 }}\nwebsocket: {{ listen: 80, host: blog.example.com, path: /burrow }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: {site}\n"
    ));
    o.report
        .diagnostics
        .iter()
        .filter(|d| !["low-port", "plain-http"].contains(&d.code))
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

#[test]
fn the_tunnel_on_web_http_takes_a_path_on_a_plain_sites_name() {
    for site in [
        "{ hosts: [blog.example.com], tls: off }",
        "blog.example.com",
    ] {
        assert_eq!(plain_by_path(site), Vec::<String>::new(), "{site}");
    }
    let o = load(
        "listen: 7835\nweb: { http: 80 }\nwebsocket: { listen: 80, host: blog.example.com, path: / }\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: blog.example.com\n",
    );
    assert!(
        errors(&o).contains(&"websocket.path \"/\" on \"blog.example.com\" is also served by service \"blog\"; give the tunnel another path".to_string()),
        "{:?}",
        errors(&o)
    );
}

#[test]
fn a_site_path_differing_from_the_tunnel_path_only_by_case_is_refused() {
    let text = "web: { https: 443, http: 80 }\nwebsocket: { listen: 443, host: blog.example.com, path: /burrow }\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: { hosts: [blog.example.com], tls: { cert: certs/blog.pem, key: certs/blog.key } }\n  app:\n    client: home\n    target: 8081\n    web: { hosts: [blog.example.com], paths: [/Burrow/x], tls: { cert: certs/blog.pem, key: certs/blog.key } }\n";
    let e = errors(&load(text));
    assert!(
        e.contains(&"websocket.path \"/burrow\" on \"blog.example.com\" is also served by service \"app\"; give the tunnel another path".to_string()),
        "{e:?}"
    );
}
