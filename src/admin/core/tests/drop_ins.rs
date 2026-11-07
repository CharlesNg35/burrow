//! Tests for a config set with drop-in files in server.d, each in a temporary folder.
//! `Set` builds one; `renames` covers renames mixed with moves and deletes.

#![cfg(test)]

mod renames;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::codes;
use burrow_admin_proto::features::CONFIG_FILES;
use burrow_admin_proto::msg::{AdminLevel, Apply, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Applied, FileChange, Plan};
use serde_json::json;

use super::{FakeLive, World, p, session, world};
use crate::admin::adapters::FileStore;
use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::*;

struct Set {
    dir: tempfile::TempDir,
    core: Core,
    w: World,
}

impl Set {
    fn new(drop_ins: &[(&str, &str)]) -> Set {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("server.yaml");
        crate::base::fs::write_private(&main, &w.text).unwrap();
        std::fs::create_dir(dir.path().join("server.d")).unwrap();
        for (name, text) in drop_ins {
            crate::base::fs::write_private(&dir.path().join("server.d").join(name), text).unwrap();
        }
        let live = FakeLive {
            text: Arc::new(Mutex::new(w.text.clone())),
            status: Mutex::new(Default::default()),
            sessions: Mutex::new(Vec::new()),
            busy: Vec::new(),
            allows: Default::default(),
        };
        let core = Core::new(
            Box::new(FileStore::new(&main)),
            Box::new(live),
            Box::new(|_: &str| None),
        );
        Set { dir, core, w }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)).ok()
    }

    fn session(&self, understands: bool) -> Session {
        let s = session(&self.w, AdminLevel::Write);
        let features = if understands {
            vec![CONFIG_FILES.to_string()]
        } else {
            Vec::new()
        };
        s.hello(None, features);
        s
    }

    fn base(&self) -> String {
        text::docs_rev(&self.core.docs().unwrap())
    }

    fn plan(&self, s: &Session, ops: &[Op]) -> Result<Plan, ApiError> {
        plan::compute(&self.core, s, &self.base(), ops).map(|c| c.plan)
    }

    fn apply(&self, s: &Session, ops: &[Op]) -> Result<Applied, ApiError> {
        let base = self.base();
        let plan = plan::compute(&self.core, s, &base, ops)?.plan;
        let begun = plan::apply_begin(
            &self.core,
            s,
            &Apply {
                base,
                ops: ops.to_vec(),
                confirm: plan.confirms().iter().map(|c| c.to_string()).collect(),
                plan: plan.plan,
                address: None,
            },
        )?;
        plan::finish(&self.core, s, begun, ReloadResult::unchanged(), None)
    }
}

const BLOG: &str = "# the blog, kept by hand\nservices:\n  blog:\n    client: home # the home box\n    bind: 7841\n    target: 3001\n";

#[test]
fn an_item_of_a_drop_in_is_edited_in_its_own_file_with_its_comments() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    set.apply(
        &s,
        &[Op::Set {
            path: p(&["services", "blog", "target"]),
            value: json!(3002),
        }],
    )
    .unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        BLOG.replace("3001", "3002")
    );
}

#[test]
fn an_item_added_to_a_file_of_its_own_creates_that_file_only() {
    let set = Set::new(&[]);
    let s = set.session(true);
    let ops = [
        Op::Add {
            path: p(&["services", "wiki"]),
            value: json!({"client": "home", "bind": 7842, "target": 3003}),
        },
        Op::Place {
            path: p(&["services", "wiki"]),
            file: "server.d/wiki.yaml".into(),
        },
    ];
    let plan = set.plan(&s, &ops).unwrap();
    let created: Vec<_> = plan
        .files
        .iter()
        .filter(|f| f.change == FileChange::Create)
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].file, "server.d/wiki.yaml");
    let placed: Vec<&str> = plan
        .changes
        .iter()
        .map(|c| c.summary.as_str())
        .filter(|c| c.contains("server.d/wiki.yaml"))
        .collect();
    assert_eq!(
        placed,
        ["adds service wiki in a new file, server.d/wiki.yaml"]
    );
    set.apply(&s, &ops).unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
    let wiki = set.read("server.d/wiki.yaml").unwrap();
    assert!(wiki.starts_with("services:\n  wiki:\n"), "{wiki}");
    let bad = [Op::Place {
        path: p(&["services", "web"]),
        file: "server.d/../evil.yaml".into(),
    }];
    let e = set.plan(&s, &bad).err().unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
}

