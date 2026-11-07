use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, open, overview_tab, run};

const TOP: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n";
const MORE: &str = "  pool:\n    clients: [home, { name: office, target: 127.0.0.1:9 }]\n    target: 22\n    health_check: true\n    visitors: [laptop]\n  nas:\n    client: home\n    target: 192.168.1.20:443\n    web:\n      hosts: [nas.example.com, \"*.nas.example.com\"]\n      tls: passthrough\n";
const WIDTHS: [i64; 3] = [1440, 1280, 390];

fn opts() -> Options {
    Options {
        extra_top: TOP.into(),
        extra_services: MORE.into(),
        ..Options::default()
    }
}

async fn widths(c: &mut Ctx, step: &str, ready: &str) {
    for w in WIDTHS {
        c.ui.viewport(w, 900).await;
        c.ui.wait_for(ready, SECS).await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        c.ui.shot(&format!("{step}-{w}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

fn shots(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let key = rcgen::KeyPair::generate().expect("a key");
        let cert = rcgen::CertificateParams::new(vec!["portal.example.com".to_string()])
            .expect("params")
            .self_signed(&key)
            .expect("a certificate");
        c.world.write("portal.pem", &cert.pem());
        c.world.write("portal.key", &key.serialize_pem());
        c.world.hand_edit(|t| {
            format!(
                "{t}  portal:\n    client: home\n    target: 8443\n    web:\n      hosts: [portal.example.com]\n      tls: {{ cert: portal.pem, key: portal.key }}\n"
            )
        });
        for (tab, ready) in [
            (
                "services",
                "[data-testid=overview-tabs] [data-testid=overview-services]",
            ),
            (
                "clients",
                "[data-testid=overview-tabs] [data-testid=overview-clients]",
            ),
            (
                "visitors",
                "[data-testid=overview-tabs] [data-testid=overview-visitors]",
            ),
            (
                "websites",
                "[data-testid=overview-tabs] [data-tab=websites]",
            ),
            ("certs", "[data-testid=overview-tabs] [data-cert]"),
        ] {
            c.ui.nav("/").await;
            overview_tab(c, tab).await;
            widths(c, &format!("overview-{tab}"), ready).await;
        }
        for (path, step, ready) in [
            (
                "/services/pool",
                "service-health",
                "[data-testid=health] tbody tr",
            ),
            ("/clients/home", "client-page", "[data-testid=live]"),
            ("/visitors/laptop", "visitor-page", "main h1"),
            ("/clients/office", "client-offline", "[data-testid=live]"),
            ("/settings", "settings", "[data-testid=settings]"),
            ("/activity", "activity", "[data-testid=activity]"),
        ] {
            c.ui.nav(path).await;
            widths(c, step, ready).await;
        }
        c.ui.assert_no_errors();
    })
}

#[test]
fn every_table_lines_up_at_three_widths_in_both_schemes() {
    run("table-shots", opts(), false, shots);
}
