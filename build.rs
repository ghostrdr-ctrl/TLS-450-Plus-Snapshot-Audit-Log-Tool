//! Stamps the Windows VERSIONINFO into the exe, so Properties > Details shows
//! the product, version and publisher. The version is read from Cargo.toml
//! (`CARGO_PKG_VERSION`), the one place it is set; the window title reads the
//! same value, so the two cannot disagree.
//!
//! Publisher and copyright name the software label, never a person.

const PRODUCT: &str = "TLS-450 Plus Snapshot Audit Log Tool";
const DESCRIPTION: &str = "TLS-450 Plus Snapshot Audit Log Tool";
const AUTHOR: &str = "Verbose Software";
const COPYRIGHT: &str = "Copyright (c) 2026 Verbose Software. All rights reserved.";
const EXE: &str = "TLS-450-Plus-Snapshot-Audit-Log-Tool.exe";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let nums: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().expect("version parts must be numbers"))
        .collect();
    let (ma, mi, pa) = (nums[0], nums[1], nums[2]);

    // The app's icon (Explorer, taskbar, Alt+Tab); drawn by tools/make_icon.py.
    println!("cargo:rerun-if-changed=assets/app.ico");
    let ico = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("app.ico");
    let ico = ico.display().to_string().replace('\\', "\\\\");

    let rc = format!(
        r#"#include <winver.h>
1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {ma},{mi},{pa},0
PRODUCTVERSION {ma},{mi},{pa},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE VFT2_UNKNOWN
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{AUTHOR}"
      VALUE "FileDescription", "{DESCRIPTION}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{EXE}"
      VALUE "LegalCopyright", "{COPYRIGHT}"
      VALUE "OriginalFilename", "{EXE}"
      VALUE "ProductName", "{PRODUCT}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("version.rc");
    std::fs::write(&out, rc).expect("write version.rc");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_required()
        .expect("compiling the version resource failed");
}
