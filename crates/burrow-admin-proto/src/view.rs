use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Shown {
    #[default]
    Written,
    WithDefaults,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShownRequest {
    #[serde(default)]
    pub show: Shown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewSection {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigView {
    pub rev: String,
    pub path: String,
    #[serde(default)]
    pub show: Shown,
    #[serde(default)]
    pub sections: Vec<ViewSection>,
}

pub const REDACTED: &str = "redacted";

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

impl ConfigView {
    pub fn download_name(&self) -> String {
        let name = file_name(&self.path);
        let stem = name
            .rsplit_once('.')
            .map_or(name, |(s, _)| s)
            .trim()
            .to_string();
        let stem = if stem.is_empty() {
            "server".into()
        } else {
            stem
        };
        format!("{stem}.{REDACTED}.yaml")
    }

    pub fn header(&self) -> String {
        let defaults = match self.show {
            Shown::Written => "",
            Shown::WithDefaults => ", with every default in effect",
        };
        format!(
            "# {}{defaults}; secrets show as fingerprints or ${{ENV}} names\n",
            file_name(&self.path)
        )
    }

    pub fn body(&self) -> String {
        let mut out = String::new();
        let mut from: Option<&str> = None;
        for s in &self.sections {
            if s.origin.as_deref() != from {
                from = s.origin.as_deref();
                if let Some(o) = from {
                    out.push_str(&format!("# from {o}\n"));
                }
            }
            out.push_str(&s.text);
        }
        out
    }

    pub fn text(&self) -> String {
        format!("{}{}", self.header(), self.body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(key: &str, origin: Option<&str>, text: &str) -> ViewSection {
        ViewSection {
            key: key.into(),
            origin: origin.map(str::to_string),
            text: text.into(),
        }
    }

    #[test]
    fn the_download_is_named_as_a_redacted_copy_of_the_file() {
        let mut v = ConfigView {
            path: "/etc/burrow/home/server.yaml".into(),
            ..ConfigView::default()
        };
        assert_eq!(v.download_name(), "server.redacted.yaml");
        v.path = r"C:\burrow\edge.json".into();
        assert_eq!(v.download_name(), "edge.redacted.yaml");
        v.path = String::new();
        assert_eq!(v.download_name(), "server.redacted.yaml");
    }

    #[test]
    fn the_text_says_what_it_is_and_marks_each_change_of_origin() {
        let mut v = ConfigView {
            path: "/etc/burrow/server.yaml".into(),
            sections: vec![
                section("listen", None, "listen: 7835\n"),
                section("web", Some("server.d/10-web.yaml"), "web:\n  http: 80\n"),
                section("metrics", Some("server.d/10-web.yaml"), "metrics: {}\n"),
            ],
            ..ConfigView::default()
        };
        assert_eq!(
            v.body(),
            "listen: 7835\n# from server.d/10-web.yaml\nweb:\n  http: 80\nmetrics: {}\n"
        );
        assert!(v.text().starts_with("# server.yaml; secrets show as"));
        v.show = Shown::WithDefaults;
        assert!(v.header().contains("with every default in effect"));
        assert!(v.text().ends_with(&v.body()));
    }

    #[test]
    fn a_request_without_a_body_asks_for_what_is_written() {
        let r: ShownRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(r.show, Shown::Written);
        let r: ShownRequest =
            serde_json::from_value(serde_json::json!({"show": "with_defaults"})).unwrap();
        assert_eq!(r.show, Shown::WithDefaults);
    }
}
