fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // The private CoreGraphics and SkyLight shims, shared with the Swift app.
    cc::Build::new()
        .file("native/SkyLight.m")
        .include("native")
        .flag("-fobjc-arc")
        .compile("agent_screen_private");
    println!("cargo:rerun-if-changed=native");
    for framework in [
        "ApplicationServices",
        "CoreGraphics",
        "AppKit",
        "ScreenCaptureKit",
        "ImageIO",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    // screencapturekit's Swift bridge links the system Swift runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
