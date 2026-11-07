use crate::ui::{Screens, Ui};

pub const DESIGN: &str = include_str!("design.js");

pub async fn assert_clean(ui: &Ui, step: &str) {
    if ui.screens == Screens::Older || std::env::var("BURROW_E2E_SKIP_DESIGN").as_deref() == Ok("1")
    {
        return;
    }
    ui.wait_true(
        "document.fonts.status === 'loaded'",
        "the fonts are loaded",
        std::time::Duration::from_secs(10),
    )
    .await;
    let bad: Vec<String> = ui.eval(DESIGN).await;
    assert!(
        bad.is_empty(),
        "{step}: the screen breaks the design rules of docs/admin-ui-dev.md: {bad:#?}"
    );
}
