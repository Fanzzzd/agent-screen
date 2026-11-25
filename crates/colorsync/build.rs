fn main() {
    // rustc writes MACOSX_DEPLOYMENT_TARGET into the binary's minos but
    // doesn't tell cargo, which would keep a binary linked for another
    // target (check-deployment-target.sh).
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
}
