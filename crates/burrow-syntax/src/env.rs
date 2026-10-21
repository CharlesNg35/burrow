#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Escape,
    Var { name: &'a str, valid: bool },
    Unclosed(&'a str),
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn pieces(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        if i > 0 {
            out.push(Piece::Text(&rest[..i]));
        }
        let tail = &rest[i..];
        if let Some(t) = tail.strip_prefix("$${") {
            out.push(Piece::Escape);
            rest = t;
        } else if let Some(t) = tail.strip_prefix("${") {
            let Some(end) = t.find('}') else {
                out.push(Piece::Unclosed(tail));
                return out;
            };
            let name = &t[..end];
            out.push(Piece::Var {
                name,
                valid: valid_name(name),
            });
            rest = &t[end + 1..];
        } else {
            out.push(Piece::Text(&tail[..1]));
            rest = &tail[1..];
        }
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

pub fn names(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in pieces(s) {
        if let Piece::Var { name, valid: true } = p
            && !out.iter().any(|n| n == name)
        {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tricky_inputs_follow_the_config_rules() {
        assert_eq!(names("${A}-${B}${A}"), ["A", "B"]);
        assert_eq!(names("$${X}"), Vec::<String>::new());
        assert_eq!(names("$$${X}"), Vec::<String>::new());
        assert_eq!(names("${X:-d}"), Vec::<String>::new());
        assert_eq!(names("${1X}"), Vec::<String>::new());
        assert_eq!(names("${_X1}"), ["_X1"]);
        assert_eq!(names("${A${B}}"), Vec::<String>::new());
        assert_eq!(names("${A"), Vec::<String>::new());
        assert_eq!(names("$5 and ${A}"), ["A"]);
        assert_eq!(
            pieces("a$${b}${C}$"),
            [
                Piece::Text("a"),
                Piece::Escape,
                Piece::Text("b}"),
                Piece::Var {
                    name: "C",
                    valid: true
                },
                Piece::Text("$")
            ]
        );
        assert_eq!(pieces("x${Y"), [Piece::Text("x"), Piece::Unclosed("${Y")]);
    }
}
