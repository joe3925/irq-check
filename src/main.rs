mod cargo;
mod policy;

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    let self_check = args.first().is_some_and(|arg| arg == "--self-check");
    if args.first().is_some_and(|arg| arg == "--interface") {
        println!("{}", policy::INTERFACE);
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|arg| arg == "--cargo") {
        return match cargo::launch(&args[1..]) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        };
    }
    if args.first().is_some_and(|arg| arg == "--version") {
        println!(
            "irq-check {} {} {} {} {}",
            env!("CARGO_PKG_VERSION"),
            env!("IRQ_CHECK_BUILD_RELEASE"),
            env!("IRQ_CHECK_BUILD_COMMIT"),
            env!("IRQ_CHECK_BUILD_HOST"),
            policy::INTERFACE
        );
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|arg| arg == "--self-check") {
        args = vec![
            env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
            "--version".into(),
        ];
    }
    let rustc = args.first().cloned().unwrap_or_else(|| "rustc".into());
    let identity = match Command::new(&rustc).arg("-vV").output() {
        Ok(output) if output.status.success() => output,
        _ => {
            eprintln!("error: cannot read the selected compiler identity");
            return ExitCode::from(2);
        }
    };
    let built = include_str!(concat!(env!("OUT_DIR"), "/compiler-identity.txt"));
    if String::from_utf8_lossy(&identity.stdout).trim() != built.trim() {
        eprintln!(
            "error: the selected compiler does not match the irq-check build compiler\nbuilt with:\n{built}selected:\n{}\nhelp: rebuild and install both checker binaries with the selected nightly: cargo install --locked --force --path <checker-checkout> --bins",
            String::from_utf8_lossy(&identity.stdout)
        );
        return ExitCode::from(2);
    }
    let output = match Command::new(&rustc).args(["--print", "sysroot"]).output() {
        Ok(output) if output.status.success() => output,
        _ => {
            eprintln!("error: cannot locate the compiler; use irq-check --cargo or RUSTC_WRAPPER");
            return ExitCode::from(2);
        }
    };
    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let executable = match env::current_exe() {
        Ok(path) => path.with_file_name(format!("irq-check-driver{}", env::consts::EXE_SUFFIX)),
        Err(error) => {
            eprintln!("error: cannot locate irq-check-driver: {error}");
            return ExitCode::from(2);
        }
    };
    let driver_identity = include_bytes!(concat!(env!("OUT_DIR"), "/driver-identity.txt"));
    match std::fs::read(&executable) {
        Ok(bytes)
            if bytes
                .windows(driver_identity.len())
                .any(|identity| identity == driver_identity) => {}
        Ok(_) => {
            eprintln!(
                "error: the driver file does not match the launcher build compiler; no driver was loaded\nhelp: install both binaries with the selected nightly: cargo install --locked --force --path <checker-checkout> --bins"
            );
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", executable.display());
            return ExitCode::from(2);
        }
    }
    let mut paths = vec![sysroot.join("bin"), sysroot.join("lib")];
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path));
    }
    let mut command = Command::new(executable);
    command.env("PATH", env::join_paths(paths).unwrap());
    for variable in ["CARGO_MAKEFLAGS", "MAKEFLAGS", "MFLAGS"] {
        command.env_remove(variable);
    }
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
        let mut paths = vec![sysroot.join("lib")];
        if let Some(path) = env::var_os(variable) {
            paths.extend(env::split_paths(&path));
        }
        command.env(variable, env::join_paths(paths).unwrap());
    }
    if self_check {
        return match command.arg("--version").output() {
            Ok(output) if output.status.success() => {
                let expected = format!(
                    "irq-check {} {} {} {}",
                    env!("CARGO_PKG_VERSION"),
                    env!("IRQ_CHECK_BUILD_RELEASE"),
                    env!("IRQ_CHECK_BUILD_COMMIT"),
                    policy::INTERFACE
                );
                if String::from_utf8_lossy(&output.stdout).trim() != expected {
                    eprintln!(
                        "error: the launcher and driver identities do not match; install both binaries from the same build"
                    );
                    ExitCode::from(2)
                } else {
                    println!("{expected}");
                    ExitCode::SUCCESS
                }
            }
            Ok(output) => {
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                ExitCode::from(output.status.code().unwrap_or(1) as u8)
            }
            Err(error) => {
                eprintln!("error: cannot load the interrupt checker driver: {error}");
                ExitCode::from(2)
            }
        };
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
                eprintln!("error: cannot start the compiler: {error}");
                ExitCode::from(2)
            }
        };
    }
    let mut analysis = args.clone();
    let mut original_search_paths = Vec::new();
    let mut cache_directories = Vec::new();
    let mut output_directory = None;
    if !passthrough {
        let mut index = 1;
        while index < analysis.len() {
            let value = analysis[index].to_string_lossy().into_owned();
            if value == "--out-dir" {
                let directory = PathBuf::from(&analysis[index + 1]).join("irq-check");
                if let Err(error) = std::fs::create_dir_all(&directory) {
                    eprintln!("error: cannot create {}: {error}", directory.display());
                    return ExitCode::from(2);
                }
                analysis[index + 1] = directory.into_os_string();
                output_directory = Some(PathBuf::from(&analysis[index + 1]));
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
                    cache_directories.push(PathBuf::from(directory).join("irq-check"));
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
                cache_directories.push(PathBuf::from(directory).join("irq-check"));
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
    let crate_name = args
        .windows(2)
        .find(|pair| pair[0] == "--crate-name")
        .map(|pair| pair[1].to_string_lossy().into_owned());
    let suffix = args.iter().find_map(|arg| {
        arg.to_str()
            .and_then(|arg| arg.strip_prefix("extra-filename="))
    });
    let metadata_file = output_directory
        .as_ref()
        .zip(crate_name)
        .map(|(directory, name)| {
            directory.join(format!("lib{name}{}.rmeta", suffix.unwrap_or("")))
        });
    let data_file = metadata_file
        .as_ref()
        .map(|path| path.with_extension("irq-data"));
    if let (Some(metadata), Some(data)) = (&metadata_file, &data_file) {
        command
            .env("IRQ_CHECK_METADATA_FILE", metadata)
            .env("IRQ_CHECK_DATA_FILE", data);
    }
    command.env(
        "IRQ_CHECK_CACHE_DIRS",
        serde_json::to_string(&cache_directories).unwrap(),
    );
    let argument_file = output_directory
        .as_ref()
        .map(|directory| directory.join(format!("irq-check-args-{}.json", std::process::id())));
    if let Some(path) = &argument_file {
        let arguments: Vec<_> = analysis
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        if let Err(error) = std::fs::write(path, serde_json::to_vec(&arguments).unwrap()) {
            eprintln!("error: cannot write driver arguments: {error}");
            return ExitCode::from(2);
        }
        command.arg("--irq-check-arguments").arg(path);
    } else {
        command.args(&analysis);
    }
    match command.status() {
        Ok(status) if !status.success() || passthrough => {
            ExitCode::from(status.code().unwrap_or(1) as u8)
        }
        Ok(_) => {
            if let (Some(metadata), Some(data)) = (&metadata_file, &data_file) {
                let result = std::fs::read(data)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        serde_json::from_slice::<policy::CrateData>(&bytes)
                            .map_err(|error| error.to_string())
                    })
                    .and_then(|mut record| {
                        record.metadata_hash = policy::hash(
                            std::fs::read(metadata).map_err(|error| error.to_string())?,
                        );
                        std::fs::write(
                            data,
                            serde_json::to_vec(&record).map_err(|error| error.to_string())?,
                        )
                        .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    eprintln!("error: cannot save interrupt check metadata: {error}");
                    return ExitCode::from(2);
                }
            }
            match Command::new(&rustc)
                .args(&args[1..])
                .arg("--check-cfg=cfg(irq_check)")
                .status()
            {
                Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
                Err(error) => {
                    eprintln!("error: cannot start the compiler: {error}");
                    ExitCode::from(2)
                }
            }
        }
        Err(error) => {
            eprintln!(
                "error: cannot start irq-check-driver: {error}\nhelp: install both binaries with cargo install --locked --path ."
            );
            ExitCode::from(2)
        }
    }
}
