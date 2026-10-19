use std::path::PathBuf;

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

fn windows_resources() {
    println!("cargo:rerun-if-changed=resources/windows.rc");
    println!("cargo:rerun-if-changed=resources/wallhaven-studio.ico");
    println!("cargo:rerun-if-env-changed=RC");
    let compiler = std::env::var_os("RC")
        .map(PathBuf::from)
        .or_else(|| {
            let root = std::env::var_os("ProgramFiles(x86)")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
                .join(r"Windows Kits\10\bin");
            let mut versions = std::fs::read_dir(root)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            versions.sort();
            versions.reverse();
            versions
                .into_iter()
                .map(|path| path.join("x64/rc.exe"))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("rc.exe"));
    let output =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("wallhaven-studio.res");
    let status = std::process::Command::new(compiler)
        .current_dir("resources")
        .arg("/nologo")
        .arg("/fo")
        .arg(&output)
        .arg("windows.rc")
        .status()
        .expect("Windows SDK resource compiler rc.exe is required (or set RC)");
    assert!(status.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg-bin=wh-desktop={}", output.display());
}
