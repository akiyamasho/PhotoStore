//! Windows only: embed the app icon and version info (VERSIONINFO) into `photostore-cli.exe`.
//!
//! On every other target this does nothing, even when `PHOTOSTORE_REQUIRE_WINRES` is set. A
//! missing resource compiler is a warning, so a cross-compile from macOS or Linux still links,
//! unless `PHOTOSTORE_REQUIRE_WINRES=1` (set by the release packaging script) turns it into a
//! build error.

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/photostore.ico");
    println!("cargo:rerun-if-env-changed=PHOTOSTORE_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let mut res = winresource::WindowsResource::new();
    // FileVersion / ProductVersion strings and the numeric FILEVERSION / PRODUCTVERSION are left
    // to `WindowsResource::new`, which reads the Cargo package version. A pre-release such as
    // `1.2.3-rc.1` stays in the strings; the numeric fields are `1.2.3.0`.
    res.set_icon("../../assets/app-icon/photostore.ico")
        .set("ProductName", "PhotoStore")
        .set("FileDescription", "PhotoStore command-line interface")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the PhotoStore authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "photostore-cli.exe")
        .set("InternalName", "photostore-cli");
    if let Err(err) = res.compile() {
        if std::env::var_os("PHOTOSTORE_REQUIRE_WINRES").is_some() {
            return Err(std::io::Error::other(format!(
                "embedding Windows resources into photostore-cli.exe failed: {err}. \
                 Install the Windows SDK resource compiler (rc.exe) or mingw-w64 windres and rebuild."
            )));
        }
        println!("cargo:warning=photostore-cli.exe built without icon/version resources: {err}");
    }
    Ok(())
}
