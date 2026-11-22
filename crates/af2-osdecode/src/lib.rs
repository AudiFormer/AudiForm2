// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Decoding through the operating system's own decoders, for files the
//! open decoders cannot read: Dolby Digital (AC-3), Dolby Digital Plus
//! (E-AC-3, which is how Dolby Atmos music comes in M4A and MP4), Dolby
//! AC-4 (when Dolby's decoder is installed), HE-AAC and multichannel AAC.
//!
//! Windows: Media Foundation's Source Reader, asked for 32-bit float at
//! the file's own channel count (the Dolby decoders otherwise mix down to
//! stereo). An Atmos file comes out as its 5.1 or 7.1 bed, which carries
//! the whole mix; the height objects need Dolby's renderer, which these
//! decoders do not apply. Channels come in WAVE order (L, R, C, LFE, then
//! the surround pairs).
//!
//! Other systems: [`decode`] says it is not available there.
//!
//! All the `unsafe` operating-system calls live in this crate.

use std::path::Path;

/// Decoded audio: the rate, and one Vec per channel.
pub struct Decoded {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

/// Whether this system has decoders this crate can use.
pub fn available() -> bool {
    cfg!(windows)
}

/// Decodes the whole file with the system's decoders.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    #[cfg(windows)]
    {
        win::decode(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("this system has no decoders AudiForm 2 can use for it".into())
    }
}

#[cfg(windows)]
mod win {
    use super::Decoded;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;

    /// COM and Media Foundation for one decode, undone when dropped.
    struct Session {
        com: bool,
    }

    impl Session {
        fn start() -> Result<Session, String> {
            // Fails harmlessly when the thread already has COM in another mode.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
                if com {
                    unsafe { CoUninitialize() };
                }
                return Err(format!("Media Foundation did not start: {e}"));
            }
            Ok(Session { com })
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    fn float_type(channels: Option<u32>) -> windows::core::Result<IMFMediaType> {
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            if let Some(n) = channels {
                t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, n)?;
            }
            Ok(t)
        }
    }

    pub fn decode(path: &Path) -> Result<Decoded, String> {
        let _mf = Session::start()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None) }
            .map_err(|e| format!("Windows could not open it: {e}"))?;
        unsafe {
            let _ = reader.SetStreamSelection(ALL, false);
            reader
                .SetStreamSelection(FIRST_AUDIO, true)
                .map_err(|_| "Windows found no audio in it".to_string())?;
        }
        let native = unsafe { reader.GetNativeMediaType(FIRST_AUDIO, 0) }
            .map_err(|_| "Windows found no audio in it".to_string())?;
        let native_ch = unsafe { native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.unwrap_or(2);
        // Every channel the file has; plain float if the decoder will not.
        let set = float_type(Some(native_ch))
            .and_then(|t| unsafe { reader.SetCurrentMediaType(FIRST_AUDIO, None, &t) })
            .or_else(|_| {
                float_type(None)
                    .and_then(|t| unsafe { reader.SetCurrentMediaType(FIRST_AUDIO, None, &t) })
            });
        set.map_err(|e| format!("Windows has no decoder for it ({e})"))?;
        let out = unsafe { reader.GetCurrentMediaType(FIRST_AUDIO) }.map_err(|e| e.to_string())?;
        let ch = unsafe { out.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.map_err(|e| e.to_string())?
            as usize;
        let rate = unsafe { out.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }
            .map_err(|e| e.to_string())?;
        if ch == 0 || rate == 0 {
            return Err("Windows gave no audio for it".into());
        }
        let mut inter: Vec<f32> = Vec::new();
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            unsafe {
                reader
                    .ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .map_err(|e| format!("Windows stopped decoding it: {e}"))?;
            }
            if let Some(s) = sample {
                let buf = unsafe { s.ConvertToContiguousBuffer() }.map_err(|e| e.to_string())?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                unsafe {
                    buf.Lock(&mut ptr, None, Some(&mut len)).map_err(|e| e.to_string())?;
                    if !ptr.is_null() && len >= 4 {
                        let n = len as usize / 4;
                        // The buffer is 32-bit float, but only byte-aligned for certain.
                        let bytes = std::slice::from_raw_parts(ptr, n * 4);
                        inter.extend(
                            bytes
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                        );
                    }
                    let _ = buf.Unlock();
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        let frames = inter.len() / ch;
        let mut channels: Vec<Vec<f32>> = (0..ch).map(|_| Vec::with_capacity(frames)).collect();
        for f in inter.chunks_exact(ch) {
            for (c, s) in channels.iter_mut().zip(f) {
                c.push(*s);
            }
        }
        Ok(Decoded { rate, channels })
    }
}
