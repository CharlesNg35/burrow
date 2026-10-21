#![no_main]

use burrow::update::{archive, http};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = http::Url::parse(text);
        let _ = burrow::update::find_asset(text, "linux-amd64", "tar.gz");
    }
    let _ = archive::from_zip(data, "burrow.exe");
    let _ = archive::from_tar_gz(data, "burrow");
});
