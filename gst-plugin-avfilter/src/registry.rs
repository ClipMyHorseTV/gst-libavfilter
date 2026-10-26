use gst::glib;
use std::sync::LazyLock;

static CAT: LazyLock<gst::DebugCategory> =
    LazyLock::new(|| gst::DebugCategory::new("avfilter", gst::DebugColorFlags::empty(), Some("libavfilter elements")));

/// One wrapped filter.
pub struct Entry {
    /// libavfilter filter name
    pub filter: &'static str,
    /// GStreamer element name
    pub element: &'static str,
    /// GPL-only in FFmpeg's configure (e.g. eq, hqdn3d). Needs the `gpl` cargo feature.
    pub gpl_only: bool,
    /// FFmpeg option names the element sets. All must exist in the linked FFmpeg.
    pub options: &'static [&'static str],
    pub register: fn(&gst::Plugin) -> Result<(), glib::BoolError>,
}

pub static ENTRIES: &[Entry] = &[];

pub fn register_all(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    for e in ENTRIES {
        let Some(info) = avfilter::FilterInfo::lookup(e.filter) else {
            gst::info!(CAT, "{}: filter '{}' not in this FFmpeg build, skipping", e.element, e.filter);
            continue;
        };
        if let Some(missing) = e.options.iter().find(|o| !info.has_option(o)) {
            gst::warning!(CAT, "{}: '{}' has no option '{}' in this FFmpeg, skipping", e.element, e.filter, missing);
            continue;
        }
        if e.gpl_only && !cfg!(feature = "gpl") {
            gst::info!(CAT, "{}: GPL-only filter and the `gpl` feature is off, skipping", e.element);
            continue;
        }
        (e.register)(plugin)?;
    }
    Ok(())
}
