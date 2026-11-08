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
