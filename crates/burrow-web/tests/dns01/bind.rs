use std::net::SocketAddr;
use std::time::Duration;

use hickory_proto::op::{Message, Query};
use hickory_proto::rr::{Name, RData, RecordType};

pub const ADDR: &str = "127.0.0.1:5399";
pub const KEY: &str = "burrow-key";
pub const SECRET: &str = "DeMe+aO2zaj8MLjzn5TsmuIjSk+5vtEvG3W7kq/1j04=";
pub const KEY_384: &str = "burrow-key-384";
pub const SECRET_384: &str = "r2F4xx/e095hTZXFDba7O5Lon/p4i5QLiZgIGTzz4Ldl7wd6BOXhVzwXCCz3tQJj";
pub const KEY_512: &str = "burrow-key-512";
pub const SECRET_512: &str =
    "JcVH5r9JhcHXNC/6Y7NX+s68xVzS2QyiRINH0qfiqib6ZLCa8/f6LYZGEIlD52BqBNNKmZokhJMa3miqkCVcnA==";
const IMAGE: &str = "internetsystemsconsortium/bind9:9.20@sha256:66092edb53c081fa11b1fa8219e99065b0f497cb38f568ba789702a4c10e2590";
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
key "burrow-key-384" { algorithm hmac-sha384; secret "r2F4xx/e095hTZXFDba7O5Lon/p4i5QLiZgIGTzz4Ldl7wd6BOXhVzwXCCz3tQJj"; };
key "burrow-key-512" { algorithm hmac-sha512; secret "JcVH5r9JhcHXNC/6Y7NX+s68xVzS2QyiRINH0qfiqib6ZLCa8/f6LYZGEIlD52BqBNNKmZokhJMa3miqkCVcnA=="; };
zone "example.test" {
  type primary;
  file "/var/cache/bind/example.test.zone";
  update-policy {
    grant burrow-key subdomain example.test. TXT;
    grant burrow-key-384 subdomain example.test. TXT;
    grant burrow-key-512 subdomain example.test. TXT;
  };
};
"#;

impl Bind {
    pub async fn start() -> Result<Bind, String> {
        let running = docker(&["ps", "-q", "-f", &format!("name=^{NAME}$")]).await;
        assert!(
            running.is_empty(),
            "{NAME} is already running: another DNS-01 run is in progress"
        );
        remove().await;
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
            "run", "-d", "--network", "host", "--name", NAME, "-v", &mount, "--entrypoint", "sh", IMAGE, "-c",
            "cp /conf/*.zone /var/cache/bind/ && chown -R bind /var/cache/bind && exec named -u bind -g -c /conf/named.conf",
        ])
        .await;
        for _ in 0..40 {
            if txt("example.test").await.is_some() {
                return Ok(Bind { _dir: dir });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let out = tokio::process::Command::new("docker")
            .args(["logs", NAME])
            .output()
            .await
            .map(|o| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                )
            })
            .unwrap_or_default();
        Err(format!("BIND did not answer on {ADDR}; its log:\n{out}"))
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
