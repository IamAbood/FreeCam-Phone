//! Compiles softcam (MIT, git submodule) + our filter registration into a
//! static library; the cdylib exports the COM entry points via exports.def.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sc = root.join("softcam/src");
    let base = sc.join("baseclasses");
    let core = sc.join("softcamcore");
    if !core.exists() {
        panic!("softcam submodule missing: run `git submodule update --init`");
    }
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++17")
        .flag("/EHsc")
        .warnings(false)
        .define("WIN32", None)
        .define("NDEBUG", None)
        .define("UNICODE", None)
        .define("_UNICODE", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .include(&sc)
        .include(&base)
        .include(&core)
        .file(root.join("src/filter.cpp"))
        .file(root.join("src/sender.cpp"));
    for f in [
        "amfilter.cpp",
        "amvideo.cpp",
        "combase.cpp",
        "dllentry.cpp",
        "dllsetup.cpp",
        "mtype.cpp",
        "source.cpp",
        "winutil.cpp",
        "wxdebug.cpp",
        "wxlist.cpp",
        "wxutil.cpp",
    ] {
        b.file(base.join(f));
    }
    for f in [
        "DShowSoftcam.cpp",
        "FrameBuffer.cpp",
        "Misc.cpp",
        "SenderAPI.cpp",
        "Watchdog.cpp",
    ] {
        b.file(core.join(f));
    }
    b.compile("freecam_softcam");
    for lib in [
        "strmiids", "winmm", "ole32", "oleaut32", "user32", "advapi32", "uuid",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    println!(
        "cargo:rustc-cdylib-link-arg=/DEF:{}",
        root.join("exports.def").display()
    );
    // Resolve DllMain from filter.cpp before the CRT's default stub.
    println!("cargo:rustc-cdylib-link-arg=/INCLUDE:DllMain");
    println!("cargo:rerun-if-changed=src/filter.cpp");
    println!("cargo:rerun-if-changed=src/sender.cpp");
    println!("cargo:rerun-if-changed=exports.def");
}
