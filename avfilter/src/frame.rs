use crate::error::{Error, check};
use crate::{formats, sys};
use std::ffi::CStr;
use std::ptr::{self, NonNull};

/// An owned AVFrame. Freed automatically when dropped.
pub struct Frame(NonNull<sys::AVFrame>);

unsafe impl Send for Frame {}

impl Frame {
    pub fn empty() -> Result<Self, Error> {
        let ptr = unsafe { sys::av_frame_alloc() };
        NonNull::new(ptr).map(Frame).ok_or(Error::OutOfMemory)
    }

    /// A frame with freshly allocated (writable, aligned) pixel buffers.
    pub fn new_video(format: sys::AVPixelFormat, width: i32, height: i32) -> Result<Self, Error> {
        let mut f = Self::empty()?;
        let raw = f.raw_mut();
        raw.format = format;
        raw.width = width;
        raw.height = height;
        check("av_frame_get_buffer", unsafe { sys::av_frame_get_buffer(f.as_mut_ptr(), 0) })?;
        Ok(f)
    }

    /// Another reference to the same data (cheap, no pixel copy).
    pub fn try_clone(&self) -> Result<Self, Error> {
        let ptr = unsafe { sys::av_frame_clone(self.as_ptr()) };
        NonNull::new(ptr).map(Frame).ok_or(Error::OutOfMemory)
    }

    pub fn as_ptr(&self) -> *const sys::AVFrame {
        self.0.as_ptr()
    }
    pub fn as_mut_ptr(&mut self) -> *mut sys::AVFrame {
        self.0.as_ptr()
    }
    fn raw(&self) -> &sys::AVFrame {
        unsafe { self.0.as_ref() }
    }
    fn raw_mut(&mut self) -> &mut sys::AVFrame {
        unsafe { self.0.as_mut() }
    }

    pub fn width(&self) -> i32 {
        self.raw().width
    }
    pub fn height(&self) -> i32 {
        self.raw().height
    }
    pub fn format(&self) -> sys::AVPixelFormat {
        self.raw().format
    }

    pub fn pts(&self) -> Option<i64> {
        let p = self.raw().pts;
        (p != sys::AV_NOPTS_VALUE).then_some(p)
    }
    pub fn set_pts(&mut self, pts: Option<i64>) {
        self.raw_mut().pts = pts.unwrap_or(sys::AV_NOPTS_VALUE)
    }
    pub fn duration(&self) -> i64 {
        self.raw().duration
    }
    pub fn set_duration(&mut self, d: i64) {
        self.raw_mut().duration = d;
    }

    pub fn is_interlaced(&self) -> bool {
        self.raw().flags & sys::AV_FRAME_FLAG_INTERLACED as i32 != 0
    }
    pub fn top_field_first(&self) -> bool {
        self.raw().flags & sys::AV_FRAME_FLAG_TOP_FIELD_FIRST as i32 != 0
    }
    pub fn set_interlacing(&mut self, interlaced: bool, tff: bool) {
        let f = &mut self.raw_mut().flags;
        for (flag, on) in
            [(sys::AV_FRAME_FLAG_INTERLACED as i32, interlaced), (sys::AV_FRAME_FLAG_TOP_FIELD_FIRST as i32, tff)]
        {
            if on { *f |= flag } else { *f &= !flag }
        }
    }
    pub fn set_color_range(&mut self, range: sys::AVColorRange) {
        self.raw_mut().color_range = range;
    }
    pub fn set_colourspace(&mut self, space: sys::AVColorSpace) {
        self.raw_mut().colorspace = space;
    }

    /// Copy one plane *into* this frame (frame must be writable, e.g. from `new_video`).
    pub fn copy_plance_from(&mut self, plane: usize, src: &[u8], src_stride: usize) -> Result<(), Error> {
        let (bytewidth, rows) =
            formats::plane_geometry(self.format(), self.width(), self.height(), plane).ok_or(Error::BadPlane(plane))?;
        if rows == 0 {
            return Ok(());
        }
        if src.len() < src_stride * (rows - 1) + bytewidth {
            return Err(Error::BufferTooSmall);
        }
        let raw = self.raw_mut();
        unsafe {
            sys::av_image_copy_plane(
                raw.data[plane],
                raw.linesize[plane],
                src.as_ptr(),
                src_stride as i32,
                bytewidth as i32,
                rows as i32,
            );
        }
        Ok(())
    }

    /// Copy one plane *out of* this frame into `dst`.
    pub fn copy_plane_to(&self, plane: usize, dst: &mut [u8], dst_stride: usize) -> Result<(), Error> {
        let (bytewidth, rows) =
            formats::plane_geometry(self.format(), self.width(), self.height(), plane).ok_or(Error::BadPlane(plane))?;
        if rows == 0 {
            return Ok(());
        }
        if dst.len() < dst_stride * (rows - 1) + bytewidth {
            return Err(Error::BufferTooSmall);
        }
        let raw = self.raw();
        unsafe {
            sys::av_image_copy_plane(
                dst.as_mut_ptr(),
                dst_stride as i32,
                raw.data[plane],
                raw.linesize[plane],
                bytewidth as i32,
                rows as i32,
            );
        }
        Ok(())
    }

    pub fn metadata(&self) -> Metadata {
        let mut out = Vec::new();
        let dict = self.raw().metadata;
        let mut entry: *const sys::AVDictionaryEntry = ptr::null();
        loop {
            entry = unsafe { sys::av_dict_iterate(dict, entry) };
            let Some(e) = (unsafe { entry.as_ref() }) else { break };
            let key = unsafe { CStr::from_ptr(e.key) }.to_string_lossy().into_owned();
            let value = unsafe { CStr::from_ptr(e.value) }.to_string_lossy().into_owned();
            out.push((key, value));
        }
        Metadata(out)
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let mut ptr = self.0.as_ptr();
        unsafe { sys::av_frame_free(&mut ptr) };
    }
}

/// Frame metadata as owned key/value pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata(pub Vec<(String, String)>);

impl Metadata {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
