mod access;
mod apply;
mod draft;
mod editor;
mod found;
mod notes;
mod pick;
mod save;
mod source;

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::Format;
use crate::config::env::{EnvSource, ProcessEnv};
use crate::diagnostics::{Severity, Style};
use crate::failure::{Failure, admin, cmd, file_reason};
use crate::prompt::Prompter;
use draft::Draft;
use found::{Found, Origin};

pub struct Options {
    pub target: Option<String>,
    pub editor: Option<String>,
}

pub fn run(opts: &Options, prompt: &mut dyn Prompter, out: &mut dyn Write) -> Result<u8, Failure> {
    let found = found::all();
    let chosen = choose(&found, opts.target.as_deref(), prompt)?;
    let editor = editor::choose(
        opts.editor.as_deref(),
        |k| std::env::var(k).ok(),
        editor::on_path,
    )?;
    edit(
        &chosen,
        &mut |file| editor::open(&editor, file),
        prompt,
        out,
    )
}

fn choose(
    found: &[Found],
    target: Option<&str>,
    prompt: &mut dyn Prompter,
) -> Result<Found, Failure> {
    match (target, found) {
        (Some(t), _) => pick::by_target(found, t),
        (None, []) => {
            let hint = if crate::service::services_hidden() {
                format!(
                    "the installed services are only visible to an administrator: {}",
                    admin("edit")
                )
            } else {
                format!(
                    "pass a file: {}, or create one with: {}",
                    cmd("edit FILE"),
                    cmd("init")
                )
            };
            Err(Failure::usage(
                "no burrow config was found here, in the installed services or in running instances",
            )
            .hint(hint))
        }
        (None, [one]) => Ok(one.clone()),
        (None, _) => pick::ask(found, prompt),
    }
}

fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    writeln!(out, "{}", line.as_ref()).map_err(|e| Failure::new(format!("cannot write: {e}")))
}

fn service_env(f: &Found) -> Option<BTreeMap<String, String>> {
    let Origin::Service { name, .. } = &f.origin else {
        return None;
    };
    crate::service::resolve(Some(name))
        .ok()
        .map(|m| crate::service::env_of(&m))
}

fn read(path: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|e| {
        let failure = Failure::new(format!(
            "cannot read {}: {}",
            path.display(),
            file_reason(&e)
        ));
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            failure.hint(format!(
                "open it as an administrator: {}",
                admin(&format!("edit {}", path.display()))
            ))
        } else {
            failure
        }
    })
}

fn only_missing_variables(checked: &crate::config::Outcome) -> bool {
    checked
        .report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .all(|d| d.code == "env-unset")
}

