#![cfg(test)]

use burrow_admin_proto::codes;
use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::text_edit::TextFile;

use super::super::text_edit::{edit_text, replace};
use super::{BLOG, Set};
use crate::wire::secret::Secret;

const SPARE: &str = "server.d/spare.yaml";

fn spare(s: &Secret) -> String {
    format!(
        "# kept apart\nclients:\n  spare:\n    secret: {} # rotate yearly\n",
        s.encode()
    )
}

#[test]
fn every_file_is_a_tab_and_an_edit_lands_in_its_own_file() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let t = edit_text(&set.core, &s);
    let names: Vec<&str> = t.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["server.yaml", "server.d/apps.yaml"]);
    assert_eq!(t.files[1].text, BLOG);
    let ops = vec![Op::ReplaceText {
        files: vec![TextFile {
            name: "server.d/apps.yaml".into(),
            text: BLOG.replace("3001", "3002"),
        }],
    }];
    set.apply(&s, &ops).unwrap();
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        BLOG.replace("3001", "3002")
    );
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
}

#[test]
fn an_item_moved_to_another_file_keeps_its_secret() {
    let secret = Secret::generate();
    let set = Set::new(&[("spare.yaml", &spare(&secret))]);
    let s = set.session(true);
    let t = edit_text(&set.core, &s);
    let shown = t.files[1].text.clone();
    assert!(!shown.contains(&secret.encode()), "{shown}");
    let block = shown.split_once("clients:\n").unwrap().1.to_string();
    let ops = replace(&t, |x| {
        if x == shown {
            "# kept apart\n".to_string()
        } else {
            x.replace("clients:\n", &format!("clients:\n{block}"))
        }
    });
    set.apply(&s, &ops).unwrap();
    let main = set.read("server.yaml").unwrap();
    assert!(
        main.contains(&format!(
            "  spare:\n    secret: {} # rotate yearly\n",
            secret.encode()
        )),
        "{main}"
    );
    assert_eq!(set.read(SPARE).unwrap(), "# kept apart\n");
}

#[test]
fn a_read_only_file_is_marked_and_a_change_to_it_is_refused() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let file = set.path("server.d/apps.yaml");
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&file, perms).unwrap();
    let s = set.session(true);
    let t = edit_text(&set.core, &s);
    assert!(t.files[1].read_only.is_some());
    let e = set
        .plan(&s, &replace(&t, |x| x.replace("3001", "3002")))
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::NotWritable, "{}", e.message);
    assert_eq!(e.body["code"], codes::FILE_READ_ONLY);
    let main_only = replace(&t, |x| x.replace("target: 3000", "target: 3005"));
    assert!(set.plan(&s, &main_only).is_ok());
    #[cfg(windows)]
    {
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
    }
}

fn new_file(name: &str, text: &str) -> Vec<Op> {
    vec![Op::ReplaceText {
        files: vec![TextFile {
            name: name.into(),
            text: text.into(),
        }],
    }]
}

const NEW: &str = "server.d/new.yaml";
const SHOP: &str =
    "# the shop\nservices:\n  shop:\n    client: home\n    bind: 7851\n    target: 3009\n";

#[test]
fn a_new_drop_in_is_made_through_review_and_a_revert_takes_it_away() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let base = set.base();
    let plan = set.plan(&s, &new_file(NEW, SHOP)).unwrap();
    assert!(plan.valid, "{:#?}", plan.diags);
    let lines: Vec<&str> = plan.changes.iter().map(|c| c.summary.as_str()).collect();
    assert!(
        lines.contains(&"creates a new file, server.d/new.yaml"),
        "{lines:?}"
    );
    let made = plan
        .files
        .iter()
        .find(|f| f.file == NEW)
        .expect("a diff for the new file");
    assert!(
        matches!(made.change, burrow_admin_proto::plan::FileChange::Create),
        "{made:?}"
    );
    set.apply(&s, &new_file(NEW, SHOP)).unwrap();
    assert_eq!(set.read(NEW).unwrap(), SHOP);
    assert_eq!(set.read("server.d/apps.yaml").unwrap(), BLOG);
    set.apply(&s, &[Op::ReplaceDocument { from_history: base }])
        .unwrap();
    assert!(set.read(NEW).is_none(), "the revert removes the new file");
}

#[test]
fn a_new_file_name_is_checked_as_a_placed_item_s_is() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    for (name, says) in [
        ("server.d/../x.yaml", "is not a file name burrow can use"),
        ("server.d/notes.txt", "is not a file name burrow can use"),
        ("other/x.yaml", "is not a file name burrow can use"),
        ("server.d/APPS.yaml", "differ only in upper and lower case"),
    ] {
        let e = set.plan(&s, &new_file(name, SHOP)).err().unwrap();
        assert_eq!(e.code, ErrorCode::BadRequest, "{name}: {}", e.message);
        assert!(e.message.contains(says), "{name}: {}", e.message);
    }
    std::fs::create_dir(set.path("server.d/taken.yaml")).unwrap();
    let e = set
        .plan(&s, &new_file("server.d/taken.yaml", SHOP))
        .err()
        .unwrap();
    assert!(e.message.contains("already exists"), "{}", e.message);
}

#[test]
fn a_server_wide_key_in_a_new_drop_in_is_refused_as_in_any_drop_in() {
    let set = Set::new(&[]);
    let s = set.session(true);
    let plan = set.plan(&s, &new_file(NEW, "listen: 7999\n")).unwrap();
    assert!(!plan.valid);
    let d = plan
        .diags
        .iter()
        .find(|d| d.code == codes::DROP_IN_KEY)
        .unwrap_or_else(|| panic!("{:#?}", plan.diags));
    assert_eq!(d.file.as_deref(), Some(NEW));
}

#[test]
fn an_empty_new_file_is_not_made() {
    let set = Set::new(&[]);
    let s = set.session(true);
    let plan = set.plan(&s, &new_file(NEW, "  \n")).unwrap();
    assert!(
        plan.changes.is_empty() && plan.files.iter().all(|f| f.file != NEW),
        "{plan:#?}"
    );
}

#[test]
fn an_item_moved_into_a_new_drop_in_keeps_its_secret() {
    let set = Set::new(&[]);
    let s = set.session(true);
    let t = edit_text(&set.core, &s);
    let main = &t.files[0].text;
    let start = main.find("  # the home box\n").unwrap();
    let end = main.find("\nvisitors:").unwrap();
    let block = &main[start..end];
    let ops = vec![Op::ReplaceText {
        files: vec![
            TextFile {
                name: "server.yaml".into(),
                text: format!("{}{}", &main[..start], &main[end..])
                    .replace("clients:\n\n", "clients: {}\n\n"),
            },
            TextFile {
                name: NEW.into(),
                text: format!("clients:\n{block}"),
            },
        ],
    }];
    set.apply(&s, &ops).unwrap();
    let made = set.read(NEW).unwrap();
    assert!(made.contains(&set.w.home.encode()), "{made}");
}
