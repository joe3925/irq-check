# irq-check

## Install

```text
git clone https://github.com/joe3925/irq-check.git
cd irq-check
rustup toolchain install nightly-2026-04-07 --component rustc-dev --component rust-src --component llvm-tools-preview
cargo +nightly-2026-04-07 install --locked --path .
```

Add the Cargo bin directory to `PATH`. The rustOS `xtask` uses the installed checker.

## Mark functions

```rust
#[cfg_attr(irq_check, irq::context)]
fn interrupt_handler() {}

#[cfg_attr(irq_check, irq::forbidden)]
fn heap_operation() {}

trait Device {
    #[cfg_attr(irq_check, irq::context)]
    fn service(&self);
}
```

Mark interrupt entry functions, not function-pointer fields. A marked trait method checks its concrete implementations. Mark the trait to check all its methods. Helpers and callbacks need no context mark when the checker can find their call targets.

The checker follows typed function pointers and `dyn` values through arguments, return values, fields, arrays, closures, and static storage. It includes initialization code. If it cannot find all call targets, it emits a Rust error. It does not emit coverage warnings or write a findings report.

## Allow an unchecked operation

```rust
#[cfg_attr(irq_check, irq::context)]
fn interrupt(address: u64) {
    #[cfg_attr(irq_check, irq::trusted(unsafe))]
    unsafe {
        let callback: unsafe extern "C" fn() = core::mem::transmute(address);
        callback();
    }
}
```

Use this mark only after you check the unknown operation. The block must be an explicit unsafe block. A function mark, a safe block, and a bare `irq::trusted` mark are errors. Rust does not accept `unsafe(irq::trusted)`.

Trust allows unknown operations in this block only. It does not apply to a called helper, a nested closure, or an async body. It does not pass to a stored or returned value. A later unknown call needs its own trusted unsafe block. Known forbidden calls remain errors.

Assembly and external calls without a known Rust body need this mark on a checked interrupt path. For `naked_asm!`, put the mark on the unsafe block inside the naked function. For `global_asm!`, put the mark at the Rust call to the external symbol.

The checker treats the compiler's `core` entries for x86 `rdtsc`, x86 `pause`, and AArch64 `isb` as known leaf operations. This rule does not trust other LLVM symbols or external functions with the same name in another crate.

## Exclude a fatal error path

```rust
if invalid_state {
    #[cfg_attr(irq_check, irq::unreachable(unsafe))]
    unsafe {
        panic!("invalid state");
    }
}
```

This mark asserts that the block cannot run in interrupt context. The block must be an explicit unsafe block and must not return. Use it only on a fatal error path whose condition you have checked. The checker cannot prove this assertion.

The checker does not follow interrupt calls from the marked block. This includes known forbidden calls. Normal Rust compilation and runtime behavior do not change. The mark does not call `unreachable_unchecked`, add undefined behavior, or mark a called helper as unreachable from other call sites. A nested closure or async body does not inherit the mark.

## Select dependencies

```toml
[dependencies]
kernel_sync = { path = "../libs/kernel_sync", irq-check = true }
kernel_executor = { path = "../libs/kernel_executor", irq-check = true }
x86_64 = "=0.15.2"
```

Run Cargo through the checker:

```text
irq-check --cargo check --manifest-path kernel/Cargo.toml -p kernel
```

The rustOS build tool uses this command form. The root package is checked. A dependency is checked only when its active dependency entry has `irq-check = true`. An absent or false flag excludes it. A selected dependency can select its own dependencies. Selection does not include other dependencies by default.

Selection uses Cargo's resolved package identity. It respects dependency renames, versions, sources, features, and target conditions. Put a workspace dependency flag on the member's dependency entry. A flag in `[workspace.dependencies]` is not inherited. Each root build has a separate policy.

An excluded dependency ends the checked call path. Its body, callbacks, assembly, and further calls are not checked. A directly marked forbidden entry is still an error. A value returned from an excluded dependency has no trusted call target. A later unknown call in checked code is an error. `core`, `alloc`, and `std` keep their built-in checks.

The Cargo launcher removes only the unused-key warning for this dependency flag. Other Cargo and Rust warnings remain. Internal metadata stores block scopes and package selection. It is not a findings report. The build cache changes when the policy or checker binaries change.
