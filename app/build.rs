// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Builds the icon (`assets/icon/audiform2.ico`) and version text into
//! audiform27.exe on Windows. `embed-resource` finds the Windows SDK's
//! rc.exe through the registry, so it need not be on PATH.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/icon/audiform2.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
    }
}

#[cfg(windows)]
fn windows_resource() {
    use std::path::PathBuf;
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = dir.join("../assets/icon/audiform2.ico");
    let ico = ico.canonicalize().unwrap_or(ico);
    // rc.exe takes forward slashes; drop the verbatim prefix canonicalize adds.
    let ico = ico.to_string_lossy().replace('\\', "/");
    let ico = ico.trim_start_matches("//?/");
    let v = |k: &str| std::env::var(k).unwrap_or_else(|_| "0".into());
    let (a, b, c) = (
        v("CARGO_PKG_VERSION_MAJOR"),
        v("CARGO_PKG_VERSION_MINOR"),
        v("CARGO_PKG_VERSION_PATCH"),
    );
    let rc = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "AudiForm 2"
      VALUE "ProductName", "AudiForm 2"
      VALUE "InternalName", "audiform27"
      VALUE "OriginalFilename", "audiform27.exe"
      VALUE "FileVersion", "{a}.{b}.{c}"
      VALUE "ProductVersion", "{a}.{b}.{c}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("audiform27.rc");
    std::fs::write(&out, rc).expect("write audiform27.rc");
    if let Err(e) = embed_resource::compile(&out, embed_resource::NONE).manifest_optional() {
        // A build without the icon still works; say why it has none.
        println!("cargo:warning=no exe icon: {e}");
    }
}
