#![no_main]

use burrow::config::{Format, Role, load_str};
use libfuzzer_sys::fuzz_target;

fn env(name: &str) -> Option<String> {
    (name == "SET").then(|| "value".to_string())
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for format in [Format::Yaml, Format::Json] {
        for role in [None, Some(Role::Server), Some(Role::Client)] {
            let o = load_str("fuzz.yaml", text, format, role, &env);
            let _ = o
                .report
                .render_text(burrow::diagnostics::Style { color: false });
            let _ = o.report.render_json();
        }
    }
});