#[test]
fn an_item_is_split_into_its_own_file_and_merged_back() {
    let set = Set::new(&[]);
    let s = set.session(true);
    set.apply(
        &s,
        &[Op::Place {
            path: p(&["services", "web"]),
            file: "server.d/web.yaml".into(),
        }],
    )
    .unwrap();
    assert!(!set.read("server.yaml").unwrap().contains("web:"));
    assert!(set.read("server.d/web.yaml").unwrap().contains("web:"));
    let back = [Op::Place {
        path: p(&["services", "web"]),
        file: "server.yaml".into(),
    }];
    let plan = set.plan(&s, &back).unwrap();
    let emptied = plan
        .files
        .iter()
        .find(|f| f.file == "server.d/web.yaml")
        .unwrap();
    assert_eq!(emptied.left_empty, Some(true));
    let mut ops = back.to_vec();
    ops.push(Op::DropFile {
        file: "server.d/web.yaml".into(),
    });
    set.apply(&s, &ops).unwrap();
    assert!(set.read("server.d/web.yaml").is_none());
    assert!(set.read("server.yaml").unwrap().contains("web:"));
}

#[test]
fn a_rename_updates_references_in_every_file() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    set.apply(
        &s,
        &[Op::Rename {
            path: p(&["clients", "home"]),
            to: "house".into(),
            refs: true,
        }],
    )
    .unwrap();
    assert!(set.read("server.yaml").unwrap().contains("  house:\n"));
    assert!(
        set.read("server.d/apps.yaml")
            .unwrap()
            .contains("client: house # the home box"),
        "{}",
        set.read("server.d/apps.yaml").unwrap()
    );
}

#[test]
fn removing_the_last_item_of_a_drop_in_offers_to_delete_it() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let remove = [Op::Remove {
        path: p(&["services", "blog"]),
    }];
    let plan = set.plan(&s, &remove).unwrap();
    let f = plan
        .files
        .iter()
        .find(|f| f.file == "server.d/apps.yaml")
        .unwrap();
    assert_eq!(
        (f.change.clone(), f.left_empty),
        (FileChange::Edit, Some(true))
    );
    let keep_one = [Op::DropFile {
        file: "server.d/apps.yaml".into(),
    }];
    assert!(
        set.plan(&s, &keep_one).is_err(),
        "a file with items is never deleted"
    );
    let mut ops = remove.to_vec();
    ops.extend(keep_one);
    set.apply(&s, &ops).unwrap();
    assert!(set.read("server.d/apps.yaml").is_none());
}

#[test]
fn a_drop_in_read_only_on_disk_is_shown_and_never_written() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let file = set.path("server.d/apps.yaml");
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&file, perms).unwrap();
    let s = set.session(true);
    let state = set.core.state(&s).unwrap();
    let apps = state
        .file
        .files
        .iter()
        .find(|f| f.name == "server.d/apps.yaml")
        .unwrap();
    assert!(apps.read_only.is_some());
    assert_eq!(apps.items, [p(&["services", "blog"])]);
    assert_eq!(
        state
            .file
            .file_of(&p(&["services", "blog", "target"]))
            .map(|f| f.name.as_str()),
        Some("server.d/apps.yaml")
    );
    for ops in [
        vec![Op::Set {
            path: p(&["services", "blog", "target"]),
            value: json!(3002),
        }],
        vec![Op::Remove {
            path: p(&["services", "blog"]),
        }],
        vec![Op::Place {
            path: p(&["services", "blog"]),
            file: "server.yaml".into(),
        }],
    ] {
        let e = set.plan(&s, &ops).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotWritable, "{}", e.message);
        assert_eq!(e.body["code"], codes::FILE_READ_ONLY);
        assert!(
            e.message.contains("read-only on disk: server.d/apps.yaml"),
            "{}",
            e.message
        );
    }
    set.apply(
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3005),
        }],
    )
    .unwrap();
    assert_eq!(set.read("server.d/apps.yaml").unwrap(), BLOG);
    #[cfg(windows)]
    {
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
    }
}

#[test]
fn a_drop_in_changed_on_disk_since_the_form_opened_is_named() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let base = set.base();
    set.core.state(&s).unwrap();
    std::fs::write(set.path("server.d/apps.yaml"), BLOG.replace("3001", "3009")).unwrap();
    let e = plan::compute(
        &set.core,
        &s,
        &base,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(1),
        }],
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(
        e.message.starts_with("server.d/apps.yaml changed on disk"),
        "{}",
        e.message
    );
}

