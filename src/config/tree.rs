use saphyr_parser::{Event, Parser, ScalarStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub offset: usize,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn point(pos: Pos) -> Self {
        Span {
            start: pos,
            end: pos,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Node>),
    Map(Vec<Entry>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub key_span: Span,
    pub value: Node,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub value: Value,
    pub span: Span,
    pub quoted: bool,
}

impl Node {
    pub fn kind(&self) -> &'static str {
        match self.value {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Int(_) => "an integer",
            Value::Float(_) => "a number",
            Value::Str(_) => "a string",
            Value::Seq(_) => "a list",
            Value::Map(_) => "a mapping",
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entry(key).map(|e| &e.value)
    }

    pub fn entry(&self, key: &str) -> Option<&Entry> {
        match &self.value {
            Value::Map(entries) => entries.iter().find(|e| e.key == key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match &self.value {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
            Value::Str(s) => J::String(s.clone()),
            Value::Seq(items) => J::Array(items.iter().map(Node::to_json).collect()),
            Value::Map(entries) => J::Object(
                entries
                    .iter()
                    .map(|e| (e.key.clone(), e.value.to_json()))
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

struct Lines<'a> {
    src: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(src: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        Lines { src, starts }
    }

    fn pos(&self, m: saphyr_parser::Marker) -> Pos {
        let line = m.line().max(1);
        let start = self.starts.get(line - 1).copied().unwrap_or(self.src.len());
        let offset = self.src[start..]
            .char_indices()
            .nth(m.col())
            .map_or(self.src.len(), |(i, _)| start + i);
        Pos {
            offset,
            line: m.line(),
            col: m.col() + 1,
        }
    }
}

enum Frame {
    Seq(Vec<Node>, Span),
    Map(Vec<Entry>, Span, Option<(String, Span)>),
}

pub fn parse(src: &str) -> Result<Node, Vec<ParseError>> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;
    let mut errors = Vec::new();
    let mut documents = 0;

    let lines = Lines::new(src);
    for item in Parser::new_from_str(src) {
        let (event, span) = match item {
            Ok(v) => v,
            Err(e) => {
                let p = lines.pos(*e.marker());
                errors.push(ParseError {
                    message: format!("invalid syntax: {}", e.info()),
                    span: Span::point(p),
                });
                return Err(errors);
            }
        };
        let span = Span {
            start: lines.pos(span.start),
            end: lines.pos(span.end),
        };
        let node = match event {
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    errors.push(ParseError {
                        message: "the file contains more than one YAML document".into(),
                        span,
                    });
                    return Err(errors);
                }
                continue;
            }
            Event::StreamStart | Event::StreamEnd | Event::DocumentEnd | Event::Nothing => {
                continue;
            }
            Event::Alias(_) => {
                errors.push(ParseError {
                    message: "YAML aliases (*name) are not supported".into(),
                    span,
                });
                return Err(errors);
            }
            Event::SequenceStart(..) => {
                stack.push(Frame::Seq(Vec::new(), span));
                continue;
            }
            Event::MappingStart(..) => {
                stack.push(Frame::Map(Vec::new(), span, None));
                continue;
            }
            Event::SequenceEnd => match stack.pop() {
                Some(Frame::Seq(items, start)) => Node {
                    value: Value::Seq(items),
                    span: Span {
                        start: start.start,
                        end: span.end,
                    },
                    quoted: false,
                },
                _ => unreachable!("parser emits balanced events"),
            },
            Event::MappingEnd => match stack.pop() {
                Some(Frame::Map(entries, start, _)) => Node {
                    value: Value::Map(entries),
                    span: Span {
                        start: start.start,
                        end: span.end,
                    },
                    quoted: false,
                },
                _ => unreachable!("parser emits balanced events"),
            },
            Event::Scalar(text, style, _, _) => {
                if let Some(Frame::Map(_, _, pending @ None)) = stack.last_mut() {
                    *pending = Some((text.to_string(), span));
                    continue;
                }
                scalar(&text, style, span)
            }
        };

        match stack.last_mut() {
            None => root = Some(node),
            Some(Frame::Seq(items, _)) => items.push(node),
            Some(Frame::Map(entries, _, pending)) => match pending.take() {
                None => {
                    errors.push(ParseError {
                        message: format!("mapping keys must be strings, found {}", node.kind()),
                        span: node.span,
                    });
                    *pending = Some((String::new(), node.span));
                }
                Some((key, key_span)) => {
                    if entries.iter().any(|e| e.key == key) {
                        errors.push(ParseError {
                            message: format!("duplicate key \"{key}\""),
                            span: key_span,
                        });
                    } else {
                        entries.push(Entry {
                            key,
                            key_span,
                            value: node,
                        });
                    }
                }
            },
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(root.unwrap_or(Node {
        value: Value::Null,
        span: Span::default(),
        quoted: false,
    }))
}

fn scalar(text: &str, style: ScalarStyle, span: Span) -> Node {
    let quoted = !matches!(style, ScalarStyle::Plain);
    let value = if quoted {
        Value::Str(text.to_string())
    } else {
        plain(text)
    };
    Node {
        value,
        span,
        quoted,
    }
}

fn plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = text.parse::<i64>() {
        return Value::Int(i);
    }
    let numeric = text
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'));
    if numeric
        && text.bytes().any(|b| b.is_ascii_digit())
        && let Ok(f) = text.parse::<f64>()
    {
        return Value::Float(f);
    }
    Value::Str(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_kept_as_written() {
        let n = parse("007: a\n7: b\nTrue: c\n1.50: d\n\"q\": e\n").unwrap();
        let keys: Vec<&str> = match &n.value {
            Value::Map(entries) => entries.iter().map(|e| e.key.as_str()).collect(),
            _ => panic!(),
        };
        assert_eq!(keys, ["007", "7", "True", "1.50", "q"]);
        assert!(parse("[a]: x\n").is_err());
    }

    #[test]
    fn offsets_are_bytes_with_multibyte_text() {
        let src = "a: \"ƿƿ\"\nb: é1\n";
        let n = parse(src).unwrap();
        let b = n.entry("b").unwrap();
        assert_eq!(
            &src[b.value.span.start.offset..b.value.span.end.offset],
            "é1"
        );
        assert_eq!(b.value.span.start.col, 4);
    }

    #[test]
    fn positions_are_one_based() {
        let n = parse("a: 1\nbb:\n  c: hello\n").unwrap();
        let bb = n.entry("bb").unwrap();
        assert_eq!((bb.key_span.start.line, bb.key_span.start.col), (2, 1));
        let c = bb.value.entry("c").unwrap();
        assert_eq!((c.value.span.start.line, c.value.span.start.col), (3, 6));
        assert_eq!(c.value.span.end.col, 11);
    }

    #[test]
    fn scalars_follow_core_schema() {
        let n = parse("a: 1\nb: \"1\"\nc: true\nd: ~\ne: 1.5\nf: 0.0.0.0:80\n").unwrap();
        assert_eq!(n.get("a").unwrap().value, Value::Int(1));
        assert_eq!(n.get("b").unwrap().value, Value::Str("1".into()));
        assert_eq!(n.get("c").unwrap().value, Value::Bool(true));
        assert_eq!(n.get("d").unwrap().value, Value::Null);
        assert_eq!(n.get("e").unwrap().value, Value::Float(1.5));
        assert_eq!(n.get("f").unwrap().value, Value::Str("0.0.0.0:80".into()));
    }

    #[test]
    fn json_is_parsed() {
        let n = parse("{\n  \"a\": [1, \"x\"],\n  \"b\": {\"c\": false}\n}").unwrap();
        assert_eq!(
            n.to_json(),
            serde_json::json!({"a": [1, "x"], "b": {"c": false}})
        );
        let b = n.entry("b").unwrap();
        assert_eq!((b.key_span.start.line, b.key_span.start.col), (3, 3));
    }

    #[test]
    fn duplicate_keys_are_errors() {
        let errs = parse("a: 1\na: 2\n").unwrap_err();
        assert_eq!(errs[0].message, "duplicate key \"a\"");
        assert_eq!(errs[0].span.start.line, 2);
    }

    #[test]
    fn syntax_errors_have_position() {
        let errs = parse("a: [1, 2\nb: 3\n").unwrap_err();
        assert!(errs[0].message.starts_with("invalid syntax"));
    }
}
