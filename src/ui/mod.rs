mod app;
mod icons;
mod list;
mod opsui;
mod props;
pub(crate) use props::{
    PROPERTIES_WINDOW_DEFAULT_HEIGHT, PROPERTIES_WINDOW_DEFAULT_WIDTH, PROPERTY_PANEL_FIXED_WIDTH,
    property_panel_fixed_range,
};
mod style;
mod tabs;
mod tree;

pub use app::FileManagerApp;
