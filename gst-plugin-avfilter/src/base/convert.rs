use avfilter::{Frame, Rational, VideoParams, sys};
use gst_video::prelude::*;
use gst_video::{VideoBufferFlags, VideoFormat as G, VideoInterlaceMode};

/// GStreamer <-> FFmpeg pixel formats
const FORMATS: &[(G, sys::AVPixelFormat)] = &[
    // planar YUV 8-bit
    (G::I420, sys::AV_PIX_FMT_YUV420P),
    (G::Y42b, sys::AV_PIX_FMT_YUV422P),
    (G::Y444, sys::AV_PIX_FMT_YUV444P),
    (G::Y41b, sys::AV_PIX_FMT_YUV411P),
    (G::Yuv9, sys::AV_PIX_FMT_YUV410P),
    (G::A420, sys::AV_PIX_FMT_YUVA420P),
    // planar YUV high bit depth (little endian)
    (G::I42010le, sys::AV_PIX_FMT_YUV420P10LE),
    (G::I42210le, sys::AV_PIX_FMT_YUV422P10LE),
    (G::Y44410le, sys::AV_PIX_FMT_YUV444P10LE),
    (G::I42012le, sys::AV_PIX_FMT_YUV420P12LE),
    // semi-planar / packed YUV
    (G::Nv12, sys::AV_PIX_FMT_NV12),
    (G::Nv21, sys::AV_PIX_FMT_NV21),
    (G::Yuy2, sys::AV_PIX_FMT_YUYV422),
    (G::Uyvy, sys::AV_PIX_FMT_UYVY422),
    // gray
    (G::Gray8, sys::AV_PIX_FMT_GRAY8),
    (G::Gray16Le, sys::AV_PIX_FMT_GRAY16LE),
    // RGB
    (G::Rgba, sys::AV_PIX_FMT_RGBA),
    (G::Bgra, sys::AV_PIX_FMT_BGRA),
    (G::Argb, sys::AV_PIX_FMT_ARGB),
    (G::Abgr, sys::AV_PIX_FMT_ABGR),
    (G::Rgbx, sys::AV_PIX_FMT_RGB0),
    (G::Bgrx, sys::AV_PIX_FMT_BGR0),
    (G::Rgb, sys::AV_PIX_FMT_RGB24),
    (G::Bgr, sys::AV_PIX_FMT_BGR24),
    (G::Gbr, sys::AV_PIX_FMT_GBRP),
];

pub fn to_av_format(f: G) -> Option<sys::AVPixelFormat> {
    FORMATS.iter().find(|(g, _)| *g == f).map(|&(_, a)| a)
}

pub fn to_gst_format(a: sys::AVPixelFormat) -> Option<G> {
    FORMATS.iter().find(|(_, x)| *x == a).map(|&(g, _)| g)
}
pub fn all_av_formats() -> Vec<sys::AVPixelFormat> {
    FORMATS.iter().map(|&(_, a)| a).collect()
}

/// GStreamer timestamps are nanoseconds; feeding FFmpeg a 1/1e9 time base keeps them lossless.
pub const NS_TIME_BASE: Rational = Rational { num: 1, den: 1_000_000_000 };

pub fn video_params(info: &gst_video::VideoInfo) -> Option<VideoParams> {
    let (fps, par) = (info.fps(), info.par());
    Some(VideoParams {
        format: to_av_format(info.format())?,
        width: info.width() as i32,
        height: info.height() as i32,
        time_base: NS_TIME_BASE,
        frame_rate: avfilter::rational(fps.numer(), fps.denom()),
        sample_aspect_ratio: avfilter::rational(par.numer(), par.denom()),
    })
}

/// FFmpeg timestamp (in `tb` units) -> GStreamer ClockTime. Negative/unset -> None.
pub fn av_to_clock(ts: i64, tb: Rational) -> Option<gst::ClockTime> {
    if ts < 0 {
        return None; // GstClockTime cannot be negative; AV_NOPTS_VALUE is i64::MIN
    }
    let ns = avfilter::rescale(ts, tb, NS_TIME_BASE);
    Some(gst::ClockTime::from_nseconds(ns as u64))
}

/// One frame's duration from the caps framerate, if known.
pub fn frame_duration(info: &gst_video::VideoInfo) -> Option<gst::ClockTime> {
    let fps = info.fps();
    (fps.numer() > 0).then(|| {
        gst::ClockTime::SECOND.mul_div_floor(fps.denom() as u64, fps.numer() as u64).unwrap_or(gst::ClockTime::ZERO)
    })
}

fn apply_colorimetry(frame: &mut Frame, info: &gst_video::VideoInfo) {
    use gst_video::{VideoColorMatrix as M, VideoColorRange as R};
    let c = info.colorimetry();
    frame.set_color_range(match c.range() {
        R::Range0_255 => sys::AVCOL_RANGE_JPEG,
        R::Range16_235 => sys::AVCOL_RANGE_MPEG,
        _ => sys::AVCOL_RANGE_UNSPECIFIED,
    });
    frame.set_colorspace(match c.matrix() {
        M::Bt709 => sys::AVCOL_SPC_BT709,
        M::Bt601 => sys::AVCOL_SPC_SMPTE170M,
        M::Bt2020 => sys::AVCOL_SPC_BT2020_NCL,
        _ => sys::AVCOL_SPC_UNSPECIFIED,
    });
}

