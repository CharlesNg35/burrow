use std::fs;
use std::path::PathBuf;

pub fn shots_dir() -> PathBuf {
    std::env::var_os("BURROW_E2E_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp/admin-ui-shots"))
}

pub struct Flow {
    pub name: String,
    pub dir: PathBuf,
    pub n: usize,
    pub steps: Vec<(String, String)>,
}

impl Flow {
    pub fn new(name: &str) -> Flow {
        let dir = shots_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a screenshot folder");
        Flow {
            name: name.to_string(),
            dir,
            n: 0,
            steps: Vec::new(),
        }
    }

    pub fn save(&mut self, step: &str, png: &[u8]) {
        self.n += 1;
        let file = format!("{:02}-{step}.png", self.n);
        fs::write(self.dir.join(&file), png).expect("write a screenshot");
        self.steps
            .push((step.to_string(), format!("{}/{file}", self.name)));
    }

    pub fn failure(&self, name: &str, bytes: &[u8]) {
        let dir = self.dir.join("failure");
        fs::create_dir_all(&dir).expect("a failure folder");
        fs::write(dir.join(name), bytes).expect("write a failure file");
    }

    pub fn finish(&self, passed: bool, message: &str) {
        let results = shots_dir().join("results");
        fs::create_dir_all(&results).expect("a results folder");
        let row = serde_json::json!({
            "flow": self.name,
            "passed": passed,
            "message": message,
            "steps": self.steps.iter().map(|(s, f)| serde_json::json!({"step": s, "file": f})).collect::<Vec<_>>(),
        });
        let tmp = results.join(format!("{}.json.tmp", self.name));
        fs::write(&tmp, row.to_string()).expect("write a result");
        fs::rename(&tmp, results.join(format!("{}.json", self.name))).expect("store a result");
        write_index();
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

static INDEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn write_index() {
    let _one = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = shots_dir();
    let mut rows: Vec<serde_json::Value> = fs::read_dir(root.join("results"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    rows.sort_by(|a, b| a["flow"].as_str().cmp(&b["flow"].as_str()));
    let passed = rows.iter().filter(|r| r["passed"] == true).count();
    let mut html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>burrow admin e2e screenshots</title>\n<style>body{{font:14px system-ui;margin:24px;background:#f7f8fa}}h2{{font-size:15px;margin-top:28px}}.ok{{color:#1a7f37}}.bad{{color:#cf222e}}pre{{white-space:pre-wrap;color:#cf222e}}\n.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:12px}}figure{{margin:0;background:#fff;border:1px solid #d7dce3;border-radius:6px;padding:6px}}img{{width:100%;height:160px;object-fit:cover;object-position:top}}figcaption{{font-size:12px;word-break:break-all}}</style>\n<h1>burrow admin e2e: {passed}/{} flows passed</h1>\n",
        rows.len()
    );
    for r in &rows {
        let ok = r["passed"] == true;
        html.push_str(&format!(
            "<section><h2><span class=\"{}\">{}</span> {}</h2>",
            if ok { "ok" } else { "bad" },
            if ok { "passed" } else { "failed" },
            esc(r["flow"].as_str().unwrap_or_default())
        ));
        if !ok {
            html.push_str(&format!(
                "<pre>{}</pre>",
                esc(r["message"].as_str().unwrap_or_default())
            ));
        }
        html.push_str("<div class=\"grid\">");
        for s in r["steps"].as_array().into_iter().flatten() {
            let f = esc(s["file"].as_str().unwrap_or_default());
            let step = esc(s["step"].as_str().unwrap_or_default());
            html.push_str(&format!(
                "<figure><a href=\"{f}\"><img loading=\"lazy\" src=\"{f}\" alt=\"{step}\"></a><figcaption>{f}</figcaption></figure>"
            ));
        }
        html.push_str("</div></section>\n");
    }
    let tmp = root.join("index.html.tmp");
    fs::write(&tmp, html).expect("write the contact sheet");
    fs::rename(&tmp, root.join("index.html")).expect("store the contact sheet");
}
