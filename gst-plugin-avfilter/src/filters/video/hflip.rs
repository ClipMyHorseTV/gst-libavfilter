use crate::base::video::{AvVideoFilterImpl, FilterBehavior, av_video_filter_element};
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;

#[derive(Default)]
pub struct Hflip;

impl ObjectImpl for Hflip {}

impl AvVideoFilterImpl for Hflip {
    const FILTER: &'static str = "hflip";
    // Stateless: dropping frames under QoS is harmless.
    const BEHAVIOR: FilterBehavior = FilterBehavior { qos_safe: true, ..FilterBehavior::DEFAULT };
}

av_video_filter_element!(Hflip, AvFilterHflip, "avfilter_hflip", "GstAvFilterHflip", "Filter/Effect/Video");
