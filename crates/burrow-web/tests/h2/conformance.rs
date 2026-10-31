use std::time::Duration;

use crate::common::*;
use crate::kit::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2spec_passes_its_non_strict_cases() {
    let Some(h2spec) = std::env::var_os("BURROW_H2SPEC") else {
        return;
    };
    let a = app().await;
    let (up, _) = Fake::new(4);
    up.serve("site", a.addr);
    let p = start_with(
        |s| s.sites = vec![https_site("site", &["localhost"])],
        up,
        fixed(&["localhost"]),
        false,
    )
    .await;
    let port = p.https_addr().unwrap().port().to_string();
    let report = std::env::var_os("BURROW_H2SPEC_REPORT");
    let mut cmd = tokio::process::Command::new(h2spec);
    cmd.args(["-t", "-k", "-h", "localhost", "-p", &port, "-o", "5"]);
    if let Some(r) = &report {
        cmd.arg("-j").arg(r);
    }
    let run = tokio::time::timeout(Duration::from_secs(600), cmd.output())
        .await
        .expect("h2spec finishes")
        .expect("h2spec starts");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "h2spec failed:\n{out}");
    assert!(out.contains("0 failed"), "{out}");
}
