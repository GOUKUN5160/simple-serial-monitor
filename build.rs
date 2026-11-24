use std::path::PathBuf;
use std::process::Command;

fn find_rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(&kits) {
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        versions.reverse();
        for v in versions {
            let cand = v.join("x64").join("rc.exe");
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=src/ui/macos/bridge.h");
    println!("cargo:rerun-if-changed=src/ui/macos/bridge.m");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        // Embed Windows application manifest (Common Controls v6 + PerMonitorV2 DPI) into ssm.exe
        println!("cargo:rustc-link-arg-bin=ssm=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=ssm=/MANIFESTINPUT:app.manifest");

        // Compile and embed application icon resource (IDI = 1) into ssm.exe
        if let Some(rc) = find_rc_exe() {
            let _ = std::fs::create_dir_all("target");
            let status = Command::new(rc)
                .args(["/nologo", "/fo", "target/app.res", "app.rc"])
                .status();
            if let Ok(s) = status {
                if s.success() {
                    println!("cargo:rustc-link-arg-bin=ssm=target/app.res");
                }
            }
        }

        if std::env::var("PROFILE").as_deref() == Ok("release") {
            println!("cargo:rustc-link-arg=/OPT:REF");
            println!("cargo:rustc-link-arg=/OPT:ICF");
            println!("cargo:rustc-link-arg=/DEBUG:NONE");
        }
    } else if target_os == "macos" {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
        let obj_path = out_dir.join("ssm_macos_ui.o");
        let lib_path = out_dir.join("libssm_macos_ui.a");

        let mut clang = Command::new("clang");
        clang.args([
            "-c",
            "-fobjc-arc",
            "-Oz",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "-mmacosx-version-min=11.0",
            "-I",
            "src/ui/macos",
            "src/ui/macos/bridge.m",
            "-o",
        ]);
        clang.arg(&obj_path);

        if let Ok(arch) = std::env::var("CARGO_CFG_TARGET_ARCH") {
            let clang_arch = match arch.as_str() {
                "aarch64" => "arm64",
                "x86_64" => "x86_64",
                other => other,
            };
            clang.args(["-arch", clang_arch]);
        }

        let status = clang.status().expect("failed to invoke clang for macOS UI");
        assert!(status.success(), "clang failed to compile src/ui/macos/bridge.m");

        let ar_status = Command::new("ar")
            .arg("rcs")
            .arg(&lib_path)
            .arg(&obj_path)
            .status()
            .expect("failed to invoke ar");
        assert!(ar_status.success(), "ar failed to archive libssm_macos_ui.a");

        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=ssm_macos_ui");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
