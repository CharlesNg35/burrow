use std::net::SocketAddr;
use std::time::Duration;

use hickory_proto::op::{Message, Query};
use hickory_proto::rr::{Name, RData, RecordType};

pub const ADDR: &str = "127.0.0.1:5399";
pub const KEY: &str = "burrow-key";
pub const SECRET: &str = "DeMe+aO2zaj8MLjzn5TsmuIjSk+5vtEvG3W7kq/1j04=";
const IMAGE: &str = "internetsystemsconsortium/bind9:9.20";
const NAME: &str = "burrow-test-bind";

pub struct Bind {
    _dir: tempfile::TempDir,
}

async fn docker(args: &[&str]) -> String {
    let out = tokio::process::Command::new("docker")
        .args(args)
        .output()
        .await
        .expect("docker runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub async fn remove() {
    docker(&["rm", "-f", NAME]).await;
}

fn zone(origin: &str, extra: &str) -> String {
    format!(
        "$TTL 60\n@ IN SOA ns.example.test. admin.{origin}. 1 60 60 600 60\n@ IN NS ns.example.test.\n@ IN A 127.0.0.1\n{extra}"
    )
}

const CONF: &str = r#"options {
  directory "/var/cache/bind";
  listen-on port 5399 { 127.0.0.1; };
  listen-on-v6 { none; };
  recursion no;
  allow-query { any; };
  dnssec-validation no;
};
key "burrow-key" { algorithm hmac-sha256; secret "DeMe+aO2zaj8MLjzn5TsmuIjSk+5vtEvG3W7kq/1j04="; };
zone "example.test" {
  type primary;
  file "/var/cache/bind/example.test.zone";
  update-policy { grant burrow-key subdomain example.test. TXT; };
};
"#;

impl Bind {
    pub async fn start() -> Bind {
        let running = docker(&["ps", "-q", "-f", &format!("name=^{NAME}$")]).await;
        assert!(
            running.is_empty(),
            "{NAME} is already running: another DNS-01 run is in progress"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("named.conf"), CONF).unwrap();
        std::fs::write(
            dir.path().join("example.test.zone"),
            zone(
                "example.test",
                "ns IN A 127.0.0.1\n*.example.test. IN A 127.0.0.1\n_acme-challenge.erp IN CNAME acme-target.example.test.\n",
            ),
        )
        .unwrap();
        let mount = format!("{}:/conf:ro", dir.path().display());
        docker(&[
            "run", "-d", "--rm", "--network", "host", "--name", NAME, "-v", &mount, "--entrypoint", "sh", IMAGE, "-c",
            "cp /conf/*.zone /var/cache/bind/ && chown -R bind /var/cache/bind && exec named -u bind -g -c /conf/named.conf",
        ])
        .await;
        for _ in 0..100 {
            if txt("example.test").await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Bind { _dir: dir }
    }
}

pub async fn txt(name: &str) -> Option<Vec<String>> {
    let addr: SocketAddr = ADDR.parse().unwrap();
    let mut q = Message::query();
    let mut n = Name::from_ascii(name).unwrap();
    n.set_fqdn(true);
    q.add_query(Query::query(n, RecordType::TXT));
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.ok()?;
    sock.send_to(&q.to_vec().ok()?, addr).await.ok()?;
    let mut buf = vec![0u8; 4096];
    let (len, _) = tokio::time::timeout(Duration::from_millis(500), sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    let m = Message::from_vec(&buf[..len]).ok()?;
    Some(
        m.answers
            .iter()
            .filter_map(|r| match &r.data {
                RData::TXT(t) => Some(
                    t.txt_data
                        .iter()
                        .map(|d| String::from_utf8_lossy(d).into_owned())
                        .collect(),
                ),
                _ => None,
            })
            .collect(),
    )
}
