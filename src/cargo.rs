use crate::policy::{self, Package, Policy};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

pub fn launch(args: &[OsString]) -> Result<ExitCode, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(wrapper) = std::env::var_os("RUSTC_WRAPPER") {
        if !wrapper.is_empty() && Path::new(&wrapper) != executable {
            return Err("RUSTC_WRAPPER is already set to another program".into());
        }
    }
    let cargo = std::env::var_os("IRQ_CHECK_CARGO")
        .or_else(|| std::env::var_os("CARGO"))
        .unwrap_or_else(|| "cargo".into());
    let mut metadata = Command::new(&cargo);
    metadata.args(["metadata", "--format-version", "1"]);
    let mut configuration = Command::new(&cargo);
    configuration.args(["-Zunstable-options", "config", "get", "--format", "json"]);
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut package = None;
    let mut platform = None;
    let mut target_directory = None;
    let mut forwarded = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let start = index;
        let argument = args[index].to_string_lossy();
        let (name, inline) = argument
            .split_once('=')
            .map_or((argument.as_ref(), None), |(name, value)| {
                (name, Some(value))
            });
        if matches!(
            name,
            "--manifest-path"
                | "--target"
                | "--target-dir"
                | "--config"
                | "--features"
                | "-F"
                | "-p"
                | "--package"
        ) {
            let value = if let Some(value) = inline {
                value.to_owned()
            } else {
                index += 1;
                args.get(index)
                    .ok_or_else(|| format!("{name} needs a value"))?
                    .to_string_lossy()
                    .into_owned()
            };
            match name {
                "--manifest-path" => {
                    manifest = PathBuf::from(&value);
                    metadata.args([name, &value]);
                }
                "--target" => {
                    if platform.as_ref().is_some_and(|platform| platform != &value) {
                        return Err("select one target per interrupt check".into());
                    }
                    platform = Some(value);
                }
                "--target-dir" => {
                    target_directory = Some(PathBuf::from(value));
                }
                "--config" => {
                    metadata.args([name, &value]);
                    configuration.args([name, &value]);
                }
                "-p" | "--package" => {
                    if package.as_ref().is_some_and(|package| package != &value) {
                        return Err("select one root package per interrupt check".into());
                    }
                    package = Some(value);
                }
                _ => {
                    metadata.args(["--features", &value]);
                }
            }
        } else if matches!(name, "--workspace" | "--all" | "--exclude") {
            return Err("select one root package per interrupt check with -p".into());
        } else if matches!(
            name,
            "--offline" | "--locked" | "--frozen" | "--all-features" | "--no-default-features"
        ) {
            metadata.arg(name);
        } else if name.starts_with("-Z") {
            metadata.arg(&args[index]);
            configuration.arg(&args[index]);
            if name == "-Z" {
                index += 1;
                metadata.arg(args.get(index).ok_or("-Z needs a value")?);
                configuration.arg(&args[index]);
            }
        }
        if name != "--target-dir" {
            forwarded.extend_from_slice(&args[start..=index]);
        }
        index += 1;
    }
    if platform.is_none() {
        let output = configuration
            .output()
            .map_err(|error| format!("cannot read the Cargo target configuration: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot read the Cargo target configuration:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let configuration: Value =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        match &configuration["build"]["target"] {
            Value::String(target) => platform = Some(target.clone()),
            Value::Array(targets) if targets.len() == 1 && targets[0].is_string() => {
                platform = targets[0].as_str().map(str::to_owned);
            }
            Value::Null => {}
            _ => return Err("select one target per interrupt check with --target".into()),
        }
        if platform.is_none() {
            let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| {
                configuration["build"]["rustc"]
                    .as_str()
                    .unwrap_or("rustc")
                    .into()
            });
            let output = Command::new(compiler)
                .arg("-vV")
                .output()
                .map_err(|error| format!("cannot read the compiler host target: {error}"))?;
            if output.status.success() {
                platform = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix("host: ").map(str::to_owned));
            }
            if platform.is_none() {
                return Err("cannot read the compiler host target".into());
            }
        }
    }
    metadata.args(["--filter-platform", platform.as_deref().unwrap()]);
    let output = metadata
        .output()
        .map_err(|error| format!("cannot read the Cargo dependency graph: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read the Cargo dependency graph:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let graph: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let packages: HashMap<_, _> = graph["packages"]
        .as_array()
        .ok_or("Cargo metadata has no packages")?
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let nodes: HashMap<_, _> = graph["resolve"]["nodes"]
        .as_array()
        .ok_or("Cargo metadata has no resolved dependency graph")?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let manifest = std::fs::canonicalize(manifest).map_err(|error| error.to_string())?;
    let roots: Vec<_> = packages
        .iter()
        .filter(|(id, data)| {
            if let Some(package) = &package {
                **id == package || data["name"].as_str() == Some(package)
            } else {
                data["manifest_path"]
                    .as_str()
                    .and_then(|path| std::fs::canonicalize(path).ok())
                    .is_some_and(|path| path == manifest)
            }
        })
        .map(|(id, _)| *id)
        .collect();
    if roots.len() != 1 {
        return Err("select one root package with --manifest-path and -p".into());
    }
    let mut selected = HashSet::new();
    let mut policy = Policy {
        root: roots[0].into(),
        ..Policy::default()
    };
    let mut pending = VecDeque::from(roots);
    while let Some(id) = pending.pop_front() {
        if !selected.insert(id) {
            continue;
        }
        let package = packages
            .get(id)
            .ok_or("selected package is missing from Cargo metadata")?;
        let manifest = PathBuf::from(
            package["manifest_path"]
                .as_str()
                .ok_or("package has no manifest")?,
        );
        let manifest = std::fs::canonicalize(manifest).map_err(|error| error.to_string())?;
        let contents = std::fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
        let contents: toml::Value = toml::from_str(&contents).map_err(|error| error.to_string())?;
        policy.packages.push(Package {
            id: id.into(),
            manifest,
        });
        let Some(dependencies) = nodes.get(id).and_then(|node| node["deps"].as_array()) else {
            continue;
        };
        for dependency in dependencies {
            let alias = dependency["name"]
                .as_str()
                .ok_or("dependency has no name")?;
            let target_id = dependency["pkg"]
                .as_str()
                .ok_or("dependency has no resolved package")?;
            for kind in dependency["dep_kinds"]
                .as_array()
                .ok_or("dependency has no kind")?
            {
                if !kind["kind"].is_null() {
                    continue;
                }
                let table = if let Some(target) = kind["target"].as_str() {
                    contents
                        .get("target")
                        .and_then(|targets| targets.get(target))
                } else {
                    Some(&contents)
                };
                let flag = table
                    .and_then(|table| table.get("dependencies"))
                    .and_then(toml::Value::as_table)
                    .and_then(|table| {
                        table
                            .iter()
                            .find(|(name, _)| name.replace('-', "_") == alias)
                    })
                    .and_then(|(_, dependency)| dependency.get("irq-check"));
                if let Some(flag) = flag {
                    match flag.as_bool() {
                        Some(true) => pending.push_back(target_id),
                        Some(false) => {}
                        None => {
                            return Err(format!(
                                "{id}: dependencies.{alias}.irq-check must be true or false"
                            ));
                        }
                    }
                }
            }
        }
    }
    policy
        .packages
        .sort_by(|left, right| left.id.cmp(&right.id));
    let policy = serde_json::to_string(&policy).map_err(|error| error.to_string())?;
    let driver =
        executable.with_file_name(format!("irq-check-driver{}", std::env::consts::EXE_SUFFIX));
    let fingerprint = format!(
        "{:016x}",
        policy::hash((
            policy::INTERFACE,
            &policy,
            std::fs::read(&executable).map_err(|error| error.to_string())?,
            std::fs::read(driver).map_err(|error| error.to_string())?
        ))
    );
    let target = target_directory
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(graph["target_directory"].as_str().unwrap_or("target")))
        .join("irq-check-policy")
        .join(&fingerprint);
    let mut command = Command::new(cargo);
    command
        .args(forwarded)
        .env("RUSTC_WRAPPER", executable)
        .env("IRQ_CHECK_POLICY", policy)
        .env("IRQ_CHECK_POLICY_HASH", fingerprint)
        .env("CARGO_TARGET_DIR", target)
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start Cargo: {error}"))?;
    let stderr = child.stderr.take().ok_or("cannot read Cargo errors")?;
    let forwarding = std::thread::spawn(move || -> std::io::Result<()> {
        let mut stderr_output = std::io::stderr().lock();
        for line in BufReader::new(stderr).split(b'\n') {
            let line = line?;
            let plain = String::from_utf8_lossy(&line);
            if plain
                .split_once("unused manifest key:")
                .is_some_and(|(_, key)| {
                    key.split('\u{1b}')
                        .next()
                        .unwrap_or(key)
                        .trim_end()
                        .ends_with(".irq-check")
                })
            {
                continue;
            }
            stderr_output.write_all(&line)?;
            stderr_output.write_all(b"\n")?;
            stderr_output.flush()?;
        }
        Ok(())
    });
    let status = child.wait().map_err(|error| error.to_string())?;
    forwarding
        .join()
        .map_err(|_| "cannot forward Cargo errors")?
        .map_err(|error| error.to_string())?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}