#[test]
fn reverting_puts_the_whole_set_back() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let before = set.base();
    set.apply(
        &s,
        &[
            Op::Remove {
                path: p(&["services", "blog"]),
            },
            Op::DropFile {
                file: "server.d/apps.yaml".into(),
            },
            Op::Add {
                path: p(&["services", "wiki"]),
                value: json!({"client": "home", "bind": 7842, "target": 3003}),
            },
            Op::Place {
                path: p(&["services", "wiki"]),
                file: "server.d/wiki.yaml".into(),
            },
        ],
    )
    .unwrap();
    assert!(set.read("server.d/apps.yaml").is_none());
    set.apply(
        &s,
        &[Op::ReplaceDocument {
            from_history: before.clone(),
        }],
    )
    .unwrap();
    assert_eq!(set.read("server.d/apps.yaml").unwrap(), BLOG);
    assert!(set.read("server.d/wiki.yaml").is_none());
    assert_eq!(set.base(), before);
}

#[test]
fn an_older_admin_cannot_change_items_outside_the_main_file() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let old = set.session(false);
    let e = set
        .plan(
            &old,
            &[Op::Set {
                path: p(&["services", "blog", "target"]),
                value: json!(3002),
            }],
        )
        .err()
        .unwrap();
    assert_eq!(e.body["code"], codes::OLDER_ADMIN_FILES);
    set.apply(
        &old,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3005),
        }],
    )
    .unwrap();
    assert_eq!(set.read("server.d/apps.yaml").unwrap(), BLOG);
}

#[test]
fn the_lock_out_check_sees_an_admin_visitor_kept_in_a_drop_in() {
    let set = Set::new(&[]);
    let s = set.session(true);
    set.apply(
        &s,
        &[Op::Place {
            path: p(&["visitors", "ops"]),
            file: "server.d/people.yaml".into(),
        }],
    )
    .unwrap();
    assert!(set.read("server.d/people.yaml").unwrap().contains("ops:"));
    let e = set
        .plan(
            &s,
            &[Op::Remove {
                path: p(&["visitors", "ops"]),
            }],
        )
        .map(|p| p.refusals.len())
        .unwrap();
    assert!(e > 0, "removing the admin's own visitor is refused");
}

#[test]
fn a_problem_in_a_drop_in_names_its_file() {
    let set = Set::new(&[(
        "apps.yaml",
        "services:\n  blog: { client: nobody, target: 1, bind: 7841 }\n",
    )]);
    let s = set.session(true);
    let state = set.core.state(&s).unwrap();
    let d = state
        .running
        .file_diags
        .iter()
        .find(|d| d.code == "unknown-client")
        .unwrap();
    assert_eq!(d.file.as_deref(), Some("server.d/apps.yaml"));
    assert_eq!(d.path, Some(p(&["services", "blog", "client"])));
    assert_eq!(d.line, Some(2));
}

#[test]
fn a_history_version_naming_a_file_outside_the_folder_is_never_restored() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let outside = set.dir.path().join("escaped.yaml");
    for name in [
        "server.d/../escaped.yaml".to_string(),
        outside.display().to_string(),
    ] {
        let docs = crate::config::docs::Docs {
            list: vec![
                crate::config::docs::Doc {
                    name: "server.yaml".into(),
                    text: set.w.text.clone(),
                },
                crate::config::docs::Doc {
                    name,
                    text: "services: {}\n".into(),
                },
            ],
        };
        let rev = text::docs_rev(&docs);
        let history = set.path(".server.yaml.history");
        std::fs::create_dir_all(&history).unwrap();
        let hex = text::rev_hex(&rev).unwrap();
        std::fs::write(history.join(format!("1000-{hex}.yaml")), docs.bundle()).unwrap();
        assert!(
            set.core.store.version(&rev).is_none(),
            "a bad bundle is no version"
        );
        assert!(
            set.apply(&s, &[Op::ReplaceDocument { from_history: rev }])
                .is_err()
        );
        assert!(!outside.exists());
    }
}

#[test]
fn a_new_file_that_sorts_first_gives_the_rev_read_back_from_disk() {
    let set = Set::new(&[("zz.yaml", BLOG)]);
    let s = set.session(true);
    let applied = set
        .apply(
            &s,
            &[Op::Place {
                path: p(&["services", "web"]),
                file: "server.d/aa.yaml".into(),
            }],
        )
        .unwrap();
    assert_eq!(applied.rev, set.base());
}

#[cfg(unix)]
#[test]
fn review_shows_a_drop_in_anyone_can_change_and_nothing_is_written() {
    use std::os::unix::fs::PermissionsExt;
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let file = set.path("server.d/apps.yaml");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o602)).unwrap();
    let ops = [Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(3005),
    }];
    let plan = set.plan(&s, &ops).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!plan.valid);
    assert!(
        plan.diags.iter().any(|d| d.code == codes::DROP_IN_UNSAFE),
        "{:?}",
        plan.diags
    );
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o602)).unwrap();
    assert!(set.apply(&s, &ops).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
}

