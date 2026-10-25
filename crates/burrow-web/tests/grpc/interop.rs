use std::time::Duration;

pub const H2C: &str = "127.0.0.1:10000";
pub const TLS: &str = "127.0.0.1:10001";
pub const TLS_NAME: &str = "foo.test.google.fr";
const IMAGE: &str =
    "golang:1.25-alpine@sha256:1ae0735f00daffa3aaf1363a5184c0d2dc55c78e3db4ec70241cdac97bf84b59";
const SERVER: &str = "google.golang.org/grpc/interop/server@v1.84.0";
const NAME: &str = "burrow-test-grpc";

async fn docker(args: &[&str]) -> (bool, String) {
    let out = tokio::process::Command::new("docker")
        .args(args)
        .output()
        .await
        .expect("docker runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text.trim().to_string())
}

pub async fn remove() {
    docker(&["rm", "-f", NAME]).await;
}

pub async fn start() -> Result<(), String> {
    let (_, running) = docker(&["ps", "-q", "-f", &format!("name=^{NAME}$")]).await;
    assert!(
        running.is_empty(),
        "{NAME} is already running: another gRPC run is in progress"
    );
    remove().await;
    let script = format!(
        "go install {SERVER} && (server --port=10000 & server --port=10001 --use_tls & wait)"
    );
    let (ok, out) = docker(&[
        "run",
        "-d",
        "--network",
        "host",
        "--name",
        NAME,
        IMAGE,
        "sh",
        "-c",
        &script,
    ])
    .await;
    if !ok {
        return Err(format!("docker run failed: {out}"));
    }
    for _ in 0..600 {
        let up = |a: &'static str| async move { tokio::net::TcpStream::connect(a).await.is_ok() };
        if up(H2C).await && up(TLS).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let (_, log) = docker(&["logs", NAME]).await;
    Err(format!(
        "the gRPC interop server did not start; its log:\n{log}"
    ))
}

pub fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push(u8::try_from(v & 0x7f).unwrap() | 0x80);
        v >>= 7;
    }
    out.push(u8::try_from(v).unwrap());
    out
}

pub fn field(number: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint(number << 3 | 2);
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

pub fn number(n: u64, v: u64) -> Vec<u8> {
    let mut out = varint(n << 3);
    out.extend(varint(v));
    out
}

pub fn message(proto: &[u8]) -> Vec<u8> {
    let mut out = vec![0];
    out.extend_from_slice(&u32::try_from(proto.len()).unwrap().to_be_bytes());
    out.extend_from_slice(proto);
    out
}

pub fn head_of(data: &[u8]) -> String {
    String::from_utf8_lossy(&data[..data.len().min(80)]).into_owned()
}

pub fn messages(mut data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let all = data;
    let mut out = Vec::new();
    while !data.is_empty() {
        let bad = |why: &str| format!("{why} in {} bytes starting {:?}", all.len(), head_of(all));
        if data.len() < 5 || data[0] > 1 {
            return Err(bad("not a gRPC message frame"));
        }
        let len = usize::try_from(u32::from_be_bytes(data[1..5].try_into().unwrap())).unwrap();
        if data.len() - 5 < len {
            return Err(bad("a gRPC message longer than the body"));
        }
        out.push(data[5..5 + len].to_vec());
        data = &data[5 + len..];
    }
    Ok(out)
}

pub fn status_request(code: u64, text: &str) -> Vec<u8> {
    let mut status = number(1, code);
    status.extend(field(2, text.as_bytes()));
    field(7, &status)
}

pub fn output_request(sizes: &[u64], interval_us: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for s in sizes {
        let mut p = number(1, *s);
        p.extend(number(2, interval_us));
        out.extend(field(2, &p));
    }
    out
}

pub fn input_request(body: usize) -> Vec<u8> {
    field(1, &field(2, &vec![b'p'; body]))
}

#[test]
fn a_body_that_is_not_grpc_is_named_with_its_first_bytes() {
    assert_eq!(messages(&[0, 0, 0, 0, 1, 7]).unwrap(), [vec![7]]);
    let page = b"<!doctype html>\n<html lang=\"en\">";
    let e = messages(page).unwrap_err();
    assert!(
        e.contains("not a gRPC message frame") && e.contains("<!doctype html>"),
        "{e}"
    );
    let e = messages(&[0, 0, 0, 9, 0, 1]).unwrap_err();
    assert!(e.contains("longer than the body"), "{e}");
}
