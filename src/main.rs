use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--version") {
        println!(
            "irq-check {} nightly-2026-04-07 bcded331651b60a0383b3ff51db4f24c4495ac53",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|arg| arg == "--self-check") {
        match Command::new("rustup")
            .args(["which", "--toolchain", "nightly-2026-04-07", "rustc"])
            .output()
        {
            Ok(output) if output.status.success() => {
                args = vec![
                    String::from_utf8_lossy(&output.stdout).trim().into(),
                    "--version".into(),
                ];
            }
            _ => {
                eprintln!(
                    "Install nightly-2026-04-07 with rustc-dev, rust-src, and llvm-tools-preview."
                );
                return ExitCode::from(2);
            }
        }
    }
    let rustc = args.first().cloned().unwrap_or_else(|| "rustc".into());
    let output = match Command::new(&rustc).args(["--print", "sysroot"]).output() {
        Ok(output) if output.status.success() => output,
        _ => {
            eprintln!("Cannot locate the compiler. Use irq-check as RUSTC_WRAPPER.");
            return ExitCode::from(2);
        }
    };
    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let executable = match env::current_exe() {
        Ok(path) => path.with_file_name(format!("irq-check-driver{}", env::consts::EXE_SUFFIX)),
        Err(error) => {
            eprintln!("Cannot locate irq-check-driver: {error}");
            return ExitCode::from(2);
        }
    };
    let mut paths = vec![sysroot.join("bin"), sysroot.join("lib")];
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path));
    }
    let mut command = Command::new(executable);
    command.env("PATH", env::join_paths(paths).unwrap());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
        let mut paths = vec![sysroot.join("lib")];
        if let Some(path) = env::var_os(variable) {
            paths.extend(env::split_paths(&path));
        }
        command.env(variable, env::join_paths(paths).unwrap());
    }
    let passthrough = args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--version"
            || arg == "-V"
            || arg == "-vV"
            || arg == "-Vv"
            || arg.starts_with("--print")
            || arg == "proc-macro"
            || arg == "build_script_build"
    });
    if args
        .iter()
        .any(|arg| arg == "proc-macro" || arg == "build_script_build")
    {
        return match Command::new(&rustc)
            .args(&args[1..])
            .arg("--check-cfg=cfg(irq_check)")
            .status()
        {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(error) => {
                eprintln!("Cannot start the compiler: {error}");
                ExitCode::from(2)
            }
        };
    }
    let mut analysis = args.clone();
    let mut original_search_paths = Vec::new();
    if !passthrough {
        let mut index = 1;
        while index < analysis.len() {
            let value = analysis[index].to_string_lossy().into_owned();
            if value == "--out-dir" {
                let directory = PathBuf::from(&analysis[index + 1]).join("irq-check");
                if let Err(error) = std::fs::create_dir_all(&directory) {
                    eprintln!("Cannot create {}: {error}", directory.display());
                    return ExitCode::from(2);
                }
                analysis[index + 1] = directory.into_os_string();
                index += 2;
                continue;
            }
            if value == "--emit" {
                analysis[index + 1] = "metadata".into();
                index += 2;
                continue;
            }
            if value.starts_with("--emit=") {
                analysis[index] = "--emit=metadata".into();
            }
            if value == "-C"
                && analysis
                    .get(index + 1)
                    .is_some_and(|arg| arg.to_string_lossy().starts_with("incremental="))
            {
                analysis.drain(index..index + 2);
                continue;
            }
            if value.starts_with("-Cincremental=") {
                analysis.remove(index);
                continue;
            }
            if value == "--extern" {
                let external = analysis[index + 1].to_string_lossy();
                if let Some((name, file)) = external.split_once('=') {
                    let file = PathBuf::from(file);
                    if matches!(
                        file.extension().and_then(|extension| extension.to_str()),
                        Some("rlib" | "rmeta")
                    ) {
                        if let (Some(parent), Some(filename)) = (file.parent(), file.file_name()) {
                            let shadow = parent
                                .join("irq-check")
                                .join(filename)
                                .with_extension("rmeta");
                            if shadow.is_file() {
                                analysis[index + 1] = format!("{name}={}", shadow.display()).into();
                            }
                        }
                    }
                }
                index += 2;
                continue;
            }
            if value == "-L" {
                let search = analysis[index + 1].to_string_lossy();
                if let Some(directory) = search.strip_prefix("dependency=") {
                    original_search_paths.push(format!("-Ldependency={directory}"));
                    analysis[index + 1] = format!(
                        "dependency={}",
                        PathBuf::from(directory).join("irq-check").display()
                    )
                    .into();
                }
                index += 2;
                continue;
            }
            if let Some(directory) = value.strip_prefix("-Ldependency=") {
                original_search_paths.push(format!("-Ldependency={directory}"));
                analysis[index] = format!(
                    "-Ldependency={}",
                    PathBuf::from(directory).join("irq-check").display()
                )
                .into();
            }
            if let Some(json) = value.strip_prefix("--json=") {
                analysis[index] = format!(
                    "--json={}",
                    json.split(',')
                        .filter(|part| *part != "artifacts")
                        .collect::<Vec<_>>()
                        .join(",")
                )
                .into();
            }
            index += 1;
        }
        analysis.extend(original_search_paths.into_iter().map(Into::into));
        analysis.push("--cap-lints=allow".into());
    }
    command.args(&analysis);
    match command.status() {
        Ok(status) if !status.success() || passthrough => {
            ExitCode::from(status.code().unwrap_or(1) as u8)
        }
        Ok(_) => {
            match Command::new(&rustc)
                .args(&args[1..])
                .arg("--check-cfg=cfg(irq_check)")
                .status()
            {
                Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
                Err(error) => {
                    eprintln!("Cannot start the compiler: {error}");
                    ExitCode::from(2)
                }
            }
        }
        Err(error) => {
            eprintln!(
                "Cannot start irq-check-driver: {error}. Install both binaries with cargo install --locked --path ."
            );
            ExitCode::from(2)
        }
    }
}
