# irq-check

## Install

```text
git clone https://github.com/joe3925/irq-check.git
cd irq-check
rustup component add --toolchain nightly rustc-dev rust-src llvm-tools-preview
cargo +nightly install --locked --force --path . --bins
```

Add the Cargo bin directory to `PATH`. The rustOS `xtask` uses the installed checker.

Use the same installed nightly for the checker and the checked project. If you select another installed nightly, use its name in both commands. The checker records its build compiler identity. The launcher checks the selected compiler and the identity stored in the driver file before it loads the driver. A driver file without matching identity data is rejected. After a compiler update, rebuild and install both binaries. Compiler API changes can also require source changes.

Run `irq-check --version` to read the build identity without loading the driver. Run `irq-check --self-check` to check the selected compiler and load the driver.

irq-check is licensed under GPL-3.0-only. See LICENSE and NOTICE.

## Check the installed build

Run the five paired Cargo scenarios from this checkout:

```powershell
.\scenarios\run.ps1 -Toolchain nightly
```

The runner uses the installed checker. It checks the exit status, compiler identity, target, selected crates, expected forbidden target, and an indirect call reachable from the IRQ root. It uses a new Cargo output directory for each run under `target/scenarios`. A compiler error or a checker crash does not count as an expected rejection. Add `-VerifyCache` to repeat each safe build and check that Cargo reuses its result without changing the analysis evidence.

Set `IRQ_CHECK_STATS=1` to print analysis counters. Set `IRQ_CHECK_DUMP_DIR` to a directory to save the context graph, call-site states, storage bindings, and backend coverage gaps. The `<crate>-backend-coverage.txt` file records the function, MIR location, and reason for each backend gap. A backend gap is not an IRQ verdict; flow refinement must cover the operation or report incomplete coverage. Set `IRQ_CHECK_EXPLAIN=1` to include states in failure diagnostics. These settings do not change the policy.

To force fresh analysis through Cargo or xtask, set a new process-local cache ID before the build:

```powershell
$previousAnalysisId = $env:IRQ_CHECK_ANALYSIS_ID
$previousToolchain = $env:RUSTUP_TOOLCHAIN
try {
    $env:RUSTUP_TOOLCHAIN = 'nightly'
    $env:IRQ_CHECK_ANALYSIS_ID = [guid]::NewGuid().ToString()
    cargo +nightly run -p xtask -- build --platform x86_64-uefi --release
    if ($LASTEXITCODE -ne 0) { throw 'Kernel build failed' }
} finally {
    $env:IRQ_CHECK_ANALYSIS_ID = $previousAnalysisId
    $env:RUSTUP_TOOLCHAIN = $previousToolchain
}
```

Run this example from the rustOS checkout. Keep the same ID to check cache reuse. A new ID selects a new Cargo output directory. It does not delete existing output or change the analysis policy.

## Backend status

The five paired Cargo scenarios and their cache checks passed with the current installed build. This build includes the changes described below. The logs are in `target/scenarios/20261009-202845-6867585`. The driver is in `target/rupta-runtime-links-install/bin`. Its SHA-256 is `9E519387D5857C94DAC10608477BB23F7099F54A9E2EDDAB521EB1383A10E037`. The rewrite is not accepted. The remaining semantic and backend work, and fresh rustOS xtask acceptance, are still required.

The production pipeline now uses Rupta for initial call-target discovery. Its candidates feed unresolved calls in the in-tree flow refinement. A candidate does not clear an unknown remainder. Known callbacks and object references in typed locals, parameters, statics, and modeled heap objects feed back as Rupta address constraints. Shared storage identities connect these constraints to MIR paths. New call contexts receive the path connections too. The bridge uses compiler identities and typed field, variant, and array paths. It merges discovery contexts only; the flow solver keeps the precise context and state bindings. General allocation and unknown-memory feedback remains incomplete. This is an intermediate integration state, not the finished backend. See NOTICE for the source revision and license.

