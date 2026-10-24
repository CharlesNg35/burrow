#[path = "diagnostics/client_and_json.rs"]
mod client_and_json;
#[path = "diagnostics/limits.rs"]
mod limits;
#[path = "diagnostics/self_target.rs"]
mod self_target;
#[path = "diagnostics/server_config.rs"]
mod server_config;
#[path = "diagnostics/services.rs"]
mod services;
#[path = "diagnostics/values.rs"]
mod values;
#[path = "diagnostics/visitors.rs"]
mod visitors;
#[path = "diagnostics/websocket.rs"]
mod websocket;

use burrow::base::style::Style;
use burrow::config::{Format, Role, load_str};

const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";
const SECRET2: &str = "q4x7Z0m3VQ1n8kPp2rT6sW9yB5cD0eF3gH7jK1lM4nA=";

fn env(name: &str) -> Option<String> {
    match name {
        "HOME_SECRET" => Some(SECRET2.to_string()),
        _ => None,
    }
}

fn render(name: &str, src: &str, role: Option<Role>) -> String {
    let format = if name.ends_with(".json") {
        Format::Json
    } else {
        Format::Yaml
    };
    let o = load_str(name, src, format, role, &env);
    let mut text = o.report.render_text(Style { color: false });
    text.push_str(&burrow::config::validate::summary_line(&o, false));
    for s in [SECRET, SECRET2] {
        assert!(!text.contains(s), "secret leaked in diagnostics:\n{text}");
    }
    let json = serde_json::to_string(&o.report.render_json()).unwrap();
    for s in [SECRET, SECRET2] {
        assert!(!json.contains(s), "secret leaked in json output");
    }
    text
}

fn server(extra_service: &str) -> String {
    format!(
        "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: {SECRET}\n  office:\n    secret: ${{HOME_SECRET}}\nservices:\n{extra_service}"
    )
}
