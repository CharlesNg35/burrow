use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, open_review, run};

const USER: &str = "alice";
const PASSWORD: &str = "correct horse battery";

fn app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("a port for the app");
    let port = l.local_addr().expect("an address").port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
                );
            });
        }
    });
    port
}

fn opts() -> Options {
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: format!(
            "  site:\n    client: home\n    target: 127.0.0.1:{}\n    web: {{ hosts: [site.example.com], tls: \"off\" }}\n",
            app()
        ),
        ..Options::default()
    }
}

fn get(port: u16, auth: Option<&str>) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let header = auth.map_or(String::new(), |a| format!("Authorization: Basic {a}\r\n"));
    let head =
        format!("GET / HTTP/1.1\r\nHost: site.example.com\r\n{header}Connection: close\r\n\r\n");
    s.write_all(head.as_bytes()).expect("write");
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).to_string()
}

fn passwords(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/site").await;
        c.ui.wait_for("[role=tablist]", SECS).await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        let box_ = "[data-testid=passwords]";
        c.ui.wait_for(box_, SECS).await;
        c.ui.fill(&format!("{box_} input[aria-label='User name']"), USER)
            .await;
        c.ui.fill(&format!("{box_} input[type=password]"), PASSWORD)
            .await;
        c.ui.click(&format!("{box_} [data-testid=password-add]"))
            .await;
        c.ui.wait_text(
            &format!("{box_} [data-user={USER}] [data-testid=password-status]"),
            "new password",
            SECS,
        )
        .await;
        let page: String = c.ui.eval("document.body.innerText").await;
        assert!(!page.contains(PASSWORD), "the password is never shown");
        both_widths(c, "password-added").await;
        open_review(c, "password-review").await;
        let review: String = c.ui.eval("document.body.innerText").await;
        assert!(
            !review.contains(PASSWORD) && !review.contains("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{review}"
        );
        applied(c, "password-applied").await;
        let text = c.world.config();
        assert!(text.contains("alice: \"$argon2id$"), "{text}");
        assert!(!text.contains(PASSWORD));
        c.ui.nav("/services/site").await;
        c.ui.wait_for("[role=tablist]", SECS).await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        c.ui.wait_text(
            "[data-schema='services/*/web/basic_auth'] [data-testid=password-status]",
            "password set",
            SECS,
        )
        .await;
        both_widths(c, "password-set").await;
        let port = c.world.ports["http"];
        use base64::Engine as _;
        let good = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"));
        let bad = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:wrong"));
        let started = std::time::Instant::now();
        loop {
            let answer = get(port, Some(&good));
            if answer.starts_with("HTTP/1.1 200") {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "{answer}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(get(port, None).starts_with("HTTP/1.1 401"));
        assert!(get(port, Some(&bad)).starts_with("HTTP/1.1 401"));
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_password_set_in_the_admin_ui_lets_its_user_in() {
    run("passwords", opts(), false, passwords);
}
