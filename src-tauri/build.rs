use std::path::PathBuf;
use std::process::Command;

fn newer_than(path: &std::path::Path, reference: &std::path::Path) -> bool {
    let Ok(path_modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return true;
    };
    let Ok(reference_modified) =
        std::fs::metadata(reference).and_then(|metadata| metadata.modified())
    else {
        return true;
    };
    path_modified > reference_modified
}

fn build_macos_subtitle_ocr() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest.join("native/subtitle_ocr.m");
    let build_script = manifest.join("build.rs");
    let binaries = manifest.join("binaries");
    let output = binaries.join(format!("subtitle-ocr-{target}"));
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", build_script.display());

    // `binaries/` is watched by `tauri dev`. Rewriting the sidecar during every
    // Cargo invocation makes Tauri observe a changed file and launch Cargo
    // again forever. Keep the existing executable unless one of its real build
    // inputs is newer.
    if output.is_file() && !newer_than(&source, &output) && !newer_than(&build_script, &output) {
        return;
    }

    std::fs::create_dir_all(&binaries).expect("create sidecar directory");
    let module_cache = manifest.join("target/clang-module-cache");
    std::fs::create_dir_all(&module_cache).expect("create clang module cache");
    let status = Command::new("/usr/bin/clang")
        .env("CLANG_MODULE_CACHE_PATH", &module_cache)
        .args(["-fobjc-arc", "-fblocks", "-O2"])
        .arg(&source)
        .args([
            "-framework",
            "Foundation",
            "-framework",
            "AVFoundation",
            "-framework",
            "Vision",
            "-framework",
            "CoreMedia",
            "-framework",
            "CoreGraphics",
            "-o",
        ])
        .arg(&output)
        .status()
        .expect("start Objective-C compiler for local subtitle OCR");
    assert!(
        status.success(),
        "failed to compile local subtitle OCR sidecar"
    );
}

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    build_macos_subtitle_ocr();
    tauri_build::build()
}
