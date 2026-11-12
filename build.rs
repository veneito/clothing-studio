fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");
    // Windows: embed the app icon and version info in the .exe (Explorer, taskbar, shortcuts).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // icon 0 = the app, icon 1 = .clothing project files (installers point the file type at ",1")
        res.set_icon("assets/icon.ico")
            .set_icon_with_id("assets/file.ico", "2")
            .set("ProductName", "Clothing Studio")
            .set("FileDescription", "Clothing Studio");
        res.compile().expect("failed to embed Windows resources");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/file.ico");
}
