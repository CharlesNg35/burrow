#![no_main]

use burrow_syntax::{duration, hostport, ip, name, ports, rate, rule, secret_fmt, target};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = name::check_name(s);
    let _ = name::suggest(s, &["a", s]);
    let _ = hostport::parse_host_port(s);
    let _ = hostport::check_host(s);
    let _ = ip::parse_ip(s);
    let _ = ports::parse_range(s);
    let _ = ports::parse_ranged_text(s, [0, 0, 0, 0], "bind");
    let _ = ports::split_target(s);
    let _ = rate::parse_rate(s);
    let _ = rate::split(s);
    if let Ok(r) = s.parse::<rule::Rule>() {
        assert_eq!(r.to_string().parse::<rule::Rule>().ok(), Some(r));
    }
    let _ = rule::parse_cidr_loose(s);
    if let Ok(t) = target::parse_target(s) {
        assert_eq!(target::parse_target(&t.full()).ok(), Some(t.clone()));
    }
    let _ = target::parse_bind(s, [0, 0, 0, 0], 1000);
    let _ = duration::to_stored(s, duration::TimeUnit::Mins, duration::TimeUnit::Secs);
    let _ = secret_fmt::env_vars(s);
    let _ = secret_fmt::looks_like_secret(s);
});
