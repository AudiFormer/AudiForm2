// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Video frames through the system's decoders (Windows: Media
//! Foundation's Source Reader with its video processor, which converts to
//! RGB and scales). A reader is made and used on one thread.

use std::path::Path;

/// A video stream's size, rate and length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

/// One picture: its time in the file and RGBA pixels, row by row.
#[derive(Clone)]
pub struct Frame {
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Opens the first video stream, decoding at `width` pixels across (the
/// height follows the picture's shape); None decodes at full size.
pub fn open(path: &Path, width: Option<u32>) -> Result<(VideoReader, VideoInfo), String> {
    #[cfg(windows)]
    {
        win::open(path, width)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, width);
        Err("The system's video decoders aren't used on this system.".into())
    }
}

pub struct VideoReader {
    #[cfg(windows)]
    inner: win::Reader,
}

impl VideoReader {
    /// Moves to `t` seconds; the next frame is the last one at or before.
    pub fn seek(&mut self, t: f64) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.seek(t)
        }
        #[cfg(not(windows))]
        {
            let _ = t;
            Err("unavailable".into())
        }
    }

    /// The next frame, or None at the end.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        #[cfg(windows)]
        {
            self.inner.next()
        }
        #[cfg(not(windows))]
        {
            Ok(None)
        }
    }
}

/// An even size `width` across with the picture's shape.
pub fn scaled(w: u32, h: u32, width: Option<u32>) -> (u32, u32) {
    match width {
        Some(nw) if nw < w && nw > 0 => {
            let nw = (nw.max(16) + 1) & !1;
            let nh = ((h as f64 * nw as f64 / w as f64).round() as u32).max(2);
            (nw, (nh + 1) & !1)
        }
        _ => (w, h),
    }
}

/// Shrinks an RGBA picture by averaging boxes (when the system won't
/// scale).
pub fn shrink(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let (w, h, nw, nh) = (w as usize, h as usize, nw as usize, nh as usize);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1).min(h));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1).min(w));
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = (sy * w + sx) * 4;
                    for k in 0..4 {
                        acc[k] += src[p + k] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let q = (y * nw + x) * 4;
            for k in 0..4 {
                out[q + k] = (acc[k] / n.max(1)) as u8;
            }
        }
    }
    out
}

#[cfg(windows)]
mod win {
    use super::{Frame, VideoInfo, VideoReader};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::{GUID, PCWSTR, PROPVARIANT};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;

    struct Mf {
        com: bool,
    }

    impl Drop for Mf {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    pub struct Reader {
        reader: IMFSourceReader,
        /// What the decoder gives (it may not have scaled).
        out_w: u32,
        out_h: u32,
        stride: i32,
        /// What was asked for.
        want_w: u32,
        want_h: u32,
        // Dropped last: the reader goes before Media Foundation stops.
        _mf: Mf,
    }

    fn rgb_type(size: Option<(u32, u32)>) -> windows::core::Result<IMFMediaType> {
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            if let Some((w, h)) = size {
                t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
            }
            Ok(t)
        }
    }