#[test]
fn names_that_differ_only_in_case_are_refused_with_a_clear_message() {
    let set = Set::new(&[("blog.yaml", BLOG)]);
    let s = set.session(true);
    for ops in [
        vec![Op::Place {
            path: p(&["services", "web"]),
            file: "server.d/Blog.yaml".into(),
        }],
        vec![
            Op::Add {
                path: p(&["services", "Wiki"]),
                value: json!({"client": "home", "bind": 7843, "target": 3003}),
            },
            Op::Place {
                path: p(&["services", "Wiki"]),
                file: "server.d/Wiki.yaml".into(),
            },
            Op::Place {
                path: p(&["services", "web"]),
                file: "server.d/wiki.yaml".into(),
            },
        ],
    ] {
        let e = set.plan(&s, &ops).err().unwrap();
        assert_eq!(e.code, ErrorCode::BadRequest);
        assert!(
            e.message.contains("differ only in upper and lower case"),
            "{}",
            e.message
        );
    }
}

#[test]
fn the_config_file_view_names_the_drop_in_each_item_comes_from() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let view =
        crate::admin::core::view::config_view(&set.core, burrow_admin_proto::view::Shown::Written)
            .unwrap();
    let body = view.body();
    let from = body.find("# from server.d/apps.yaml\n").expect(&body);
    let blog = body.find("  blog:\n").expect(&body);
    assert!(blog > from, "{body}");
    let main_services = body.find("services:\n").unwrap();
    assert!(
        main_services < from,
        "server.yaml's own services come first: {body}"
    );
    assert_eq!(body.matches("# from ").count(), 1, "{body}");
}

#[test]
fn an_empty_or_comment_only_drop_in_is_never_rewritten_by_an_unrelated_save() {
    let untouched = [
        ("empty.yaml", ""),
        ("placeholder.yaml", "# placeholder\n"),
        ("open.yaml", "services:\n"),
        (
            "mixed.yaml",
            "services:\n  wiki: { client: home, bind: 7850, target: 3009 }\nvisitors:\n",
        ),
    ];
    let set = Set::new(&untouched);
    let s = set.session(true);
    let edit = [Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(3005),
    }];
    let plan = set.plan(&s, &edit).unwrap();
    for f in plan.files.iter().filter(|f| f.file != "server.yaml") {
        assert_eq!(f.change, FileChange::Same, "{}", f.file);
    }
    set.apply(&s, &edit).unwrap();
    assert!(set.read("server.yaml").unwrap().contains("3005"));
    for (name, text) in untouched {
        assert_eq!(
            set.read(&format!("server.d/{name}")).unwrap(),
            text,
            "{name}"
        );
    }
    let older = set.session(false);
    let edit = [Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(3006),
    }];
    set.apply(&older, &edit).unwrap();
    assert!(set.read("server.yaml").unwrap().contains("3006"));
}

#[test]
fn a_new_file_named_like_an_entry_the_listing_skips_is_refused_at_plan_time() {
    let set = Set::new(&[]);
    std::fs::create_dir(set.path("server.d/blog.yaml")).unwrap();
    let s = set.session(true);
    let ops = [Op::Place {
        path: p(&["services", "web"]),
        file: "server.d/blog.yaml".into(),
    }];
    let e = set.plan(&s, &ops).err().unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest, "{}", e.message);
    assert!(
        e.message
            .contains("server.d/blog.yaml: a file or folder with this name already exists"),
        "{}",
        e.message
    );
    assert!(
        !e.message.contains("changed on the server"),
        "{}",
        e.message
    );
}

fn rename(from: &str, to: &str) -> Op {
    Op::Rename {
        path: p(&["services", from]),
        to: to.into(),
        refs: true,
    }
}

const WIKI: &str = "  wiki:\n    client: home # also home\n    bind: 7842\n    target: 3003\n";

#[test]
fn an_item_renamed_twice_in_one_change_stays_in_its_drop_in_with_its_comments() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    set.apply(&s, &[rename("blog", "journal"), rename("journal", "diary")])
        .unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        BLOG.replace("  blog:", "  diary:")
    );
    let there_and_back = [
        rename("diary", "journal"),
        Op::Set {
            path: p(&["services", "journal", "target"]),
            value: json!(3002),
        },
        rename("journal", "diary"),
    ];
    set.apply(&s, &there_and_back).unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        BLOG.replace("  blog:", "  diary:").replace("3001", "3002")
    );
}

