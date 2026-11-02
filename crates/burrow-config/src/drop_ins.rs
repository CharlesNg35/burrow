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

pub fn new_name_ok(folder: &str, name: &str) -> bool {
    let Some(file) = file_in(folder, name) else {
        return false;
    };
    let stem = file.strip_suffix(".yaml").unwrap_or("");
    let device = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ((device.starts_with("COM") || device.starts_with("LPT"))
            && device.len() == 4
            && device.ends_with(|c: char| c.is_ascii_digit()));
    !stem.is_empty()
        && stem.len() <= 100
        && !reserved
        && !stem.contains("..")
        && !stem.starts_with(['.', '-'])
        && !stem.ends_with('.')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_in_name_is_one_plain_file_in_the_folder() {
        assert_eq!(folder_name("server.yaml"), "server.d");
        assert_eq!(folder_name("home.json"), "home.d");
        assert_eq!(
            file_in("server.d", "server.d/my apps.yaml"),
            Some("my apps.yaml")
        );
        for bad in [
            "server.d/../x.yaml",
            "server.d/a/b.yaml",
            "server.d/a\\b.yaml",
            "/etc/cron.d/x.yaml",
            "server.d/..",
            "server.d/.hidden.yaml",
            "server.d/x.yml",
            "server.d/c:x.yaml",
            "other.d/x.yaml",
            "server.d",
            "server.d/a\nb.yaml",
            "server.d/a\tb.yaml",
            "server.d/a\u{7f}b.yaml",
        ] {
            assert_eq!(file_in("server.d", bad), None, "{bad}");
        }
        assert!(new_name_ok("server.d", "server.d/blog.yaml"));
        assert!(!new_name_ok("server.d", "server.d/my apps.yaml"));
        assert!(!new_name_ok("server.d", "server.d/com1.yaml"));
    }
}
