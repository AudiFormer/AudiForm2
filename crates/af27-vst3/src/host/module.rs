// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Opening a plug-in bundle: its library, entry call and factory.

use std::path::{Path, PathBuf};

use vst3::ComPtr;
use vst3::Steinberg::{
    IPluginFactory, IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo,
    PClassInfo2, PFactoryInfo,
};

use crate::protocol::cid_hex;

/// The library inside a bundle: `Contents/x86_64-win/<name>.vst3` for a
/// bundle folder, the file itself for a single-file plug-in.
pub fn library_path(bundle: &Path) -> PathBuf {
    if bundle.is_dir() {
        let stem = bundle
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64-win"
        } else {
            "x86_64-win"
        };
        let dir = bundle.join("Contents").join(arch);
        let named = dir.join(format!("{stem}.vst3"));
        if named.is_file() {
            return named;
        }
        // Some bundles name the library differently from the folder.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("vst3"))
                    && p.is_file()
                {
                    return p;
                }
            }
        }
        named
    } else {
        bundle.to_path_buf()
    }
}

/// A class in a bundle's factory.
#[derive(Debug, Clone)]
pub struct Class {
    pub cid: [u8; 16],
    pub name: String,
    pub category: String,
    pub sub_categories: String,
    pub vendor: String,
    pub version: String,
}

impl Class {
    pub fn hex(&self) -> String {
        cid_hex(&self.cid)
    }
}

/// An open plug-in library. Dropping it calls the library's exit, but the
/// library itself is never unloaded: many plug-ins leave threads running,
/// and unloading under them crashes the process on the way out.
pub struct Module {
    pub factory: std::mem::ManuallyDrop<ComPtr<IPluginFactory>>,
    /// The maker from the factory's own info (for classes with none).
    pub vendor: String,
    #[cfg(windows)]
    lib: windows_sys::Win32::Foundation::HMODULE,
}

// The factory is only used from the host's main thread; the module is
// kept in an Arc there and in the audio half, which never calls it.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

pub(crate) fn cstr(b: &[i8]) -> String {
    let bytes: Vec<u8> = b
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

pub(crate) fn wstr(b: &[u16]) -> String {
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n]).trim().to_string()
}

impl Module {
    #[cfg(windows)]
    pub fn open(bundle: &Path) -> Result<Module, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{
            GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        let path = library_path(bundle);
        if !path.is_file() {
            return Err(format!("Its library was not found ({}).", path.display()));
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: loading a library runs its initialisers; that is what a
        // host does, and it happens in a process of its own.
        let lib = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        };
        if lib.is_null() {
            let err = std::io::Error::last_os_error();
            return Err(format!("Its library could not be loaded: {err}"));
        }
        unsafe {
            if let Some(init) = GetProcAddress(lib, c"InitDll".as_ptr().cast()) {
                let init: extern "system" fn() -> bool = std::mem::transmute(init);
                if !init() {
                    return Err("It refused to start (InitDll).".into());
                }
            }
            let Some(get) = GetProcAddress(lib, c"GetPluginFactory".as_ptr().cast()) else {
                return Err("It is not a VST3 plug-in (no GetPluginFactory).".into());
            };
            let get: extern "system" fn() -> *mut IPluginFactory = std::mem::transmute(get);
            let Some(factory) = ComPtr::from_raw(get()) else {
                return Err("It gave no plug-in factory.".into());
            };
            let mut info: PFactoryInfo = std::mem::zeroed();
            let vendor = if factory.getFactoryInfo(&mut info) == vst3::Steinberg::kResultOk {
                cstr(&info.vendor)
            } else {
                String::new()
            };
            Ok(Module {
                factory: std::mem::ManuallyDrop::new(factory),
                vendor,
                lib,
            })
        }
    }

    #[cfg(not(windows))]
    pub fn open(_bundle: &Path) -> Result<Module, String> {
        Err("Plug-ins are hosted on Windows for now.".into())
    }

    /// Every class the factory lists (the newer form when it has it).
    pub fn classes(&self) -> Vec<Class> {
        let mut out = Vec::new();
        unsafe {
            let f2 = self.factory.cast::<IPluginFactory2>();
            let n = self.factory.countClasses();
            for i in 0..n {
                let mut c = None;
                if let Some(f2) = &f2 {
                    let mut info: PClassInfo2 = std::mem::zeroed();
                    if f2.getClassInfo2(i, &mut info) == vst3::Steinberg::kResultOk {
                        c = Some(Class {
                            cid: info.cid.map(|b| b as u8),
                            name: cstr(&info.name),
                            category: cstr(&info.category),
                            sub_categories: cstr(&info.subCategories),
                            vendor: cstr(&info.vendor),
                            version: cstr(&info.version),
                        });
                    }
                }
                if c.is_none() {
                    let mut info: PClassInfo = std::mem::zeroed();
                    if self.factory.getClassInfo(i, &mut info) == vst3::Steinberg::kResultOk {
                        c = Some(Class {
                            cid: info.cid.map(|b| b as u8),
                            name: cstr(&info.name),
                            category: cstr(&info.category),
                            sub_categories: String::new(),
                            vendor: String::new(),
                            version: String::new(),
                        });
                    }
                }
                if let Some(mut c) = c {
                    if c.vendor.is_empty() {
                        c.vendor = self.vendor.clone();
                    }
                    out.push(c);
                }
            }
        }
        out
    }

    /// The plug-ins (audio modules) among the classes.
    pub fn audio_classes(&self) -> Vec<Class> {
        self.classes()
            .into_iter()
            .filter(|c| c.category == "Audio Module Class")
            .collect()
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // The factory goes first, then the library's exit call.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.factory) };
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
            if let Some(exit) = GetProcAddress(self.lib, c"ExitDll".as_ptr().cast()) {
                let exit: extern "system" fn() -> bool = std::mem::transmute(exit);
                exit();
            }
            // Never FreeLibrary (see the type's comment).
        }
    }
}
