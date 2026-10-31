#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::process::Output;
use std::time::{Duration, Instant};

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn read_until(
    from: impl Read + Send + 'static,
    within: Duration,
    mut done: impl FnMut(&str) -> bool,
) -> String {
    let (tx, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + within;
    let mut out = String::new();
    while !done(&out) {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            Err(_) => break,
        }
    }
    out
}
