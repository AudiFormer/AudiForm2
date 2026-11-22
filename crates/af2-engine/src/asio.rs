// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Opening one ASIO driver by name.
//!
//! cpal's ASIO host lists devices by loading every installed driver in
//! turn. Here only the chosen driver is loaded, through `asio-sys` (the
//! crate cpal's ASIO host is built on), and wrapped as a cpal device
//! with cpal's public types, so cpal itself needs no patch.
//!
//! ASIO allows one loaded driver per process, and the input and output
//! streams of a driver must share one device value (cpal keeps the
//! driver's buffers in it). So the device is kept here, per thread: every
//! ASIO call is made on the UI thread, which opens all streams.
//!
//! Without the `asio` feature (or off Windows) [`device`] says ASIO is not
//! in this build and nothing else is compiled.

#[cfg(all(windows, feature = "asio"))]
mod imp {
    use std::cell::RefCell;
    use std::sync::atomic::AtomicI32;
    use std::sync::{Arc, Mutex};

    thread_local! {
        static ASIO: asio_sys::Asio = asio_sys::Asio::new();
        static LOADED: RefCell<Option<(String, cpal::Device)>> = const { RefCell::new(None) };
    }

    pub fn device(name: &str) -> Result<cpal::Device, String> {
        LOADED.with(|loaded| {
            let mut loaded = loaded.borrow_mut();
            if let Some((n, d)) = loaded.as_ref() {
                if n == name {
                    return Ok(d.clone());
                }
            }
            // Another driver: let go of ours first (it unloads once no
            // stream holds it any more).
            *loaded = None;
            let driver = ASIO.with(|a| a.load_driver(name)).map_err(|e| {
                format!(
                    "The ASIO driver \"{name}\" did not open ({e:?}). Another program, or \
                     another ASIO driver still in use, may be holding it."
                )
            })?;
            let dev = cpal::platform::AsioDevice {
                driver: Arc::new(driver),
                asio_streams: Arc::new(Mutex::new(asio_sys::AsioStreams {
                    input: None,
                    output: None,
                })),
                current_buffer_index: Arc::new(AtomicI32::new(-1)),
            };
            let dev = cpal::Device::from(dev);
            *loaded = Some((name.to_string(), dev.clone()));
            Ok(dev)
        })
    }

    pub fn release() {
        LOADED.with(|l| *l.borrow_mut() = None);
    }
}

#[cfg(not(all(windows, feature = "asio")))]
mod imp {
    pub fn device(_name: &str) -> Result<cpal::Device, String> {
        Err(crate::devices::NO_ASIO.into())
    }

    pub fn release() {}
}

/// The ASIO driver `name` as a cpal device, loading it if it is not
/// loaded yet. UI thread only.
pub fn device(name: &str) -> Result<cpal::Device, String> {
    imp::device(name)
}

/// Lets go of the loaded driver (it unloads once no stream holds it), for
/// switching to the system driver or another ASIO driver.
pub fn release() {
    imp::release()
}
