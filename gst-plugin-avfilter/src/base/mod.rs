pub mod convert;
pub mod video;

/// The nick of a registered GEnum value. Our nicks are chosen to equal FFmpeg's constant names,
/// so this is what we pass to av_opt_set ("mci", "epzs", …).
pub fn enum_nick(value: impl gst::glib::value::ToValue) -> String {
    let v = value.to_value();
    gst::glib::EnumValue::from_value(&v).map(|(_, ev)| ev.nick().to_owned()).unwrap_or_default()
}
