//! Gives the Windows executable its icon and product name. Other platforms
//! get theirs from packaging: the macOS app bundle and the Linux desktop
//! entry (packaging/).

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/swgoh-ae3.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("../../packaging/icons/swgoh-ae3.ico")
            .set("ProductName", "swgoh-ae3")
            .set("FileDescription", "swgoh-ae3")
            .set("InternalName", "ae3-gui")
            .set("OriginalFilename", "ae3-gui.exe");
        if let Err(e) = resource.compile() {
            panic!("embedding the Windows icon and version info failed: {e}");
        }
    }
}
