mod actions;
mod body;
mod detail;
mod list;
mod problems;
mod rows;
mod summary;

use crate::screens::plural;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::as_secret;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Tone, tag_class};

use detail::Detail;
pub(crate) use list::{Entities, rows_for};
pub(crate) use problems::go_to_field;
use problems::{apply_notes, problem_list, problems_for, problems_of};
pub(crate) use summary::health_tag;
use summary::{overview, served, visitor_overview};

const SERVICES: &str = ServiceEntry::SECTION;
const CLIENTS: &str = ClientEntry::SECTION;
const VISITORS: &str = VisitorEntry::SECTION;
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn secrets(v: Option<&Value>) -> usize {
    match v {
        Some(Value::Array(a)) => a.len(),
        Some(x) if as_secret(x).is_some() || x.is_object() => 1,
        _ => 0,
    }
}

fn wrapping_tag(tone: Tone) -> String {
    tag_class(tone)
        .replace("h-[18px]", "min-h-[18px] max-w-full py-px leading-tight")
        .replace("whitespace-nowrap", "whitespace-normal")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_tag_wraps_instead_of_running_under_the_buttons() {
        let class = wrapping_tag(Tone::Warning);
        assert!(class.contains("whitespace-normal"), "{class}");
        assert!(class.contains("max-w-full"), "{class}");
        assert!(!class.contains("whitespace-nowrap"), "{class}");
        assert!(!class.contains(" h-[18px]"), "{class}");
    }
}
