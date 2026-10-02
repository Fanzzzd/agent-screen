fn main() {
    // screencapturekit's Swift bridge links the system Swift runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
