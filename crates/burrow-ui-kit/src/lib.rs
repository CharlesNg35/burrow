//! Generic Leptos components for the admin UI: inputs, tables, charts, dialogs and path diagrams.
//! It knows nothing about burrow and must not depend on burrow-admin-proto, burrow-syntax or burrow-admin-logic.

#![forbid(unsafe_code)]

mod bar;
mod basic;
mod button;
mod code;
mod combo;
mod deferred;
mod disclosure;
mod feedback;
mod field;
mod icon;
mod inline;
mod input;
mod overlay;
mod path;
mod radio;
mod room;
mod scroll;
mod spark;
mod stepper;
mod toast;
mod version;

pub use bar::{StickyBar, bar_status, bar_top, view_height};
pub use basic::{
    CARD_VIEW, Card, Chip, Dot, Fact, Facts, Kbd, Kpi, LABEL, LINK, LockLine, PageHeader, PillTabs,
    SectionHead, Stat, Stats, StatusDot, TABLE_VIEW, TabMark, Table, Tabs, Tag, Tone, WIDE_ONLY,
    card_head, chip_class,
};
pub use button::{
    Button, ButtonLook, ButtonSize, IconButton, LinkButton, MENU_ITEM, MenuItem, SpinDot,
    button_class, menu_class,
};
pub use code::{CodeBlock, DiffLine, DiffView, copy_text, download, download_text};
pub use combo::{ComboOption, Combobox, filter_options};
pub use deferred::{Deferred, LoadFailed, Needs, can_run};
pub use disclosure::Disclosure;
pub use feedback::{Banner, Callout, DANGER_CARD, EmptyState, Skeleton, Spinner};
pub use field::{FieldRow, FieldState, field_message, tag_class};
pub use icon::{Icon, path as icon_path};
pub use inline::{Inline, inline};
pub use input::{
    ChipInput, ConfirmInput, INPUT_H, Segmented, Select, TextArea, TextInput, Toggle, field_class,
    split_entries,
};
pub use overlay::{
    Dialog, Modal, Tooltip, active as active_element, auto_titles, focus_first, trap as trap_focus,
};
pub use path::{Edge, EdgeState, Hop, HopState, PathData, PathDiagram};
pub use radio::{RadioList, RadioOption};
pub use room::cover_top;
pub use scroll::{ScrollRow, active_item};
pub use spark::{ChartData, LineChart, segments, spark_max, spark_path};
pub use stepper::{Stepper, stepped};
pub use toast::{MAX_TOASTS, Toast, ToastHost, push_toast};
pub use version::VersionTag;
