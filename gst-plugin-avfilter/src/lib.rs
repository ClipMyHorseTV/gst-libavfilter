//! GStreamer elements wrapping FFmpeg libavfilter filters.

use gst::glib;

mod base;
mod filters;
pub mod registry;

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    registry::register_all(plugin)
}

gst::plugin_define!(
    avfilter, // plugin name (must match libgst<name>.so)
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("COMMIT_ID")),
    "LGPL", // GStreamer's license identifier
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    env!("BUILD_REL_DATE")
);
