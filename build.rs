fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=BURROW_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
