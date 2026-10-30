use std::time::Duration;

use burrow_web::tls::acme::http::{Client, Limits};
use http::{Method, Request};
use tokio::net::TcpStream;

use super::bind::txt_at;
use super::pebble::{docker, docker_logs};

pub const DOMAIN: &str = "auth.acme.test";
pub const DNS: &str = "127.0.0.1:8153";
pub const NS_IP: &str = "127.0.0.2";
pub const API: &str = "http://127.0.0.1:8180";
const API_ADDR: &str = "127.0.0.1:8180";
const IMAGE: &str = "joohoi/acme-dns:v2.0.2@sha256:8d327df58b1c9f6361c76734f9761a108fa4f12cf7ca94440a0404a28f75de62";
const NAME: &str = "burrow-test-acme-dns";

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Account {
    pub username: String,
    pub password: String,
    pub subdomain: String,
    pub fulldomain: String,
}

pub struct AcmeDns {
    pub good: Account,
    pub other: Account,
    _dir: tempfile::TempDir,
}

pub async fn remove() {
    docker(&["rm", "-f", "-v", NAME]).await;
}

pub fn forward_zone() -> String {
    let (ip, port) = DNS.split_once(':').unwrap();
    format!(
        "zone \"{DOMAIN}\" {{\n  type forward;\n  forward only;\n  forwarders {{ {ip} port {port}; }};\n}};\n"
    )
}

fn config() -> String {
    let (api_ip, port) = API_ADDR.split_once(':').unwrap();
    format!(
        r#"[general]
listen = "{DNS}"
protocol = "both4"
domain = "{DOMAIN}"
nsname = "ns.{DOMAIN}"
nsadmin = "admin.{DOMAIN}"
records = ["{DOMAIN}. NS ns.{DOMAIN}.", "ns.{DOMAIN}. A {NS_IP}"]
debug = false

[database]
engine = "sqlite"
connection = "/var/lib/acme-dns/acme-dns.db"

[api]
ip = "{api_ip}"
disable_registration = false
port = "{port}"
tls = "none"
corsorigins = ["*"]
use_header = false
header_name = "X-Forwarded-For"

[logconfig]
loglevel = "info"
logtype = "stdout"
logformat = "text"
"#
    )
}

async fn register() -> Result<Account, String> {
    let client = Client::new(None, Limits::default())?.with_plain_private();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("{API}/register"))
        .body(Vec::new())
        .map_err(|e| e.to_string())?;
    let resp = client.fetch(req).await?;
    if resp.status() != http::StatusCode::CREATED {
        return Err(format!("acme-dns /register answered {}", resp.status()));
    }
    serde_json::from_slice(resp.body()).map_err(|e| format!("acme-dns /register: {e}"))
}

impl AcmeDns {
    pub async fn start() -> Result<AcmeDns, String> {
        let running = docker(&["ps", "-q", "-f", &format!("name=^{NAME}$")]).await;
        assert!(
            running.is_empty(),
            "{NAME} is already running: another DNS-01 run is in progress"
        );
        remove().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.cfg"), config()).unwrap();
        let mount = format!("{}:/etc/acme-dns:ro", dir.path().display());
        docker(&[
            "run",
            "-d",
            "--network",
            "host",
            "--name",
            NAME,
            "-v",
            &mount,
            IMAGE,
        ])
        .await;
        for _ in 0..40 {
            let api = TcpStream::connect(API_ADDR).await.is_ok();
            if api && txt_at(DNS, &format!("ns.{DOMAIN}")).await.is_some() {
                let good = register().await?;
                let other = register().await?;
                return Ok(AcmeDns {
                    good,
                    other,
                    _dir: dir,
                });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!(
            "acme-dns did not start; its log:\n{}",
            docker_logs(NAME).await
        ))
    }

    pub fn records(&self) -> String {
        format!(
            "_acme-challenge.adns IN CNAME {}.\n_acme-challenge.bad-adns IN CNAME {}.\n",
            self.good.fulldomain, self.other.fulldomain
        )
    }
}
