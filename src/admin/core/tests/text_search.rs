#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, req};
use burrow_admin_proto::text_search::{SearchReply, SearchText};
use serde_json::json;

use super::{core_for, session, world};
use crate::admin::core::{Core, Session, dispatch, text};

pub(super) fn search(core: &Core, s: &Session, query: &str, limit: Option<u32>) -> SearchReply {
    let body = json!(SearchText {
        query: query.into(),
        limit,
    });
    serde_json::from_value(dispatch(core, s, req::SEARCH_TEXT, &body).unwrap()).unwrap()
}

#[test]
fn any_admin_searches_the_text_as_shown_with_its_lines_and_columns() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Read);
    let r = search(&core, &s, "TARGET", None);
    assert_eq!(r.rev, text::docs_rev(&core.docs().unwrap()));
    assert_eq!(r.matches.len(), 1, "{r:?}");
    let m = &r.matches[0];
    assert_eq!((m.file.as_str(), m.line), ("server.yaml", 15));
    let line = w.text.lines().nth(14).unwrap();
    assert_eq!(m.col as usize, line.find("target").unwrap() + 1);
    assert_eq!(m.end_col, m.col + 6);
    assert_eq!(m.excerpt, line.trim());
    assert!(!r.more && r.unsearchable.is_empty());
    assert!(search(&core, &s, " ", None).matches.is_empty());
    let few = search(&core, &s, "e", Some(2));
    assert_eq!((few.matches.len(), few.more), (2, true));
}

#[test]
fn a_secret_is_never_found_but_its_placeholder_is() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Read);
    for secret in [w.home.encode(), w.ops.encode()] {
        for query in [&secret[..], &secret[..8], &secret[10..18]] {
            let r = search(&core, &s, query, None);
            assert!(r.matches.is_empty(), "{query} found {r:?}");
        }
    }
    let shown = format!("«secret {}»", w.home.fingerprint());
    let r = search(&core, &s, &shown, None);
    assert_eq!(r.matches.len(), 1, "{r:?}");
    assert!(r.matches[0].excerpt.contains(&shown));
    assert!(
        !serde_json::to_string(&r)
            .unwrap()
            .contains(&w.home.encode())
    );
}

#[test]
fn a_weak_secret_is_never_found_either() {
    let w = world();
    let text = w.text.replace(&w.home.encode(), "hunter2,PARTSECRET");
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Read);
    for query in ["hunter2", "PARTSECRET", "ter2,PART", "hunter2,PARTSECRET"] {
        assert!(search(&core, &s, query, None).matches.is_empty(), "{query}");
    }
}
