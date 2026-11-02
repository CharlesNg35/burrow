#[path = "../tests/common/releases.rs"]
mod releases;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || !args.len().is_multiple_of(3) {
        burrow::say_errln!("usage: release_server (VERSION BINARY good|tampered)...");
        std::process::exit(2);
    }
    let r = releases::Releases::start();
    for spec in args.chunks(3) {
        let binary = std::fs::read(&spec[1]).expect("a readable binary");
        r.publish(&spec[0], &binary, spec[2] == "tampered");
    }
    println!("{}", r.base);
    loop {
        std::thread::park();
    }
}
