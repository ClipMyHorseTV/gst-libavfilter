use crate::sys;

/// Number of planes for a pixel format (e.g. 3 for YUV420P, 2 for NV12, 1 for RGBA).
pub fn plane_count(format: sys::AVPixelFormat) -> usize {
    let n = unsafe { sys::av_pix_fmt_count_planes(format) };
    n.max(0) as usize
}

pub fn plane_geometry(format: sys::AVPixelFormat, width: i32, height: i32, plane: usize) -> Option<(usize, usize)> {
    let desc = unsafe { sys::av_pix_fmt_desc_get(format).as_ref()? };
    let bytewidth = unsafe { sys::av_image_get_linesize(format, width, plane as i32) };
    if bytewidth <= 0 {
        return None;
    }
    let rows = if plane == 1 || plane == 2 { -((-height) >> desc.log2_chroma_h) } else { height };
    Some((bytewidth as usize, rows as usize))
}

pub fn name(format: sys::AVPixelFormat) -> String {
    let p = unsafe { sys::av_get_pix_fmt_name(format) };
    if p.is_null() {
        return format!("unknown({format})");
    }
    unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
}
