use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use chromiumoxide::cdp::browser_protocol::emulation::{
    MediaFeature, SetDeviceMetricsOverrideParams,
};
use chromiumoxide::cdp::browser_protocol::log::EventEntryAdded;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EventConsoleApiCalled, EventExceptionThrown,
};
use chromiumoxide::page::{Page, ScreenshotParams};
use futures_util::StreamExt;
use serde_json::Value;

use crate::sheet::Flow;

pub fn chrome() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BURROW_E2E_CHROME") {
        return Some(PathBuf::from(p));
    }
    [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

pub struct Ui {
    pub browser: Browser,
    pub page: Page,
    pub errors: Arc<Mutex<Vec<String>>>,
    pub flow: Flow,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("a JSON string")
}

impl Ui {
    pub async fn launch(chrome: &Path, flow: Flow, profile: &Path) -> Ui {
        let config = BrowserConfig::builder()
            .chrome_executable(chrome)
            .user_data_dir(profile)
            .window_size(1440, 900)
            .viewport(None)
            .arg("--headless=new")
            .arg("--no-first-run")
            .arg("--disable-gpu")
            .arg("--hide-scrollbars")
            .arg("--font-render-hinting=none")
            .arg("--force-device-scale-factor=1")
            .launch_timeout(Duration::from_secs(30))
            .request_timeout(Duration::from_secs(30))
            .build()
            .expect("a browser config");
        let (browser, mut handler) = Browser::launch(config).await.expect("Chrome starts");
        let mut tasks = vec![tokio::spawn(async move {
            while let Some(r) = handler.next().await {
                if r.is_err() {
                    break;
                }
            }
        })];
        let page = browser.new_page("about:blank").await.expect("a tab");
        page.execute(SetDeviceMetricsOverrideParams::new(1440, 900, 1.0, false))
            .await
            .expect("viewport");
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mut thrown = page
            .event_listener::<EventExceptionThrown>()
            .await
            .expect("events");
        let e1 = errors.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(e) = thrown.next().await {
                let d = &e.exception_details;
                let what = d
                    .exception
                    .as_ref()
                    .and_then(|x| x.description.clone())
                    .unwrap_or_else(|| d.text.clone());
                e1.lock().expect("lock").push(format!("exception: {what}"));
            }
        }));
        let mut console = page
            .event_listener::<EventConsoleApiCalled>()
            .await
            .expect("events");
        let e2 = errors.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(e) = console.next().await {
                if e.r#type == ConsoleApiCalledType::Error {
                    let text: Vec<String> = e
                        .args
                        .iter()
                        .map(|a| {
                            a.value
                                .as_ref()
                                .map(|v| v.to_string())
                                .or(a.description.clone())
                                .unwrap_or_default()
                        })
                        .collect();
                    e2.lock()
                        .expect("lock")
                        .push(format!("console.error: {}", text.join(" ")));
                }
            }
        }));
        let _ = page.enable_log().await;
        let mut log = page
            .event_listener::<EventEntryAdded>()
            .await
            .expect("events");
        let e3 = errors.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(e) = log.next().await {
                let level = format!("{:?}", e.entry.level).to_lowercase();
                if level == "error" {
                    e3.lock().expect("lock").push(format!(
                        "log: {} {}",
                        e.entry.text,
                        e.entry.url.clone().unwrap_or_default()
                    ));
                }
            }
        }));
        Ui {
            browser,
            page,
            errors,
            flow,
            _tasks: tasks,
        }
    }

    pub async fn goto(&self, url: &str) {
        match tokio::time::timeout(Duration::from_secs(15), self.page.goto(url)).await {
            Ok(Ok(_)) => {}
            Ok(Err(chromiumoxide::error::CdpError::Timeout)) | Err(_) => {
                let now: String = self.eval("location.href").await;
                assert!(
                    now.split('#').next() == url.split('#').next(),
                    "navigating to {url} timed out at {now}"
                );
            }
            Ok(Err(e)) => panic!("navigate to {url}: {e}"),
        }
    }

    pub async fn eval<T: serde::de::DeserializeOwned>(&self, expr: &str) -> T {
        let started = Instant::now();
        loop {
            match self.page.evaluate(expr).await {
                Ok(v) => {
                    return v
                        .into_value::<T>()
                        .unwrap_or_else(|e| panic!("evaluate {expr}: {e}"));
                }
                Err(e)
                    if e.to_string().contains("context")
                        && started.elapsed() < Duration::from_secs(10) =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("evaluate {expr}: {e}"),
            }
        }
    }

    pub async fn count(&self, css: &str) -> usize {
        self.eval::<usize>(&format!(
            "document.querySelectorAll({}).length",
            js_str(css)
        ))
        .await
    }

    pub async fn text(&self, css: &str) -> String {
        self.eval::<Option<String>>(&format!(
            "(() => {{ const e = document.querySelector({}); return e ? e.innerText : ''; }})()",
            js_str(css)
        ))
        .await
        .unwrap_or_default()
    }

    pub async fn all_attrs(&self, css: &str, name: &str) -> Vec<String> {
        self.eval::<Vec<String>>(&format!(
            "Array.from(document.querySelectorAll({})).map(e => e.getAttribute({}) || '')",
            js_str(css),
            js_str(name)
        ))
        .await
    }

    pub async fn wait_for(&self, css: &str, timeout: Duration) {
        let started = Instant::now();
        while self.count(css).await == 0 {
            assert!(
                started.elapsed() < timeout,
                "{css} did not appear within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_gone(&self, css: &str, timeout: Duration) {
        let started = Instant::now();
        while self.count(css).await > 0 {
            assert!(
                started.elapsed() < timeout,
                "{css} did not go away within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_text(&self, css: &str, wanted: &str, timeout: Duration) -> String {
        let started = Instant::now();
        loop {
            let t = self.text(css).await;
            if t.contains(wanted) {
                return t;
            }
            assert!(
                started.elapsed() < timeout,
                "{css} did not show {wanted:?} within {timeout:?}; it shows {t:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_not_text(&self, css: &str, unwanted: &str, timeout: Duration) -> String {
        let started = Instant::now();
        loop {
            let t = self.text(css).await;
            if !t.is_empty() && !t.contains(unwanted) {
                return t;
            }
            assert!(
                started.elapsed() < timeout,
                "{css} still shows {unwanted:?} after {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn click(&self, css: &str) {
        self.wait_for(css, Duration::from_secs(10)).await;
        let el = self.page.find_element(css).await.expect("element");
        el.scroll_into_view().await.expect("scroll");
        el.click().await.expect("click");
    }

    pub async fn press(&self, css: &str, key: &str) {
        let el = self.page.find_element(css).await.expect("element");
        el.press_key(key).await.expect("key");
    }

    pub async fn nav(&self, path: &str) {
        let _: bool = self
            .eval(&format!(
                "(() => {{ history.pushState(null, '', {}); dispatchEvent(new PopStateEvent('popstate')); return true; }})()",
                js_str(path)
            ))
            .await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    pub async fn viewport(&self, w: i64, h: i64) {
        self.page
            .execute(SetDeviceMetricsOverrideParams::new(w, h, 1.0, w < 600))
            .await
            .expect("viewport");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    pub async fn dark(&self, on: bool) {
        self.page
            .emulate_media_features(vec![MediaFeature::new(
                "prefers-color-scheme",
                if on { "dark" } else { "light" },
            )])
            .await
            .expect("media");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    pub async fn png(&self) -> Vec<u8> {
        self.page
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .full_page(true)
                    .build(),
            )
            .await
            .expect("screenshot")
    }

    pub async fn shot(&mut self, step: &str) {
        let png = self.png().await;
        self.flow.save(step, &png);
    }

    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock().expect("lock"))
    }

    pub fn assert_no_errors(&self) {
        let e = self.take_errors();
        assert!(e.is_empty(), "the page logged errors: {e:#?}");
    }

    pub async fn ax_tree(&self) -> Vec<Value> {
        let r = self
            .page
            .execute(GetFullAxTreeParams::default())
            .await
            .expect("accessibility tree");
        r.result
            .nodes
            .iter()
            .map(|n| serde_json::to_value(n).expect("JSON"))
            .collect()
    }

    pub async fn close(mut self) {
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
    }
}