pub fn buffer_to_frame(buf: &gst::BufferRef, info: &gst_video::VideoInfo) -> Result<Frame, gst::FlowError> {
    let fmt = to_av_format(info.format()).ok_or(gst::FlowError::NotNegotiated)?;
    let vframe = gst_video::VideoFrameRef::from_buffer_ref_readable(buf, info).map_err(|_| gst::FlowError::Error)?;
    let mut frame =
        Frame::new_video(fmt, info.width() as i32, info.height() as i32).map_err(|_| gst::FlowError::Error)?;

    for plane in 0..vframe.n_planes() {
        let stride = vframe.plane_stride()[plane as usize] as usize;
        let data = vframe.plane_data(plane).map_err(|_| gst::FlowError::Error)?;
        frame.copy_plane_from(plane as usize, data, stride).map_err(|_| gst::FlowError::Error)?;
    }

    frame.set_pts(buf.pts().map(|t| t.nseconds() as i64));
    let dur = buf.duration().or_else(|| frame_duration(info));
    frame.set_duration(dur.map_or(0, |d| d.nseconds() as i64));

    let vflags = buf.video_flags();
    let interlaced = match info.interlace_mode() {
        VideoInterlaceMode::Interleaved => true,
        VideoInterlaceMode::Mixed => vflags.contains(VideoBufferFlags::INTERLACED),
        _ => false,
    };
    let tff = vflags.contains(VideoBufferFlags::TFF) || info.field_order() == gst_video::VideoFieldOrder::TopFieldFirst;
    frame.set_interlacing(interlaced, interlaced && tff);
    apply_colorimetry(&mut frame, info);
    Ok(frame)
}

pub fn frame_to_buffer(
    frame: &Frame,
    info: &gst_video::VideoInfo,
    tb: Rational,
) -> Result<gst::Buffer, gst::FlowError> {
    let mut buf = gst::Buffer::with_size(info.size()).map_err(|_| gst::FlowError::Error)?;
    {
        let b = buf.get_mut().expect("fresh buffer is writable");
        b.set_pts(frame.pts().and_then(|p| av_to_clock(p, tb)));
        let dur = if frame.duration() > 0 { av_to_clock(frame.duration(), tb) } else { frame_duration(info) };
        b.set_duration(dur);
        if info.interlace_mode() == VideoInterlaceMode::Mixed && frame.is_interlaced() {
            let mut f = VideoBufferFlags::INTERLACED;
            if frame.top_field_first() {
                f |= VideoBufferFlags::TFF;
            }
            b.set_video_flags(f);
        }

        let mut vframe =
            gst_video::VideoFrameRef::from_buffer_ref_writable(b, info).map_err(|_| gst::FlowError::Error)?;
        let strides: Vec<i32> = vframe.plane_stride().to_vec();
        for plane in 0..vframe.n_planes() {
            let dst = vframe.plane_data_mut(plane).map_err(|_| gst::FlowError::Error)?;
            frame
                .copy_plane_to(plane as usize, dst, strides[plane as usize] as usize)
                .map_err(|_| gst::FlowError::Error)?;
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_table_round_trips() {
        for &(g, a) in FORMATS {
            assert_eq!(to_av_format(g), Some(a), "{g:?}");
            assert_eq!(to_gst_format(a), Some(g), "{}", avfilter::formats::name(a));
        }
    }

    #[test]
    fn formats_have_same_plane_count() {
        gst::init().unwrap();
        for &(g, a) in FORMATS {
            let info = gst_video::VideoInfo::builder(g, 64, 64).build().unwrap();
            assert_eq!(info.n_planes() as usize, avfilter::formats::plane_count(a), "{g:?}");
        }
    }

    #[test]
    fn buffer_frame_buffer_is_identity() {
        gst::init().unwrap();
        let info = gst_video::VideoInfo::builder(G::I420, 64, 48).fps(gst::Fraction::new(25, 1)).build().unwrap();
        let mut buf = gst::Buffer::with_size(info.size()).unwrap();
        {
            let b = buf.get_mut().unwrap();
            b.map_writable().unwrap().iter_mut().enumerate().for_each(|(i, p)| *p = (i % 251) as u8);
            b.set_pts(gst::ClockTime::from_mseconds(40));
        }
        let frame = buffer_to_frame(&buf, &info).unwrap();
        let back = frame_to_buffer(&frame, &info, NS_TIME_BASE).unwrap();
        assert_eq!(back.pts(), buf.pts());
        assert_eq!(&*back.map_readable().unwrap(), &*buf.map_readable().unwrap());
    }
}