    pub fn open(path: &Path, width: Option<u32>) -> Result<(VideoReader, VideoInfo), String> {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .map_err(|e| format!("Media Foundation did not start: {e}"))?;
        let mf = Mf { com };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let reader = unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs
                .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(|e| e.to_string())?;
            MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs)
        }
        .map_err(|e| format!("Windows could not open it: {e}"))?;
        unsafe {
            let _ = reader.SetStreamSelection(ALL, false);
            reader.SetStreamSelection(VIDEO, true).map_err(|_| "It has no video.".to_string())?;
        }
        let native = unsafe { reader.GetNativeMediaType(VIDEO, 0) }.map_err(|_| "It has no video.".to_string())?;
        let size = unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(|_| "Its video has no size.".to_string())?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        if w == 0 || h == 0 {
            return Err("Its video has no size.".into());
        }
        let fps = unsafe { native.GetUINT64(&MF_MT_FRAME_RATE) }
            .ok()
            .map(|r| ((r >> 32) as f64, (r & 0xffff_ffff) as f64))
            .filter(|(n, d)| *n > 0.0 && *d > 0.0)
            .map_or(30.0, |(n, d)| n / d);
        let duration = unsafe { reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) }
            .ok()
            .and_then(|v| u64::try_from(&v).ok())
            .map_or(0.0, |hns| hns as f64 / 1e7);
        let (want_w, want_h) = super::scaled(w, h, width);
        // Scaled by the system when it will; else full size, shrunk here.
        let ok = rgb_type(Some((want_w, want_h)))
            .and_then(|t| unsafe { reader.SetCurrentMediaType(VIDEO, None, &t) })
            .or_else(|_| rgb_type(None).and_then(|t| unsafe { reader.SetCurrentMediaType(VIDEO, None, &t) }));
        ok.map_err(|e| format!("Windows has no decoder for this video ({e})"))?;
        let cur = unsafe { reader.GetCurrentMediaType(VIDEO) }.map_err(|e| e.to_string())?;
        let osize = unsafe { cur.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(size);
        let (out_w, out_h) = ((osize >> 32) as u32, osize as u32);
        let stride = unsafe { cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) }.map(|s| s as i32).unwrap_or(out_w as i32 * 4);
        let info = VideoInfo { width: w, height: h, fps, duration };
        let inner = Reader { reader, out_w, out_h, stride, want_w, want_h, _mf: mf };
        Ok((VideoReader { inner }, info))
    }

    impl Reader {
        pub fn seek(&mut self, t: f64) -> Result<(), String> {
            let pos = PROPVARIANT::from((t.max(0.0) * 1e7) as i64);
            unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &pos) }.map_err(|e| e.to_string())
        }

        pub fn next(&mut self) -> Result<Option<Frame>, String> {
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                unsafe {
                    self.reader
                        .ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))
                        .map_err(|e| e.to_string())?;
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(None);
                }
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                    if let Ok(cur) = unsafe { self.reader.GetCurrentMediaType(VIDEO) } {
                        if let Ok(s) = unsafe { cur.GetUINT64(&MF_MT_FRAME_SIZE) } {
                            self.out_w = (s >> 32) as u32;
                            self.out_h = s as u32;
                        }
                        self.stride = unsafe { cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
                            .map(|s| s as i32)
                            .unwrap_or(self.out_w as i32 * 4);
                    }
                }
                let Some(s) = sample else { continue };
                let time = unsafe { s.GetSampleTime() }.unwrap_or(0) as f64 / 1e7;
                let buf = unsafe { s.ConvertToContiguousBuffer() }.map_err(|e| e.to_string())?;
                let (w, h) = (self.out_w as usize, self.out_h as usize);
                let mut rgba = vec![255u8; w * h * 4];
                unsafe {
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut len = 0u32;
                    buf.Lock(&mut ptr, None, Some(&mut len)).map_err(|e| e.to_string())?;
                    let pitch = self.stride.unsigned_abs() as usize;
                    if !ptr.is_null() && pitch >= w * 4 && len as usize >= pitch * h {
                        let bytes = std::slice::from_raw_parts(ptr, pitch * h);
                        for y in 0..h {
                            // A negative stride: bottom row first.
                            let row = if self.stride < 0 { h - 1 - y } else { y };
                            let src = &bytes[row * pitch..row * pitch + w * 4];
                            let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                                d[0] = s[2];
                                d[1] = s[1];
                                d[2] = s[0];
                            }
                        }
                    }
                    let _ = buf.Unlock();
                }
                let (mut fw, mut fh) = (self.out_w, self.out_h);
                if fw > self.want_w || fh > self.want_h {
                    rgba = super::shrink(&rgba, fw, fh, self.want_w, self.want_h);
                    fw = self.want_w;
                    fh = self.want_h;
                }
                return Ok(Some(Frame { time, width: fw, height: fh, rgba }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_keep_the_shape_and_stay_even() {
        assert_eq!(scaled(1920, 1080, Some(960)), (960, 540));
        assert_eq!(scaled(1920, 1080, Some(480)), (480, 270));
        assert_eq!(scaled(640, 360, None), (640, 360));
        assert_eq!(scaled(640, 360, Some(1000)), (640, 360));
        let (w, h) = scaled(1280, 720, Some(321));
        assert!(w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn shrinking_averages() {
        let src = [10u8, 20, 30, 255, 30, 40, 50, 255, 50, 60, 70, 255, 70, 80, 90, 255];
        assert_eq!(shrink(&src, 2, 2, 1, 1), vec![40, 50, 60, 255]);
    }

    /// `AF2_VIDEO=<file> cargo test -p af2-osdecode -- --ignored reads_frames --nocapture`
    #[test]
    #[ignore]
    fn reads_frames_from_a_file() {
        let p = std::path::PathBuf::from(std::env::var("AF2_VIDEO").expect("AF2_VIDEO"));
        let (mut r, info) = open(&p, Some(320)).map_err(|e| eprintln!("open: {e}")).unwrap();
        eprintln!("{info:?}");
        let f = r.next_frame().unwrap().unwrap();
        eprintln!("first frame at {:.3} s, {}x{}", f.time, f.width, f.height);
        assert_eq!(f.rgba.len(), (f.width * f.height * 4) as usize);
        // A seek lands on the keyframe before; frames run on from there.
        let mid = info.duration / 2.0;
        r.seek(mid).unwrap();
        let mut g = r.next_frame().unwrap().unwrap();
        eprintln!("seek to {mid:.2} s lands at {:.3} s", g.time);
        while g.time + 0.5 / info.fps < mid {
            g = r.next_frame().unwrap().unwrap();
        }
        eprintln!("decoded on to {:.3} s", g.time);
        let mean: f64 = g.rgba.iter().map(|x| *x as f64).sum::<f64>() / g.rgba.len() as f64;
        assert!(mean > 5.0, "not black: {mean}");
    }
}
