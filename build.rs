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

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
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
    }
}
