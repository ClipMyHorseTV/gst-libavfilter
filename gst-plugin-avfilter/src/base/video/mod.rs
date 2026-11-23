use crate::base::convert;
use avfilter::{FilterOptions, Metadata};
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;

mod imp;

glib::wrapper! {
    pub struct AvVideoFilter(ObjectSubclass<imp::AvVideoFilter>)
        @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

impl AvVideoFilter {
    /// A property that can't change at runtime was changed: rebuild the graph before the next buffer.
    pub fn request_rebuild(&self) {
        self.imp().request_rebuild();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FilterBehavior {
    /// Stateless filter: QoS may drop input frames.
    pub qos_safe: bool,
    /// Filter doesn't flush its tail at EOF: feed this many copies of the last frame, then clip.
    pub eos_tail_frames: u32,
}

impl FilterBehavior {
    pub const DEFAULT: Self = Self { qos_safe: false, eos_tail_frames: 0 };
}

pub trait AvVideoFilterImpl: BaseTransformImpl + ObjectSubclass<Type: IsA<AvVideoFilter>> {
    const FILTER: &'static str;
    const BEHAVIOR: FilterBehavior = FilterBehavior::DEFAULT;

    /// Options used for format probing at class-init time (no instance exists yet).
    fn probe_options() -> FilterOptions {
        FilterOptions::new()
    }
    /// Options from the current property values (called on every graph build).
    fn options(&self) -> FilterOptions {
        FilterOptions::new()
    }
    /// Caps on one side → caps on the other side. Identity by default.
    fn map_caps(&self, _direction: gst::PadDirection, caps: &gst::Caps) -> gst::Caps {
        caps.clone()
    }
    /// Extra latency this filter adds for the given input format.
    fn latency(&self, _info: &gst_video::VideoInfo) -> gst::ClockTime {
        gst::ClockTime::ZERO
    }
    /// Current settings are a no-op → let BaseTransform pass buffers straight through.
    fn wants_passthrough(&self) -> bool {
        false
    }
    /// Called (outside any lock) for every output frame that carries frame metadata.
    fn handle_metadata(&self, _meta: &Metadata, _pts: Option<gst::ClockTime>) {}
}

/// Function pointers the base class calls; filled per subclass in `class_init`.
#[derive(Clone, Copy)]
struct Hooks {
    options: fn(&AvVideoFilter) -> FilterOptions,
    map_caps: fn(&AvVideoFilter, gst::PadDirection, &gst::Caps) -> gst::Caps,
    latency: fn(&AvVideoFilter, &gst_video::VideoInfo) -> gst::ClockTime,
    wants_passthrough: fn(&AvVideoFilter) -> bool,
    handle_metadata: fn(&AvVideoFilter, &Metadata, Option<gst::ClockTime>),
}

#[repr(C)]
pub struct Class {
    parent: gst_base::ffi::GstBaseTransformClass,
    filter: &'static str,
    behavior: FilterBehavior,
    hooks: Hooks,
}

unsafe impl ClassStruct for Class {
    type Type = imp::AvVideoFilter;
}

impl std::ops::Deref for Class {
    type Target = glib::Class<gst_base::BaseTransform>;
    fn deref(&self) -> &Self::Target {
        // SAFETY: `parent` is the first field of a #[repr(C)] struct.
        unsafe { &*(&self.parent as *const _ as *const _) }
    }
}

/// Runs once per *subclass* type: copy its consts and point the hooks at its methods.
unsafe impl<T: AvVideoFilterImpl> IsSubclassable<T> for AvVideoFilter {
    fn class_init(class: &mut glib::Class<Self>) {
        Self::parent_class_init::<T>(class);
        let class = class.as_mut();
        class.filter = T::FILTER;
        class.behavior = T::BEHAVIOR;
        // SAFETY (all closures): GObject only calls these on instances of T's type.
        class.hooks = Hooks {
            options: |obj| unsafe { obj.unsafe_cast_ref::<T::Type>() }.imp().options(),
            map_caps: |obj, dir, caps| unsafe { obj.unsafe_cast_ref::<T::Type>() }.imp().map_caps(dir, caps),
            latency: |obj, info| unsafe { obj.unsafe_cast_ref::<T::Type>() }.imp().latency(info),
            wants_passthrough: |obj| unsafe { obj.unsafe_cast_ref::<T::Type>() }.imp().wants_passthrough(),
            handle_metadata: |obj, m, pts| unsafe { obj.unsafe_cast_ref::<T::Type>() }.imp().handle_metadata(m, pts),
        };
    }
}

pub fn element_metadata(filter: &str, classification: &str) -> gst::subclass::ElementMetadata {
    let desc = avfilter::FilterInfo::lookup(filter).map(|i| i.description()).unwrap_or_default();
    gst::subclass::ElementMetadata::new(
        &format!("FFmpeg {filter}"),
        classification,
        &desc,
        "Simon Koester <s.koester@clipmyhorse.tv>",
    )
}

/// Pad templates from probing which formats the filter supports natively.
pub fn pad_templates<T: AvVideoFilterImpl>() -> Vec<gst::PadTemplate> {
    let formats: Vec<gst_video::VideoFormat> =
        avfilter::probe_video_formats(T::FILTER, &T::probe_options(), &convert::all_av_formats())
            .into_iter()
            .filter_map(convert::to_gst_format)
            .collect();
    let caps = gst_video::VideoCapsBuilder::new().format_list(formats).build();
    vec![
        gst::PadTemplate::new("sink", gst::PadDirection::Sink, gst::PadPresence::Always, &caps).unwrap(),
        gst::PadTemplate::new("src", gst::PadDirection::Src, gst::PadPresence::Always, &caps).unwrap(),
    ]
}

/// Generates the per-element boilerplate. Usage:
/// `av_video_filter_element!(Hflip, AvFilterHflip, "avfilter_hflip", "GstAvFilterHflip", "Filter/Effect/Video");`
macro_rules! av_video_filter_element {
    ($imp:ident, $wrapper:ident, $element:literal, $gtype:literal, $classification:literal) => {
        #[glib::object_subclass]
        impl ObjectSubclass for $imp {
            const NAME: &'static str = $gtype;
            type Type = $wrapper;
            type ParentType = $crate::base::video::AvVideoFilter;
        }

        impl GstObjectImpl for $imp {}

        impl ElementImpl for $imp {
            fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
                static META: std::sync::LazyLock<gst::subclass::ElementMetadata> = std::sync::LazyLock::new(|| {
                    $crate::base::video::element_metadata(
                        <$imp as $crate::base::video::AvVideoFilterImpl>::FILTER,
                        $classification,
                    )
                });
                Some(&*META)
            }
            fn pad_templates() -> &'static [gst::PadTemplate] {
                static TEMPLATES: std::sync::LazyLock<Vec<gst::PadTemplate>> =
                    std::sync::LazyLock::new($crate::base::video::pad_templates::<$imp>);
                TEMPLATES.as_ref()
            }
        }

        impl BaseTransformImpl for $imp {
            const MODE: gst_base::subclass::BaseTransformMode = gst_base::subclass::BaseTransformMode::NeverInPlace;
            const PASSTHROUGH_ON_SAME_CAPS: bool = false;
            const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;
        }

        glib::wrapper! {
            pub struct $wrapper(ObjectSubclass<$imp>)
                @extends $crate::base::video::AvVideoFilter, gst_base::BaseTransform, gst::Element, gst::Object;
        }

        pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
            gst::Element::register(Some(plugin), $element, gst::Rank::NONE, $wrapper::static_type())
        }
    };
}
pub(crate) use av_video_filter_element;
