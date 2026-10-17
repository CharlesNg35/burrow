use std::io::Write;

pub trait Prompter {
    fn ask(&mut self, question: &str) -> Option<String>;
    fn interactive(&self) -> bool {
        false
    }
    fn confirm(&mut self, question: &str, _default: bool) -> bool {
        self.ask(question)
            .and_then(|a| decide(Some(&a), false))
            .unwrap_or(false)
    }
}

pub fn decide(line: Option<&str>, default: bool) -> Option<bool> {
    let Some(line) = line else {
        return Some(false);
    };
    match line.trim().to_ascii_lowercase().as_str() {
        "" => Some(default),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

pub struct NoPrompt;

impl Prompter for NoPrompt {
    fn ask(&mut self, _: &str) -> Option<String> {
        None
    }
}

pub struct TtyPrompt;

impl TtyPrompt {
    fn read(question: &str) -> Option<String> {
        eprint!("{question} ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }
}

impl Prompter for TtyPrompt {
    fn interactive(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    }

    fn ask(&mut self, question: &str) -> Option<String> {
        if !self.interactive() {
            return None;
        }
        let line = Self::read(&format!("{question}:"))?;
        let line = line.trim().to_string();
        (!line.is_empty()).then_some(line)
    }

    fn confirm(&mut self, question: &str, default: bool) -> bool {
        if !self.interactive() {
            return false;
        }
        let shown = if default { "[Y/n]" } else { "[y/N]" };
        for _ in 0..3 {
            let line = Self::read(&format!("{question} {shown}"));
            if let Some(answer) = decide(line.as_deref(), default) {
                return answer;
            }
            eprintln!("type y or n");
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_input_never_counts_as_yes() {
        assert_eq!(decide(None, true), Some(false));
        assert_eq!(decide(Some("\n"), true), Some(true));
        assert_eq!(decide(Some("\n"), false), Some(false));
        assert_eq!(decide(Some(" Yes\n"), false), Some(true));
        assert_eq!(decide(Some("n"), true), Some(false));
        assert_eq!(decide(Some("maybe"), true), None);
        assert!(!NoPrompt.confirm("restart?", true));
    }
}
