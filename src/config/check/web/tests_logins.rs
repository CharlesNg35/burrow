use super::tests::errors;
use super::tests_site_keys::{errs, run, warnings};

const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$3Zrnt9ZV6D9A/Ed3Je/Hww$lg26ncNCHaoX2amIl0p8mWtR1BCyscvYzi1EmeCfbvg";

const ONE: &str = "  forward_auth:\n    authelia: { service: authelia, path: /api/authz/forward-auth, copy_headers: [Remote-User] }\n";

const TWO: &str = "  forward_auth:\n    authelia: { service: authelia, path: /api/authz/forward-auth }\n    authelia-basic: { service: authelia, path: /api/authz/basic }\n";

fn svc(name: &str, web: &str) -> String {
    format!("  {name}:\n    client: home\n    target: 8080\n    web: {web}\n")
}

fn wiki(login: &str) -> String {
    svc(
        "wiki",
        &format!("{{ hosts: [wiki.example.com], forward_auth: {login} }}"),
    )
}

fn portal() -> String {
    svc("authelia", "auth.example.com")
}

fn notes(o: &crate::config::Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| !d.is_error() && d.code != "low-port")
        .map(|d| d.message.clone())
        .collect()
}

fn helps(top: &str, services: &str) -> Vec<String> {
    run(top, services)
        .1
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{} | {}", d.message, d.help.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn a_site_names_its_login_service_or_says_true_when_there_is_only_one() {
    for login in ["authelia", "true"] {
        let (_d, o) = run(ONE, &(wiki(login) + &portal()));
        assert!(errors(&o).is_empty(), "{login}: {:#?}", errors(&o));
        assert!(notes(&o).is_empty(), "{login}: {:#?}", notes(&o));
    }
    let both = wiki("authelia")
        + &svc(
            "dav",
            "{ hosts: [dav.example.com], forward_auth: authelia-basic }",
        );
    let (_d, o) = run(TWO, &(both + &portal()));
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
}

#[test]
fn a_login_service_that_is_missing_or_unclear_is_an_error_with_the_names() {
    assert_eq!(
        helps(TWO, &(wiki("authelai") + &portal())),
        [
            "services.wiki.web.forward_auth: there is no login service \"authelai\" under web.forward_auth; the names are authelia, authelia-basic | did you mean \"authelia\"?"
        ]
    );
    assert_eq!(
        helps(TWO, &(wiki("zzz") + &portal())),
        [
            "services.wiki.web.forward_auth: there is no login service \"zzz\" under web.forward_auth; the names are authelia, authelia-basic | pick one of them, or add it under web.forward_auth"
        ]
    );
    assert_eq!(
        helps("", &wiki("sso")),
        [
            "services.wiki.web.forward_auth: there is no login service \"sso\" under web.forward_auth; none are defined | add web.forward_auth: {sso: {service, path}}"
        ]
    );
    assert_eq!(
        helps("", &wiki("true")),
        [
            "services.wiki.web.forward_auth: true uses web.forward_auth, which is not set; add web.forward_auth: {authelia: {service, path}} | for example web.forward_auth: {authelia: {service: authelia, path: /api/authz/forward-auth}}"
        ]
    );
    let unclear = wiki("true")
        + &svc(
            "dav",
            "{ hosts: [dav.example.com], forward_auth: authelia-basic }",
        );
    assert_eq!(
        helps(TWO, &(unclear + &portal())),
        [
            "services.wiki.web.forward_auth: true is unclear with 2 login services under web.forward_auth; name one: authelia or authelia-basic | for example forward_auth: authelia"
        ]
    );
}

#[test]
fn the_login_portal_itself_never_sits_behind_any_login() {
    let top = "  forward_auth:\n    authelia: { service: authelia, path: /api/authz/forward-auth }\n    authentik: { service: authentik, path: /outpost.goauthentik.io/auth/caddy }\n";
    let s = wiki("authelia")
        + &svc(
            "authentik",
            "{ hosts: [ak.example.com], forward_auth: authentik }",
        )
        + &svc(
            "authelia",
            "{ hosts: [auth.example.com], forward_auth: authentik }",
        );
    assert_eq!(
        errs(top, &s),
        [
            "services.authentik.web.forward_auth: authentik answers the logins of web.forward_auth.authentik, so its own site must stay open",
            "services.authelia.web.forward_auth: authelia answers the logins of web.forward_auth.authelia, so its own site must stay open",
        ]
    );
}

#[test]
fn login_service_names_are_names_and_each_one_is_used() {
    let top = "  forward_auth:\n    \"true\": { service: authelia, path: /a }\n    \"a b\": { service: authelia, path: /a }\n    authentik: { service: authelia, path: /a }\n    authelia: { service: authelia, path: /a }\n";
    let (_d, o) = run(top, &(wiki("authelia") + &portal()));
    let e = errors(&o);
    assert!(e.contains(&"web.forward_auth.true: true and false cannot name a login service; they mean on and off".to_string()), "{e:#?}");
    assert!(
        e.contains(&"invalid login service name \"a b\"".to_string()),
        "{e:#?}"
    );
    assert_eq!(
        notes(&o),
        [
            "web.forward_auth.authentik: no website uses it; set forward_auth: authentik on a site, or remove it"
        ]
    );
}

#[test]
fn each_login_service_names_a_service_of_this_config() {
    let top = "  forward_auth:\n    authelia: { service: nobody, path: /auth }\n";
    let found = helps(top, &wiki("authelia"));
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].starts_with(
            "unknown service \"nobody\" referenced by web.forward_auth.authelia.service"
        ),
        "{found:#?}"
    );
}

#[test]
fn the_old_inline_block_says_to_name_a_login_service() {
    let inline = wiki("{ service: authelia, path: /auth }") + &portal();
    let found = helps("", &inline);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .ends_with("| name a login service from web.forward_auth, like forward_auth: authelia"),
        "{found:#?}"
    );
    let shared = "  forward_auth: { service: authelia, path: /auth }\n";
    let found = helps(shared, &(wiki("true") + &portal()));
    assert!(
        found.iter().any(|f| f.ends_with(
            "| each login service has its own name, like web.forward_auth: {authelia: {service, path}}"
        )),
        "{found:#?}"
    );
}

#[test]
fn passwords_and_a_login_service_do_not_mix_and_the_old_notes_stay() {
    let both = svc(
        "wiki",
        &format!(
            "{{ hosts: [wiki.example.com], basic_auth: {{ alice: \"{HASH}\" }}, forward_auth: authelia }}"
        ),
    );
    assert_eq!(
        errs(ONE, &(both + &portal())),
        [
            "services.wiki.web: basic_auth and forward_auth are both set; use one of them (the login service checks its own users)"
        ]
    );
    let h2 = wiki("authelia")
        + &svc(
            "authelia",
            "{ hosts: [auth.example.com], target_http2: true, target_tls: true }",
        );
    let (_d, o) = run(ONE, &h2);
    assert!(
        warnings(&o).contains(&"services.wiki.web.forward_auth: the login service \"authelia\" (authelia) speaks HTTP/2 only to its app (target_http2 with target_tls), but the login check asks over HTTP/1.1, so every check would fail with 502".to_string()),
        "{:#?}",
        warnings(&o)
    );
}
