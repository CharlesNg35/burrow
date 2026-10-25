use super::tests::{errors, load};
use super::tests_site_keys::{run, warnings};

fn svc(name: &str, extra: &str, web: &str) -> String {
    format!(
        "  {name}:\n    client: home\n    target: 8080\n    bind: 18080\n{extra}    web: {web}\n"
    )
}

#[test]
fn an_http_check_needs_a_website_burrow_serves() {
    let no_web = "listen: 7835\nservices:\n  db:\n    client: home\n    target: 5432\n    bind: 5432\n    health_check: { path: /up }\n";
    assert_eq!(
        errors(&load(no_web)),
        [
            "services.db.health_check.path: an HTTP check asks a website's app for a path, and service \"db\" is not a website (no web:)"
        ]
    );
    let (_d, o) = run(
        "",
        &svc(
            "nas",
            "    health_check: { path: /up }\n",
            "{ hosts: [nas.example.com], tls: passthrough }",
        ),
    );
    assert_eq!(
        errors(&o),
        [
            "services.nas.health_check.path: the app keeps its own HTTPS (tls: passthrough), so burrow cannot ask it for a path"
        ]
    );
    let (_d, o) = run(
        "",
        &svc(
            "blog",
            "    health_check: { path: \"up\" }\n",
            "blog.example.com",
        ),
    );
    assert_eq!(
        errors(&o),
        [
            "services.blog.health_check.path: path \"up\" must start with / and hold no ?, # or *, like /api"
        ]
    );
    let (_d, o) = run(
        "",
        &svc(
            "blog",
            "    health_check: { path: /healthz, interval_secs: 5 }\n",
            "blog.example.com",
        ),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
}

#[test]
fn a_grpc_site_and_a_wildcard_only_site_get_notes_for_their_check() {
    let (_d, o) = run(
        "  certificates: [{ hosts: [\"*.apps.example.com\"], cert: w.pem, key: w.key }]\n",
        &(svc(
            "grpc",
            "    health_check: { path: /up }\n",
            "{ hosts: [grpc.example.com], target_http2: true }",
        ) + &svc(
            "apps",
            "    health_check: { path: /up }\n",
            "[\"*.apps.example.com\"]",
        )),
    );
    let w = warnings(&o);
    assert!(w.contains(&"services.grpc.health_check.path: gRPC apps usually do not answer a plain GET; use health_check: true (the TCP check)".to_string()), "{w:#?}");
    assert!(w.contains(&"services.apps.health_check.path: the check asks for apps.example.com, the base name of \"*.apps.example.com\"; the app must answer that name, or list a plain name in hosts for service \"apps\"".to_string()), "{w:#?}");
}

#[test]
fn target_http2_gets_notes_where_grpc_clients_cannot_reach_it() {
    let (_d, o) = run(
        "  http2: false\n",
        &svc(
            "grpc",
            "",
            "{ hosts: [grpc.example.com], target_http2: true }",
        ),
    );
    assert!(warnings(&o).contains(&"services.grpc.web.target_http2: web.http2 is false, so gRPC clients cannot reach this site (gRPC needs HTTP/2); turn http2 on".to_string()), "{:#?}", warnings(&o));
    let (_d, o) = run(
        "",
        &svc(
            "grpc",
            "",
            "{ hosts: [grpc.example.com], tls: off, target_http2: true }",
        ),
    );
    assert!(warnings(&o).contains(&"services.grpc.web.target_http2: this site has tls: off; gRPC clients cannot connect, burrow accepts HTTP/2 from visitors only over TLS".to_string()), "{:#?}", warnings(&o));
    let (_d, o) = run(
        "",
        &svc(
            "nas",
            "",
            "{ hosts: [nas.example.com], tls: passthrough, target_http2: true }",
        ),
    );
    assert_eq!(
        errors(&o),
        [
            "services.nas.web.target_http2: burrow cannot see the requests of a passthrough service (tls: passthrough); remove target_http2, or let burrow handle TLS"
        ]
    );
}
