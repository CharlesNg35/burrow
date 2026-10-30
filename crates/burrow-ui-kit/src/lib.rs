//! Generic Leptos components for the admin UI: inputs, tables, charts, dialogs and path diagrams.
//! It knows nothing about burrow and must not depend on burrow-admin-proto, burrow-syntax or burrow-admin-logic.

#![forbid(unsafe_code)]

mod bar;
mod basic;
mod code;
mod combo;
mod disclosure;
mod feedback;
mod field;
mod icon;
mod input;
mod overlay;
mod path;
mod scroll;
mod spark;
mod toast;

pub use bar::StickyBar;
pub use basic::{
    Button, Card, Chip, Fact, Facts, Kbd, Kpi, LABEL, LINK, LockLine, MENU_ITEM, MONO, Mono, Muted,
    PageHeader, PillTabs, SectionHead, SpinDot, Stat, Stats, StatusDot, TabMark, Table, Tabs, Tag,
    Tone, button_class, card_head, chip_class, menu_class,
};
pub use code::{CodeBlock, DiffLine, DiffView, copy_text, download, download_text};
pub use combo::{ComboOption, Combobox, filter_options};
pub use disclosure::Disclosure;
pub use feedback::{Banner, Callout, EmptyState, Skeleton, Spinner};
pub use field::{FieldRow, FieldState, field_message, tag_class};
pub use icon::{Icon, path as icon_path};
pub use input::{
    ChipInput, ConfirmInput, Segmented, Select, TextArea, TextInput, Toggle, field_class,
    split_entries,
};
pub use overlay::{
    Dialog, Drawer, Tooltip, active as active_element, focus_first, trap as trap_focus,
};
pub use path::{Edge, EdgeState, Hop, HopState, PathData, PathDiagram};
pub use scroll::{ScrollRow, active_item};
pub use spark::{ChartData, LineChart, Sparkline, segments, spark_max, spark_path};
pub use toast::{Toast, ToastHost};