#[test]
fn swapping_two_names_keeps_each_item_in_its_own_file() {
    let set = Set::new(&[("apps.yaml", &format!("{BLOG}{WIKI}"))]);
    let s = set.session(true);
    let swap = |a: &str, b: &str| [rename(a, "tmp"), rename(b, a), rename("tmp", b)];
    set.apply(&s, &swap("blog", "wiki")).unwrap();
    assert_eq!(set.read("server.yaml").unwrap(), set.w.text);
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        format!(
            "{}{}",
            BLOG.replace("  blog:", "  wiki:"),
            WIKI.replace("  wiki:", "  blog:")
        )
    );
    set.apply(&s, &swap("web", "wiki")).unwrap();
    assert!(
        set.read("server.yaml")
            .unwrap()
            .contains("  wiki: { client: home, bind: 7840, target: 3000 }\n"),
        "{}",
        set.read("server.yaml").unwrap()
    );
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        format!(
            "{}{}",
            BLOG.replace("  blog:", "  web:"),
            WIKI.replace("  wiki:", "  blog:")
        )
    );
}

const COPY: &str = "# an old copy\nservices:\n  blog: { client: home, bind: 7849, target: 3009 }\n  lab: { client: home, bind: 7848, target: 3008 }\n";

#[test]
fn a_drop_in_with_content_the_server_ignores_stays_as_it_is() {
    let listen = format!("listen: 9000\n{BLOG}");
    let set = Set::new(&[("apps.yaml", &listen), ("zz.yaml", COPY)]);
    let s = set.session(true);
    for edit in [
        Op::Set {
            path: p(&["listen"]),
            value: json!(7836),
        },
        Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3005),
        },
    ] {
        let plan = set.plan(&s, &[edit]).unwrap();
        for f in plan.files.iter().filter(|f| f.file != "server.yaml") {
            assert_eq!(f.change, FileChange::Same, "{}", f.file);
        }
    }
}

#[test]
fn a_change_to_a_file_with_ignored_content_is_refused_by_name() {
    let set = Set::new(&[("apps.yaml", BLOG), ("zz.yaml", COPY)]);
    let s = set.session(true);
    let e = set
        .plan(
            &s,
            &[Op::Set {
                path: p(&["services", "lab", "target"]),
                value: json!(3010),
            }],
        )
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert_eq!(
        e.message,
        "server.d/zz.yaml holds content the server ignores (services.blog is also in server.d/apps.yaml); fix that file by hand first"
    );
    let plan = set
        .plan(
            &s,
            &[Op::Set {
                path: p(&["services", "blog", "target"]),
                value: json!(3002),
            }],
        )
        .unwrap();
    let zz = plan.files.iter().find(|f| f.file == "server.d/zz.yaml");
    assert_eq!(zz.map(|f| f.change.clone()), Some(FileChange::Same));
}

#[test]
fn a_credential_only_one_drop_in_holds_never_shows_in_review() {
    let copy = "services:\n  web:\n    client: home\n    target: 3000\n    web:\n      hosts: [x.example.com]\n      request_headers:\n        Authorization: Basic c2VjcmV0LXZhbHVl\n";
    let set = Set::new(&[("zz.yaml", copy)]);
    let s = set.session(true);
    let plan = set
        .plan(
            &s,
            &[Op::DropFile {
                file: "server.d/zz.yaml".into(),
            }],
        )
        .unwrap();
    let zz = plan.files.iter().find(|f| f.file == "server.d/zz.yaml");
    assert_eq!(zz.map(|f| f.change.clone()), Some(FileChange::Remove));
    let shown = serde_json::to_string(&plan).unwrap();
    assert!(!shown.contains("c2VjcmV0LXZhbHVl"), "{shown}");
}

#[test]
fn the_config_file_view_puts_a_duplicate_where_the_server_reads_it() {
    let copy = "services:\n  web: { client: home, bind: 7849, target: 3009 }\n";
    let set = Set::new(&[("zz.yaml", copy)]);
    let view =
        crate::admin::core::view::config_view(&set.core, burrow_admin_proto::view::Shown::Written)
            .unwrap();
    let body = view.body();
    assert!(!body.contains("# from server.d/zz.yaml"), "{body}");
    assert!(body.contains("target: 3000"), "{body}");
}

#[test]
fn a_server_without_drop_ins_lists_its_main_file_with_its_items() {
    let set = Set::new(&[]);
    let state = set.core.state(&set.session(true)).unwrap();
    assert_eq!(state.file.files.len(), 1);
    let main = &state.file.files[0];
    assert_eq!(main.name, "server.yaml");
    assert!(main.items.contains(&p(&["services", "web"])), "{main:?}");
}
