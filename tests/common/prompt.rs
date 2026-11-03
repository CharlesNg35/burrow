use burrow::base::prompt::Prompter;

#[derive(Default)]
pub struct Terminal {
    pub yes: Vec<bool>,
    pub asked: Vec<String>,
}

impl Terminal {
    pub fn answering(yes: &[bool]) -> Terminal {
        Terminal {
            yes: yes.to_vec(),
            asked: Vec::new(),
        }
    }
}

impl Prompter for Terminal {
    fn ask(&mut self, question: &str) -> Option<String> {
        self.asked.push(question.to_string());
        None
    }

    fn interactive(&self) -> bool {
        true
    }

    fn confirm(&mut self, question: &str, _default: bool) -> bool {
        self.asked.push(question.to_string());
        (!self.yes.is_empty()) && self.yes.remove(0)
    }
}
