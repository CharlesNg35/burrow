//! Drop-in config files: the sections they may hold, the folder's name, and which file names burrow reads and may make.
//! `name_problem` is the one rule for a new file's name, shared by the server and the admin UI.

use crate::{ClientEntry, Entry, ServiceEntry, VisitorEntry};

pub fn sections() -> [(&'static str, &'static str); 3] {
    [
        (ServiceEntry::section(), ServiceEntry::ENTITY),
        (ClientEntry::section(), ClientEntry::ENTITY),
        (VisitorEntry::section(), VisitorEntry::ENTITY),
    ]
}

pub fn is_section(key: &str) -> bool {
    sections().iter().any(|(s, _)| *s == key)
}

pub fn folder_name(main_file: &str) -> String {
    let stem = main_file
        .rsplit_once('.')
        .map_or(main_file, |(s, _)| s)
        .trim();
    let stem = if stem.is_empty() { "server" } else { stem };
    format!("{stem}.d")
}

pub fn file_in<'a>(folder: &str, name: &'a str) -> Option<&'a str> {
    let file = name.strip_prefix(folder)?.strip_prefix('/')?;
    let plain = !file.is_empty()
        && file != "."
        && file != ".."
        && !file.starts_with('.')
        && !file.contains(['/', '\\', ':'])
        && !file.chars().any(char::is_control)
        && file.ends_with(".yaml");
    plain.then_some(file)
}

pub fn is_device_name(stem: &str) -> bool {
    let device = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ((device.starts_with("COM") || device.starts_with("LPT"))
            && device.len() == 4
            && device.ends_with(|c: char| c.is_ascii_digit()))
}

pub const MAX_STEM: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    NotInFolder,
    Characters,
    TooLong,
    Reserved,
}

impl NameProblem {
    pub fn message(self, folder: &str, name: &str) -> String {
        match self {
            NameProblem::NotInFolder => {
                format!("\"{name}\" is not a drop-in file name; use {folder}/<name>.yaml")
            }
            NameProblem::Characters => format!(
                "\"{name}\": use only letters, digits, dots, dashes and underscores, not starting with a dot or dash, not ending with a dot and without \"..\", so the name works on Linux, macOS and Windows"
            ),
            NameProblem::TooLong => {
                format!("\"{name}\": a name may have at most {MAX_STEM} characters before .yaml")
            }
            NameProblem::Reserved => format!(
                "\"{name}\": {} is a device name on Windows and cannot be a file name there; pick another name",
                name.rsplit('/')
                    .next()
                    .unwrap_or(name)
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase()
            ),
        }
    }
}

pub fn name_problem(folder: &str, name: &str) -> Option<NameProblem> {
    let Some(stem) = name
        .strip_prefix(folder)
        .and_then(|r| r.strip_prefix('/'))
        .and_then(|f| f.strip_suffix(".yaml"))
    else {
        return Some(NameProblem::NotInFolder);
    };
    let plain = !stem.is_empty()
        && !stem.contains("..")
        && !stem.starts_with(['.', '-'])
        && !stem.ends_with('.')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !plain {
        return Some(NameProblem::Characters);
    }
    if stem.len() > MAX_STEM {
        return Some(NameProblem::TooLong);
    }
    is_device_name(stem).then_some(NameProblem::Reserved)
}

pub fn new_name_ok(folder: &str, name: &str) -> bool {
    name_problem(folder, name).is_none()
}

pub fn exists_message(file: &str) -> String {
    format!("{file} already exists; pick another file name")
}

pub fn case_clash_message(file: &str, other: &str) -> String {
    format!(
        "{file} and {other} differ only in upper and lower case, which macOS and Windows treat as one file; pick another file name"
    )
}

pub fn new_name_problem(existing: &[&str], folder: &str, file: &str) -> Option<String> {
    if let Some(other) = existing.iter().find(|n| n.eq_ignore_ascii_case(file)) {
        return Some(if *other == file {
            exists_message(file)
        } else {
            case_clash_message(file, other)
        });
    }
    name_problem(folder, file).map(|p| p.message(folder, file))
}

#[cfg(test)]
mod tests;
