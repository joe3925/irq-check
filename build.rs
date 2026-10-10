use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC");
    let rustc = std::env::var_os("RUSTC").expect("Cargo must supply RUSTC");
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("cannot read the build compiler identity");
    assert!(
        output.status.success(),
        "cannot read the build compiler identity"
    );
    let identity = String::from_utf8(output.stdout).expect("invalid compiler identity");
    for (field, variable) in [
        ("commit-hash: ", "IRQ_CHECK_BUILD_COMMIT"),
        ("host: ", "IRQ_CHECK_BUILD_HOST"),
        ("release: ", "IRQ_CHECK_BUILD_RELEASE"),
    ] {
        let value = identity
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .expect("incomplete compiler identity");
        assert!(
            !value.is_empty() && value != "unknown",
            "unknown compiler identity"
        );
        println!("cargo:rustc-env={variable}={value}");
    }
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(directory.join("compiler-identity.txt"), &identity)
        .expect("cannot save the build compiler identity");
    std::fs::write(
        directory.join("driver-identity.txt"),
        format!(
            "irq-check-driver build identity v1\npackage: {}\n{identity}end irq-check-driver build identity\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("cannot save the driver build identity");
}
