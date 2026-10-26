
use crate::error::{AvError, Error, check};
use crate::{FilterOptions, Frame, Rational, sys};
use std::ffi::{CStr, CString, c_char};
use std::ptr::{self, NonNull};

/// Input description for the `buffer` source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoParams {
    pub format: sys::AVPixelFormat,
    pub width: i32,
    pub height: i32,
    pub time_base: Rational,
    pub frame_rate: Rational,
    pub sample_aspect_ratio: Rational,
}

/// Result of pulling from the graph.
pub enum Pulled {
    Frame(Frame),
    Again, // needs more input
    Eof,   // finished after close()
}

pub struct Graph {
    graph: NonNull<sys::AVFilterGraph>,
    src: *mut sys::AVFilterContext,  // owned by `graph`
    sink: *mut sys::AVFilterContext, // owned by `graph`
}

unsafe impl Send for Graph {}

impl Graph {
    pub const FILTER_INSTANCE: &'static CStr = c"filter";

    pub fn video(params: &VideoParams, filter: &str, opts: &FilterOptions, threads: u32) -> Result<Self, Error> {
        let raw = unsafe { sys::avfilter_graph_alloc() };
        let mut g =
            Graph { graph: NonNull::new(raw).ok_or(Error::OutOfMemory)?, src: ptr::null_mut(), sink: ptr::null_mut() };

        unsafe {
            sys::avfilter_graph_set_auto_convert(raw, sys::AVFILTER_AUTO_CONVERT_NONE as u32);
            (*raw).nb_threads = threads as i32;

            g.src = g.alloc_filter("buffer", c"in")?;
            let par = sys::av_buffersrc_parameters_alloc();
            if par.is_null() {
                return Err(Error::OutOfMemory);
            }
            (*par).format = params.format;
            (*par).width = params.width;
            (*par).height = params.height;
            (*par).time_base = params.time_base;
            (*par).frame_rate = params.frame_rate;
            (*par).sample_aspect_ratio = params.sample_aspect_ratio;
            let ret = sys::av_buffersrc_parameters_set(g.src, par);
            sys::av_free(par.cast());
            check("av_buffersrc_paramters_set", ret)?;
            check("init buffer", sys::avfilter_init_str(g.src, ptr::null()))?;

            let f = g.alloc_filter(filter, Self::FILTER_INSTANCE)?;
            opts.apply(f)?;
            check("init filter", sys::avfilter_init_str(f, ptr::null()))?;

            g.sink = g.alloc_filter("buffersink", c"out")?;
            check("init buffersink", sys::avfilter_init_str(g.sink, ptr::null()))?;

            check("avfilter_link", sys::avfilter_link(g.src, 0, f, 0))?;
            check("avfilter_link", sys::avfilter_link(f, 0, g.sink, 0))?;
            check("avfilter_graph_config", sys::avfilter_graph_config(raw, ptr::null_mut()))?;
        }
        Ok(g)
    }

    /// # Safety: internal helper; returns a context owned by the graph.
    pub fn alloc_filter(&mut self, name: &str, instance: &CStr) -> Result<*mut sys::AVFilterContext, Error> {
        let cname = CString::new(name)?;
        let filter = unsafe { sys::avfilter_get_by_name(cname.as_ptr()) };
        if filter.is_null() {
            return Err(Error::FilterNotFound(name.to_owned()));
        }
        let ctx = unsafe { sys::avfilter_graph_alloc_filter(self.graph.as_ptr(), filter, instance.as_ptr()) };
        if ctx.is_null() { Err(Error::OutOfMemory) } else { Ok(ctx) }
    }

    /// Push one frame. Ownership of its data moves into the graph.
    pub fn push(&mut self, mut frame: Frame) -> Result<(), Error> {
        let ret = unsafe { sys::av_buffersrc_add_frame_flags(self.src, frame.as_mut_ptr(), 0) };
        check("av_buffersrc_add_frame_flags", ret).map(|_| ())
    }

    /// Pull one frame if available.
    pub fn pull(&mut self) -> Result<Pulled, Error> {
        let mut frame = Frame::empty()?;
        let ret = unsafe { sys::av_buffersink_get_frame(self.sink, frame.as_mut_ptr()) };
        match ret {
            r if r >= 0 => Ok(Pulled::Frame(frame)),
            sys::AVERROR_EAGAIN => Ok(Pulled::Again),
            sys::AVERROR_EOF => Ok(Pulled::Eof),
            r => Err(Error::Av { op: "av_buffersink_get_frame", code: AvError(r) }),
        }
    }

    /// Signal end of stream. `pts` (in the input time base) is where the stream ends.
    pub fn close(&mut self, pts: Option<i64>) -> Result<(), Error> {
        let ret = unsafe {
            sys::av_buffersrc_close(self.src, pts.unwrap_or(sys::AV_NOPTS_VALUE), sys::AV_BUFFERSRC_FLAG_PUSH as u32)
        };
        check("av_buffersrc_close", ret).map(|_| ())
    }

    /// What the graph actually produces (after negotiation). Filters may change any of this.
    pub fn output_video(&self) -> VideoParams {
        let s = self.sink;
        unsafe {
            VideoParams {
                format: sys::av_buffersink_get_format(s),
                width: sys::av_buffersink_get_w(s),
                height: sys::av_buffersink_get_h(s),
                time_base: sys::av_buffersink_get_time_base(s),
                frame_rate: sys::av_buffersink_get_frame_rate(s),
                sample_aspect_ratio: sys::av_buffersink_get_sample_aspect_ratio(s),
            }
        }
    }

    pub fn send_command(&mut self, cmd: &str, arg: &str) -> Result<(), Error> {
        let (c, a) = (CString::new(cmd)?, CString::new(arg)?);
        let mut resp = [0 as c_char; 256];
        let ret = unsafe {
            sys::avfilter_graph_send_command(
                self.graph.as_ptr(),
                Self::FILTER_INSTANCE.as_ptr(),
                c.as_ptr(),
                a.as_ptr(),
                resp.as_mut_ptr(),
                resp.len() as i32,
                0,
            )
        };
        check("avfilter_graph_send_command", ret).map(|_| ())
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        let mut p = self.graph.as_ptr();
        unsafe { sys::avfilter_graph_free(&mut p) };
    }
}
