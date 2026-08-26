use std::{env, fs, path::PathBuf};

fn main() {
    stage_codex_runtime();
    tauri_build::build()
}

fn stage_codex_runtime() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("target OS");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
    let (package, triple, executable) = match (target_os.as_str(), target_arch.as_str()) {
        ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin", "codex"),
        ("macos", "x86_64") => ("codex-darwin-x64", "x86_64-apple-darwin", "codex"),
        ("linux", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl", "codex"),
        ("linux", "x86_64") => ("codex-linux-x64", "x86_64-unknown-linux-musl", "codex"),
        ("windows", "aarch64") => ("codex-win32-arm64", "aarch64-pc-windows-msvc", "codex.exe"),
        ("windows", "x86_64") => ("codex-win32-x64", "x86_64-pc-windows-msvc", "codex.exe"),
        _ => panic!("unsupported Codex runtime target: {target_os}-{target_arch}"),
    };
    let source = manifest
        .join("..")
        .join("node_modules")
        .join("@openai")
        .join(package)
        .join("vendor")
        .join(triple)
        .join("bin")
        .join(executable);
    if !source.is_file() {
        panic!(
            "Codex runtime is missing at {}. Run bun install for the target platform.",
            source.display()
        );
    }
    let directory = manifest.join("resources").join("bin");
    fs::create_dir_all(&directory).expect("create generated resource directory");
    let destination = directory.join(executable);
    let already_staged = fs::metadata(&destination)
        .and_then(|destination| {
            fs::metadata(&source).map(|source| destination.len() == source.len())
        })
        .unwrap_or(false);
    if !already_staged {
        if destination.exists() {
            fs::remove_file(&destination).expect("replace generated Codex runtime");
        }
        if fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination).expect("copy generated Codex runtime");
        }
    }
    println!("cargo:rerun-if-changed={}", source.display());
}
