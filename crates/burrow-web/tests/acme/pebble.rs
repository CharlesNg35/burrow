use std::path::PathBuf;
use std::time::Duration;

use burrow_web::tls::acme::http::{Client, Limits};
use bytes::Bytes;
use instant_acme::{BodyWrapper, HttpClient};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::pki_types::pem::PemObject;

pub const DIRECTORY: &str = "https://localhost:14000/dir";
const PEBBLE: &str = "ghcr.io/letsencrypt/pebble:2.10.1";
const CHALLTESTSRV: &str = "ghcr.io/letsencrypt/pebble-challtestsrv:2.10.1";
const PEBBLE_NAME: &str = "burrow-test-pebble";
const DNS_NAME: &str = "burrow-test-challtestsrv";

pub struct Pebble {
    pub dir: tempfile::TempDir,
    pub tls_port: u16,
    pub http_port: u16,
    pub ca: PathBuf,
}

async fn docker(args: &[&str]) -> String {
    let out = tokio::process::Command::new("docker")
        .args(args)
        .output()
        .await
        .expect("docker runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

fn config(tls_port: u16, http_port: u16) -> String {
    serde_json::json!({"pebble": {
        "listenAddress": "0.0.0.0:14000",
        "managementListenAddress": "0.0.0.0:15000",
        "certificate": "test/certs/localhost/cert.pem",
        "privateKey": "test/certs/localhost/key.pem",
        "httpPort": http_port,
        "tlsPort": tls_port,
        "ocspResponderURL": "",
        "externalAccountBindingRequired": false,
        "retryAfter": {"authz": 3, "order": 5},
        "keyAlgorithm": "ecdsa",
        "profiles": {
            "classic": {"description": "90 days, like Let's Encrypt", "validityPeriod": 7_776_000},
            "tlsserver": {"description": "45 days", "validityPeriod": 3_888_000},
            "shortlived": {"description": "160 hours", "validityPeriod": 576_000}
        }
    }})
    .to_string()
}

impl Pebble {
    pub async fn start() -> Pebble {
        for name in [PEBBLE_NAME, DNS_NAME] {
            let running = docker(&["ps", "-q", "-f", &format!("name=^{name}$")]).await;
            assert!(
                running.is_empty(),
                "{name} is already running: another Pebble run is in progress on this host (stop it with docker rm -f {name} if it is left over)"
            );
        }
        docker(&["rm", "-f", PEBBLE_NAME, DNS_NAME]).await;
        docker(&[
            "run",
            "-d",
            "--rm",
            "--network",
            "host",
            "--name",
            DNS_NAME,
            CHALLTESTSRV,
            "-defaultIPv4",
            "127.0.0.1",
            "-defaultIPv6",
            "",
            "-dnsserver",
            ":8053",
            "-http01",
            "",
            "-https01",
            "",
            "-tlsalpn01",
            "",
            "-doh",
            "",
            "-management",
            ":8055",
        ])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let (tls_port, http_port) = (free_port().await, free_port().await);
        std::fs::write(
            dir.path().join("pebble-config.json"),
            config(tls_port, http_port),
        )
        .unwrap();
        let ca = dir.path().join("minica.pem");
        let p = Pebble {
            dir,
            tls_port,
            http_port,
            ca,
        };
        p.run_pebble(&["PEBBLE_AUTHZREUSE=0"]).await;
        p
    }

    pub async fn run_pebble(&self, env: &[&str]) {
        docker(&["rm", "-f", PEBBLE_NAME]).await;
        let mount = format!("{}:/burrow:ro", self.dir.path().display());
        let mut args = vec![
            "run",
            "-d",
            "--rm",
            "--network",
            "host",
            "--name",
            PEBBLE_NAME,
        ];
        args.extend(["-e", "PEBBLE_VA_NOSLEEP=1", "-v", &mount]);
        for e in env {
            args.extend(["-e", e]);
        }
        args.extend([
            PEBBLE,
            "-config",
            "/burrow/pebble-config.json",
            "-dnsserver",
            "127.0.0.1:8053",
        ]);
        docker(&args).await;
        for _ in 0..100 {
            if TcpStream::connect("127.0.0.1:14000").await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let from = format!("{PEBBLE_NAME}:/test/certs/pebble.minica.pem");
        docker(&["cp", &from, &self.ca.display().to_string()]).await;
        assert!(self.ca.exists(), "Pebble's API CA was copied out");
    }

    pub async fn pause(&self, paused: bool) {
        docker(&[if paused { "pause" } else { "unpause" }, PEBBLE_NAME]).await;
    }

    async fn call(&self, method: http::Method, url: &str, body: Vec<u8>) -> (u16, Bytes) {
        let client = Client::new(Some(&self.ca), Limits::default()).unwrap();
        let req = http::Request::builder()
            .method(method)
            .uri(url)
            .header(http::header::USER_AGENT, "burrow-test")
            .body(BodyWrapper::from(body))
            .unwrap();
        let mut resp = client.request(req).await.unwrap();
        let status = resp.parts.status.as_u16();
        (status, resp.body.into_bytes().await.unwrap())
    }

    pub async fn root(&self) -> CertificateDer<'static> {
        let (_, pem) = self
            .call(
                http::Method::GET,
                "https://localhost:15000/roots/0",
                Vec::new(),
            )
            .await;
        CertificateDer::from_pem_slice(&pem).expect("Pebble's root")
    }

    pub async fn set_renewal_info(&self, cert_pem: &str, answer: &str) {
        let body = serde_json::json!({"Certificate": cert_pem, "ARIResponse": answer});
        let (status, text) = self
            .call(
                http::Method::POST,
                "https://localhost:15000/set-renewal-info/",
                serde_json::to_vec(&body).unwrap(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "set-renewal-info: {}",
            String::from_utf8_lossy(&text)
        );
    }
}

pub async fn remove_containers() {
    docker(&["rm", "-f", PEBBLE_NAME, DNS_NAME]).await;
}
