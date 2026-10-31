use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

use super::tests::{SECRET, errors, settings};

const GOOGLE: &str = "    google:\n      issuer: https://accounts.google.com\n      client_id: 1234.apps.googleusercontent.com\n      client_secret: ${GOOGLE_SECRET}\n";
const WORK: &str = "    work:\n      issuer: https://sso.example.com/realms/work\n      client_id: burrow\n      client_secret: ${GOOGLE_SECRET}\n";

fn load(providers: &str, sites: &str) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "listen: 7835\nclients:\n  home: {{ secret: \"{SECRET}\" }}\nweb:\n  https: 443\n  http: 80\n  letsencrypt: {{ email: ops@example.com }}\n  oidc:\n{providers}services:\n{sites}"
    );
    let env = |name: &str| (name == "GOOGLE_SECRET").then(|| "from-env".to_string());
    load_str_in(
        dir.path(),
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    )
}

fn site(name: &str, rest: &str) -> String {
    format!(
        "  {name}:\n    client: home\n    target: 8080\n    web: {{ hosts: [{name}.example.com], {rest} }}\n"
    )
}

fn notes(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| !d.is_error() && d.code != "low-port")
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn a_site_logs_in_with_one_named_provider_or_true_and_the_runtime_gets_both() {
    for pick in ["google", "true", "[google]"] {
        let o = load(
            GOOGLE,
            &site(
                "wiki",
                &format!("oidc: {pick}, allow_users: [alice@gmail.com, \"@example.com\"]"),
            ),
        );
        assert!(errors(&o).is_empty(), "{pick}: {:#?}", errors(&o));
        assert!(notes(&o).is_empty(), "{pick}: {:#?}", notes(&o));
        let s = settings(o);
        let web = s.web.unwrap();
        let login = web.sites["wiki"].options.oidc.clone().unwrap();
        assert_eq!(login.providers, ["google"]);
        assert_eq!(login.allow_users, ["alice@gmail.com", "@example.com"]);
        assert_eq!(web.oidc.len(), 1);
        assert_eq!(web.oidc[0].issuer, "https://accounts.google.com");
        assert_eq!(
            web.oidc[0].client_secret.as_ref().unwrap().expose(),
            "from-env"
        );
        assert!(
            !format!("{:?}", web.written).contains("from-env"),
            "the secret is not kept for display"
        );
    }
}

#[test]
fn a_wrong_or_unclear_pick_names_the_providers() {
    let two = format!("{GOOGLE}{WORK}");
    let allow = "allow_users: [a@b.co]";
    let e = errors(&load(&two, &site("wiki", &format!("oidc: gogle, {allow}"))));
    assert_eq!(
        e,
        [
            "services.wiki.web.oidc: there is no login provider \"gogle\" under web.oidc; the names are google, work"
        ]
    );
    let e = errors(&load(&two, &site("wiki", &format!("oidc: true, {allow}"))));
    assert_eq!(
        e,
        [
            "services.wiki.web.oidc: true is unclear with 2 login providers under web.oidc; name one or a list: google or work"
        ]
    );
    let o = load(
        &two,
        &site("wiki", &format!("oidc: [google, work], {allow}")),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert_eq!(
        settings(o).web.unwrap().sites["wiki"]
            .options
            .oidc
            .clone()
            .unwrap()
            .providers,
        ["google", "work"]
    );
    let e = errors(&load(&two, &site("wiki", &format!("oidc: [], {allow}"))));
    assert_eq!(e, ["services.wiki.web.oidc: the list is empty"]);
}

#[test]
fn a_site_with_oidc_needs_https_allow_users_and_no_other_login() {
    let e = errors(&load(GOOGLE, &site("wiki", "oidc: google")));
    assert_eq!(
        e,
        ["services.wiki.web.allow_users: nobody may log in; list who may, as emails or @domain"]
    );
    let e = errors(&load(
        GOOGLE,
        &site(
            "wiki",
            "oidc: google, allow_users: [alice, \"@\", bob@example.com]",
        ),
    ));
    assert_eq!(
        e,
        [
            "services.wiki.web.allow_users: \"alice\" is neither an email address nor @domain",
            "services.wiki.web.allow_users: \"@\" is neither an email address nor @domain",
        ]
    );
    let allow = "allow_users: [a@b.co]";
    let e = errors(&load(
        GOOGLE,
        &site("wiki", &format!("oidc: google, {allow}, tls: off")),
    ));
    assert!(
        e.iter().any(|m| m.starts_with(
            "services.wiki.web.oidc: this site is served as plain HTTP (tls: off, or no web.https)"
        )),
        "{e:#?}"
    );
    let e = errors(&load(
        GOOGLE,
        &site(
            "wiki",
            &format!("oidc: google, {allow}, https_redirect: false"),
        ),
    ));
    assert_eq!(
        e,
        [
            "services.wiki.web.oidc: https_redirect: false would serve this site over plain http:// too, where the login cookie is never sent; remove it, or remove oidc"
        ]
    );
    let e = errors(&load(
        GOOGLE,
        &site(
            "wiki",
            &format!("oidc: google, {allow}, basic_auth: {{ a: x }}"),
        ),
    ));
    assert!(
        e.contains(
            &"services.wiki.web: basic_auth and oidc are both set; use one kind of login"
                .to_string()
        ),
        "{e:#?}"
    );
    let e = errors(&load(
        GOOGLE,
        &site("wiki", &format!("oidc: google, {allow}, tls: passthrough")),
    ));
    assert!(
        e.iter().any(|m| m.contains("remove oidc"))
            && e.iter().any(|m| m.contains("remove allow_users")),
        "{e:#?}"
    );
}

#[test]
fn a_provider_needs_an_issuer_url_a_client_id_and_a_secret_kept_out_of_the_file() {
    let bad = "    google:\n      issuer: accounts.google.com\n      client_id: \"\"\n      client_secret: plain-text\n";
    let e = errors(&load(
        bad,
        &site("wiki", "oidc: google, allow_users: [a@b.co]"),
    ));
    assert_eq!(
        e,
        [
            "web.oidc.google.issuer must start with https://",
            "web.oidc.google.client_id is empty",
            "web.oidc.google.client_secret is written in the file; use ${OIDC_SECRET} or file: so it stays out of the config",
        ]
    );
    let lan =
        "    home:\n      issuer: http://192.168.1.5:8080/realms/home\n      client_id: burrow\n";
    let o = load(lan, &site("wiki", "oidc: home, allow_users: [a@b.co]"));
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert_eq!(
        notes(&o),
        [
            "web.oidc.home.issuer is plain http://; burrow reaches it only on loopback or private addresses, and the client secret travels unencrypted"
        ]
    );
    let o = load(
        &format!("{GOOGLE}{WORK}"),
        &(site("wiki", "oidc: google, allow_users: [a@b.co]")
            + &site("blog", "allow_users: [a@b.co]")),
    );
    assert_eq!(
        notes(&o),
        [
            "web.oidc.work: no website uses it; set oidc: work on a site, or remove it",
            "services.blog.web.allow_users: nobody logs in without oidc, so it does nothing",
        ]
    );
}

#[test]
fn the_summary_names_the_providers_and_who_may_log_in() {
    let o = load(
        GOOGLE,
        &site(
            "wiki",
            "oidc: google, allow_users: [alice@gmail.com, \"@example.com\"]",
        ),
    );
    let text = crate::config::web_summary::summary(&settings(o))
        .unwrap()
        .lines()
        .join("\n");
    assert!(
        text.contains("-> service wiki (login with google for alice@gmail.com, @example.com)"),
        "{text}"
    );
}