Shared opaque effects use a changed-storage set. Once an effect includes unknown shared storage, the checker does not repeat the full walk of unchanged escaped objects. New escapes, changed storage, allocation aliases, and local state overrides still need a read. The unknown write and all possible known callback targets remain in the effect. A storage change schedules the shared-effect consumers again. This reduces repeated work; it does not exclude opaque calls from policy checks.

The flow solver can summarize a MIR return projection through local copies and moves. It computes the return value from each caller's arguments and keeps the concrete call edge. This also covers raw-pointer mutability changes and a one-field transparent pointer wrapper when the pointee type stays the same and the target layout permits the conversion. The summary uses the normal pointer-view conversion. Calls, dereferences, branches, memory writes, other casts, and IRQ roots do not use this summary. Unsupported bodies keep the full analysis. The graph records each use of the summary.

The refined storage state now uses the ported Rupta delta store and hybrid points-to sets. The adapter interns storage and fact identities. Flow-sensitive states still control strong updates and branch alternatives.

The backend stores complete call bindings separately from graph edges. A later binding with different argument paths or a different return destination still adds constraints when its graph edge already exists. Opt-in statistics include the binding count. Formatting retained 3721 bindings and passed in 46.02 seconds including compilation. This count includes different callees and contexts; it does not prove that this fixture exercised the prior binding loss.

Block-state lookup no longer copies the complete state before it checks for an existing entry. Widening does not join the previous result back into itself. Diagnostic state snapshots are taken only for operations that can produce a call edge. These changes retain the input state for each recorded edge. The latest formatting build took 43.98 seconds including compilation, compared with 49.05 seconds for the previous build. This is one comparison under system load, not a kernel speed estimate. The active kernel build still uses an earlier checker and has no policy verdict.

Argument binding shares an unchanged tree when its facts already meet the scalar-normalization rules. A changed storage binding or a required normalization still rebuilds the tree. Context lookup borrows an existing key and copies it only for a new entry. Backend feedback is flushed when the flow queue is empty or when 4096 facts are pending. This threshold controls batches; it does not limit analysis coverage or discard facts.

Opaque calls use one callee identity without a separate argument or memory context. Their writes, callbacks, return uncertainty, and unwind state remain in each caller's state. The call-site evidence and forbidden-target checks remain separate. The formatting build passed in 43.77 seconds; this is close to the previous 43.98 seconds and does not show a material speed change.

Return-value summaries now forward the analyzed callee's memory effects before they assign the return value. Calls and drops use the same effect transfer. The `RawVec` no-op summaries and the old-pointer reallocation shortcut were removed. These operations use MIR or conservative opaque effects instead.

The linked-symbol map excludes foreign declarations, so an import cannot replace a Rust definition with the same symbol. Default allocator shims use the selected compiler's allocator-kind queries and internal-symbol mangling. Their targets must be concrete Rust definitions. The default allocator-error link follows the Rust handler and its panic path. Missing targets remain unresolved; this mapping is not an allocator exemption. The generated allocator-error link still needs confirmation in a fresh kernel run. The current formatting build passed in 44.05 seconds including compilation.

The installed-driver cache check is in `target/driver-identity-proof-20261009-2012`. The previous and current drivers each built the same mixed-dispatch fixture with exit code 0 and fresh root evidence. They selected cache directories `7f208015242d99c2` and `84a4314f25c43257`. Repeated builds returned 0 and left each proof unchanged. A temporary source change selected the existing bad receiver and returned 101 for `Bad::call`. Restoring the source returned 0 with fresh evidence. The restored source SHA-256 is `B0E915ED722C454A8D4840A371B9E8B2A4B90BE9F27D3E1282D989D2B00DC414`. No scenario was added.

The Rupta library in `vendor/rupta` now compiles with the installed nightly. It includes the graph builder, delta propagator, indirect-call discovery, and context strategies. The port removes the standalone compiler launcher and Linux process monitor. It accepts concrete discovery roots and keeps compiler instance kinds distinct. Generic constants are no longer replaced with `1`. Missing bodies and unsupported new aggregates have explicit coverage records.

Production integration is still in progress. A library compile does not establish semantic coverage or kernel acceptance. Check the port with `cargo +nightly check --manifest-path vendor/rupta/Cargo.toml --locked`.

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