fn edit(
    f: &Found,
    open: &mut dyn FnMut(&Path) -> Result<(), Failure>,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<u8, Failure> {
    let path = &f.path;
    let shown = path.display();
    let format = Format::from_path(path).ok_or_else(|| {
        Failure::usage(format!("cannot tell the format of {shown}"))
            .hint("burrow configs end in .yaml, .yml or .json")
    })?;
    let before = read(path)?;
    if !access::can_write(path) {
        return Err(
            Failure::new(format!("you cannot change {shown}")).hint(format!(
                "open it as an administrator: {}",
                admin(&format!("edit {shown}"))
            )),
        );
    }
    if let Some(owner) = access::other_owner(path) {
        say(
            out,
            format!("note: {shown} belongs to {owner}; it keeps that owner and its permissions"),
        )?;
    }
    let complete_env = matches!(f.origin, Origin::Service { .. });
    let env: Box<dyn EnvSource> = match service_env(f) {
        Some(vars) => Box::new(move |k: &str| vars.get(k).cloned()),
        None => Box::new(ProcessEnv),
    };
    let name = path.file_name().unwrap_or_default();
    say(out, format!("editing {shown} ({})", pick::place(f)))?;
    let style = Style::detect(std::io::stderr().is_terminal());
    let mut current = before.clone();
    let mut opened = before.clone();
    let text = loop {
        let started = Instant::now();
        let text = {
            let draft = Draft::new(name, &opened)
                .map_err(|e| Failure::new(format!("cannot make a private copy to edit: {e}")))?;
            open(draft.path())?;
            notes::strip(
                &draft
                    .read()
                    .map_err(|e| Failure::new(format!("cannot read the edited copy: {e}")))?,
            )
        };
        if text == before {
            say(out, format!("no changes; {shown} is unchanged"))?;
            if started.elapsed() < Duration::from_secs(1) {
                say(
                    out,
                    "  if your editor returned at once, give it its wait flag, for example: --editor \"code --wait\"",
                )?;
            }
            return Ok(0);
        }
        if opened != before && text == current {
            say(out, format!("edit cancelled; {shown} is unchanged"))?;
            return Ok(1);
        }
        let checked = save::check(path, format, f.role, &text, &*env);
        if checked.report.error_count() + checked.report.warning_count() > 0 {
            eprint!("{}", checked.report.render_text(style));
        }
        if !checked.report.has_errors() {
            break text;
        }
        if !prompt.interactive() {
            say(out, format!("not saved; {shown} is unchanged"))?;
            return Ok(1);
        }
        if !complete_env && only_missing_variables(&checked) {
            say(
                out,
                "these variables are not set in this terminal, so the check is incomplete; whatever runs this config needs them",
            )?;
            if prompt.confirm("save anyway?", false) {
                break text;
            }
        }
        opened = match format {
            Format::Yaml => {
                say(
                    out,
                    "reopening the editor with the problems listed at the top; close it without saving to cancel",
                )?;
                notes::annotate(&text, &checked.report.diagnostics)
            }
            Format::Json => {
                if !prompt.confirm("reopen the editor to fix it?", true) {
                    say(out, format!("not saved; {shown} is unchanged"))?;
                    return Ok(1);
                }
                text.clone()
            }
        };
        current = text;
    };
    let keep = |text: &str| {
        Draft::new(name, text)
            .map(|d| d.keep().display().to_string())
            .unwrap_or_else(|e| format!("nowhere ({e})"))
    };
    match save::write_back(path, &before, &text) {
        Ok(()) => say(out, format!("saved {shown}"))?,
        Err(save::Refused::Changed) => {
            return Err(Failure::new(format!(
                "{shown} changed while you were editing it; nothing was written"
            ))
            .hint(format!(
                "your version is in {}; compare and copy what you need",
                keep(&text)
            )));
        }
        Err(save::Refused::Io(e)) => {
            return Err(
                Failure::new(format!("cannot write {shown}: {}", file_reason(&e)))
                    .hint(format!("your version is in {}", keep(&text))),
            );
        }
    }
    match &f.origin {
        Origin::Service { name, .. } => match source::of_service(name, path, &before, &text) {
            source::Source::Synced(src) => say(
                out,
                format!(
                    "also saved {}, which {name} was installed from",
                    src.display()
                ),
            )?,
            source::Source::Differs(src) => say(
                out,
                format!(
                    "note: {name} was installed from {}, which is different; installing from it again would replace this edit",
                    src.display()
                ),
            )?,
            source::Source::Alone => {}
        },
        Origin::Folder => {
            for service in source::installed_from(path) {
                say(
                    out,
                    format!(
                        "note: {service} runs its own copy of this file; to use the change: {}",
                        admin(&format!("service install -c {shown}"))
                    ),
                )?;
            }
        }
        Origin::Running { .. } => {}
    }
    apply::apply(apply::after(f), prompt, out)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Role;
    use std::path::PathBuf;

    struct Answers {
        given: Vec<&'static str>,
        asked: Vec<String>,
    }

    impl Answers {
        fn new(given: &[&'static str]) -> Answers {
            Answers {
                given: given.to_vec(),
                asked: Vec::new(),
            }
        }
    }

    impl Prompter for Answers {
        fn interactive(&self) -> bool {
            true
        }
        fn ask(&mut self, question: &str) -> Option<String> {
            self.asked.push(question.to_string());
            (!self.given.is_empty()).then(|| self.given.remove(0).to_string())
        }
    }

    fn good() -> String {
        format!(
            "server: example.com\nname: home\nsecret: {}\nallow: [127.0.0.0/8]\n",
            crate::secret::Secret::generate().encode()
        )
    }

    fn config(dir: &Path, name: &str, text: &str) -> Found {
        let path = dir.join(name);
        crate::instance::write_private(&path, text).unwrap();
        Found {
            path: std::fs::canonicalize(&path).unwrap(),
            role: Role::Client,
            origin: Origin::Folder,
        }
    }

    fn editor(versions: Vec<Option<String>>) -> impl FnMut(&Path) -> Result<(), Failure> {
        let mut versions = versions.into_iter();
        move |file: &Path| {
            if let Some(Some(text)) = versions.next() {
                std::fs::write(file, text).unwrap();
            }
            Ok(())
        }
    }

    fn run(f: &Found, versions: Vec<Option<String>>, prompt: &mut dyn Prompter) -> (u8, String) {
        let mut out = Vec::new();
        let code = edit(f, &mut editor(versions), prompt, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_valid_edit_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let next = good.replace("name: home", "name: office");
        let (code, out) = run(&f, vec![Some(next.clone())], &mut crate::prompt::NoPrompt);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("saved"), "{out}");
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), next);
    }

    #[test]
    fn a_wrong_edit_reopens_with_the_problems_on_top_until_it_is_right() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let wrong = good.replace("name: home", "nmae: home");
        let right = good.replace("name: home", "name: office");
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = seen.clone();
        let mut step = 0;
        let mut open = |file: &Path| {
            log.borrow_mut()
                .push(std::fs::read_to_string(file).unwrap());
            step += 1;
            let text = if step == 1 { &wrong } else { &right };
            std::fs::write(file, text).unwrap();
            Ok(())
        };
        let mut out = Vec::new();
        let code = edit(&f, &mut open, &mut Answers::new(&[]), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{out}");
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], good);
        assert!(
            seen[1].starts_with("# burrow edit: not saved"),
            "{}",
            seen[1]
        );
        assert!(seen[1].contains("nmae"), "{}", seen[1]);
        assert!(seen[1].ends_with(&wrong));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), right);
    }

    #[test]
    fn closing_the_reopened_editor_without_saving_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let wrong = good.replace("name: home", "nmae: home");
        let (code, out) = run(&f, vec![Some(wrong), None], &mut Answers::new(&[]));
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("edit cancelled"), "{out}");
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), good);
    }

    #[test]
    fn without_a_terminal_a_wrong_edit_is_dropped_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let wrong = good.replace("name: home", "nmae: home");
        let mut calls = 0;
        let mut open = |file: &Path| {
            calls += 1;
            std::fs::write(file, &wrong).unwrap();
            Ok(())
        };
        let mut out = Vec::new();
        let code = edit(&f, &mut open, &mut crate::prompt::NoPrompt, &mut out).unwrap();
        assert_eq!((code, calls), (1, 1));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), good);
    }

    #[test]
    fn a_wrong_json_edit_asks_before_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let good = format!(
            r#"{{"server": "example.com", "name": "home", "secret": "{}", "allow": ["127.0.0.0/8"]}}"#,
            crate::secret::Secret::generate().encode()
        );
        let f = config(dir.path(), "client.json", &good);
        let wrong = good.replace("\"name\"", "\"nmae\"");
        let mut answers = Answers::new(&["n"]);
        let (code, out) = run(&f, vec![Some(wrong)], &mut answers);
        assert_eq!(code, 1, "{out}");
        assert!(
            answers.asked[0].contains("reopen the editor"),
            "{:?}",
            answers.asked
        );
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), good);
    }

    #[test]
    fn missing_variables_can_be_saved_only_outside_a_service() {
        let dir = tempfile::tempdir().unwrap();
        let text = "server: example.com\nname: home\nsecret: ${BURROW_EDIT_UNIT_UNSET}\nallow: [127.0.0.0/8]\n".to_string();
        let next = text.replace("name: home", "name: office");
        let f = config(dir.path(), "client.yaml", &text);
        let mut answers = Answers::new(&["y"]);
        let (code, out) = run(&f, vec![Some(next.clone())], &mut answers);
        assert_eq!(code, 0, "{out}");
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), next);

        let mut service = config(dir.path(), "other.yaml", &text);
        service.origin = Origin::Service {
            name: "burrow-unit-test-none".into(),
            running: false,
        };
        let mut answers = Answers::new(&[]);
        let (code, out) = run(&service, vec![Some(next), None], &mut answers);
        assert_eq!(code, 1, "{out}");
        assert!(
            answers.asked.iter().all(|q| !q.contains("save anyway")),
            "{:?}",
            answers.asked
        );
        assert_eq!(std::fs::read_to_string(&service.path).unwrap(), text);
    }

    #[test]
    fn an_unchanged_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let (code, out) = run(&f, vec![None], &mut crate::prompt::NoPrompt);
        assert_eq!(code, 0);
        assert!(out.contains("no changes"), "{out}");
    }

    #[test]
    fn a_change_made_meanwhile_is_kept_and_yours_is_saved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let good = good();
        let f = config(dir.path(), "client.yaml", &good);
        let theirs = good.replace("name: home", "name: theirs");
        let mine = good.replace("name: home", "name: mine");
        let original = f.path.clone();
        let mut open = |file: &Path| {
            std::fs::write(file, &mine).unwrap();
            std::fs::write(&original, &theirs).unwrap();
            Ok(())
        };
        let err = edit(&f, &mut open, &mut crate::prompt::NoPrompt, &mut Vec::new()).unwrap_err();
        assert!(err.message.contains("changed while you were editing"));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), theirs);
        let hint = err.hint.unwrap();
        let kept = PathBuf::from(
            hint.trim_start_matches("your version is in ")
                .split(';')
                .next()
                .unwrap(),
        );
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), mine);
        std::fs::remove_dir_all(kept.parent().unwrap()).unwrap();
    }
}
