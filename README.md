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

Normal analysis uses Rust compiler diagnostics. Optional reports use the following settings. The driver reads them once per process. They do not change the policy or select another backend.

| Setting | Output |
| --- | --- |
| `IRQ_CHECK_STATS=1` | Analysis counters on standard error |
| `IRQ_CHECK_DUMP_DIR=<directory>` | IRQ graph, query evidence, separate candidate edges, accepted gaps, and backend coverage files |
| `IRQ_CHECK_EXPLAIN=1` | Query evidence and internal site details in failure diagnostics |

The `<crate>-backend-coverage.txt` file records the function, MIR location, and reason for each backend gap. A backend gap is not an IRQ verdict. The pointer query must establish the binding or report an unknown call. Internal `IRQ_CHECK_TRACE*` and `IRQ_CHECK_RUPTA_POINTS` settings have been removed. Reports no longer write phase or per-worker progress files.

The launcher CLI, `--interface` result, policy metadata, compiler-identity checks, and existing evidence fields retain their current interface. Rupta remains the pointer-analysis backend. See [saved analysis findings](TARGET_FINDINGS.md) for the investigation results preserved before build cleanup.

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

## Solver

Rupta builds one global pointer graph from initialization, exported functions, and interrupt roots. RCEUS runs with context depth zero. Stack filtering and cast pruning are disabled. The graph supplies call candidates. A candidate is not a proved IRQ binding, even when there is only one candidate.

The IRQ walker starts at marked roots. A pointer query reads only the definitions and memory effects needed for the requested object, byte offset, type, and call site. Call frames retain argument origins. They do not retain full memory snapshots. Branch and array-index choices stay with each result. Large bounded index sets use separate cases under one transfer budget.

Call-site reachability uses shared control-flow summaries. Each block retains at most eight structural alternatives, subject to the configured alternative limit. Exact branch simplification runs before widening. If paths exceed the bound, the summary keeps specific paths and one conservative remainder. Loop backedges do not add iteration histories to these summaries. Loop-entry and incoming-edge relations remain available to the memory solver.

Memory path conditions use the same bounded structural builder. They do not use a separate path-history cache. The cache distinguishes a memory path from a control-return request. Memory SSA supplies iteration selection for memory reads; the structural template must not add a second iteration-zero constraint.

Memory loop headers retain bounded value states. A changing state widens to an unknown value after at most eight updates, subject to the configured alternative limit. The same limit bounds retained loop conditions. Widening releases dependencies used only to refine that value. It does not mark affected storage unchanged or remove unrelated memory facts. An unknown value covers less general alternatives under the same conditions; established function bindings remain available. A completed unknown condition is not repeatedly refined as pending work. Resource stops remain separate from completed conservative values.

Recursive control requests use provisional alternatives and repeat until their shared calculation is stable. An empty provisional result is not a completed proof that a function cannot return. Context changes discard the calculation. Only completed groups enter the refined-answer cache. Cache keys include call inputs, query choices, root scalar assignments, and the dependency generation.

Control work uses the enclosing request's allowance. Optional refinement uses the scalar allowance; required control work uses the remaining main-query allowance. A local refinement stop retains a conservative structural result and any fully proved paths already found. The stopped group is not cached as complete. Process, main-transfer, and context-change stops propagate. Control dependencies inside memory queries use the same bounded templates, with pending reads tracked by the memory worklist. A diagnostic records the control-refinement stop reason when that stop caused the unresolved result. With `IRQ_CHECK_STATS`, the checker reports control-summary evaluations, cache hits, precision-loss counts, refinement stops, and estimated retained bytes separately.

Scalar summaries retain bounded constants, ranges, enum variants, and parameter relations. Parameter-based memory summaries retain supported return expressions, entry-memory loads, and ordered conditional stores. Summary expressions have a 32-node bound. Read coverage is checked before summary application: a known enum tag does not establish its callback fields. Strong updates require a unique destination object. Unsupported effects still require bounded detailed refinement. General recursive memory-effect groups and removal of old memory histories remain incomplete. Rupta remains the primary pointer-analysis backend.

Memory-preservation checks use backend write regions and supported effect summaries. They no longer copy caller frames into a second coarse solver. The safe-target fallback uses complete backend candidate coverage and requires all candidates to be safe. It cannot establish a forbidden binding.

Target resolution and callee inspection continue when control is unresolved. Unknown return behaviour does not remove a normal MIR continuation or require a termination proof. Explicit unreachable exclusions retain their call-path checks. Return values, branch conditions, and memory effects remain separate: assuming continuation does not prove an Ok result, a branch condition, or an unchanged callback pointer. Trust retains its existing scope.

An established forbidden binding produces a forbidden-target error. If its IRQ path is unresolved, the same error includes a reachability uncertainty note and the source locations that caused uncertainty. An unresolved target still produces an unknown-call error. A backend candidate alone cannot produce a forbidden-target error. The evidence retains whether the path is proved; the diagnostic does not change that fact. A feasible path for some valid input is sufficient; the call need not occur for every input. Use `irq::unreachable(unsafe)` only for a reviewed fatal path.

For primitive integer and Boolean root arguments, the checker first tries two separate input assignments: all zero and all one. Each attempt has one scalar-step allowance. These attempts can report an established forbidden path. They cannot produce a Safe summary. If neither attempt proves a forbidden path, the normal analysis still runs. The attempts do not reset the process time or memory limits.

An unreachable block cuts IRQ queries before its pointer operations are evaluated. Global initialization discovery can still inspect that code. There is no fallback to the old context and flow solver, and no feedback loop from IRQ queries into Rupta.

On Windows, each driver has a committed-memory limit of at most 10 GiB. A Job Object applies the limit before compiler work starts. The default time limit is two hours. A process limit rejects the complete analysis. Partial results cannot pass.

Each pointer query starts with 100,000 transfer steps. It can retry once with an increased allowance. The precise-query counter is retained, and conservative fallback has a separate bounded allowance. Each case retains at most 32 alternatives. Scratch accounting defaults to 256 MiB per query and 16 MiB per function. Query caches have a combined 1 GiB budget. Up to four workers process independent IRQ roots by default.

Optional refinement has a default 1,000-step allowance within the pointer-query budget. A stopped request retains its stop status. It is not a completed unknown value. Required binding work uses the main query allowance. Read-cache keys include the refinement owner, index choices, and explicit caller choices. The work queue processes dependencies of the active request.

Set `IRQ_CHECK_MEMORY_MIB` to a positive value no greater than 10240. Set `IRQ_CHECK_TIMEOUT_SECONDS` to a positive timeout. Set `IRQ_CHECK_QUERY_STEPS`, `IRQ_CHECK_SCALAR_STEPS`, and `IRQ_CHECK_ALTERNATIVES` to positive work limits. `IRQ_CHECK_CALL_DEPTH` defaults to 256. `IRQ_CHECK_FUNCTION_SCRATCH_MIB` defaults to 16, and `IRQ_CHECK_QUERY_SCRATCH_MIB` defaults to 256. Local scratch limits cannot exceed the process memory allowance. `IRQ_CHECK_WORKERS` selects the worker limit. The cache key includes all resource settings and both checker binaries.

Identify the exact stop before increasing a limit. Keep each run bounded and record its settings. An increased query limit also changes retry and refinement allowances. More work does not prove that the implementation is correct.

Validation is in progress. The full scenario audit, the panic-only negative case, the block-contract cases, and both kernel targets must pass before acceptance.

The saved experiments do not establish a complete repair. Stage213 passes 113 non-panic cases, 45 non-panic cache checks, 18 contract checks, and 12 focused checks at increased work limits. Its panic negative case still reports one Forbidden and five Unknown errors. That saved source differs from this checkout. These counts are historical results, not validation of the current build. No saved result establishes a clean kernel check for the current source. See [saved analysis findings](TARGET_FINDINGS.md) for source identities, limits, failed experiments, and remaining work.

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

Trust allows unknown operations in this block only. It does not apply to a called helper, a nested closure, or an async body. It does not pass to a stored or returned value. A later unknown call needs its own trusted unsafe block. A forbidden call on a proved feasible IRQ path remains an error. The existing trust rule can accept unresolved reachability for an established target at the marked operation. Without that acceptance, an established forbidden target reports a forbidden error with a reachability uncertainty note.

Assembly and external calls without a known Rust body need this mark on a checked interrupt path. For `naked_asm!`, put the mark on the unsafe block inside the naked function. For `global_asm!`, put the mark at the Rust call to the external symbol.

The checker treats the compiler's `core` entries for x86 `rdtsc`, x86 `pause`, and AArch64 `isb` as known leaf operations. This rule does not trust other LLVM symbols or external functions with the same name in another crate.

## Accept incomplete analysis in a call path

```rust
#[cfg_attr(irq_check, irq::allow_incomplete(unsafe))]
unsafe {
    external_crate::operation();
}
```

Use this mark only as a last resort for reviewed external code. Prefer `trusted` for a local operation and `unreachable` for a fatal path that cannot run in IRQ context. Put the safety reason in a safety comment, not in the attribute arguments. The intended external-code restriction is a usage rule, not a crate-identity check.

The checker still analyzes the block and its synchronous calls. The mark accepts unresolved operations, effects, and reachability in that invocation. It does not accept a forbidden call on a proved feasible IRQ path. It does not accept compiler errors, invalid metadata, or whole-analysis resource failures.

Acceptance ends when the invocation returns or unwinds out. Stored callbacks, returned values, and later calls do not inherit it. Unknown memory effects remain unknown. A later callback read or path condition that depends on those effects can produce another unknown-call error outside the block. A separate unmarked call to the same function is still checked.

With `IRQ_CHECK_DUMP_DIR`, `<crate>-accepted-gaps.json` records accepted diagnostics and the sites that accept them. The graph retains the original gaps. Accepted gaps are not proved-safe summaries.

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

## Check dependencies

```toml
[dependencies]
kernel_sync = { path = "../libs/kernel_sync" }
kernel_executor = { path = "../libs/kernel_executor" }
```

Run Cargo through the checker:

```text
irq-check --cargo check --manifest-path kernel/Cargo.toml -p kernel
```

The rustOS build tool uses this command form. The root package and its active runtime dependencies are checked by default. This includes transitive runtime dependencies. Remove old `irq-check` dependency flags; they are rejected.

Selection uses Cargo's resolved package identity. It respects dependency renames, versions, sources, features, and target conditions. Host-only build dependencies and procedural macros are not runtime IRQ targets. Generated runtime code is checked. Each root build has a separate policy.

There is no crate-wide exclusion. Use a narrow explicit unsafe block when a reviewed analysis gap must be accepted. `core`, `alloc`, and `std` retain their built-in checks.

Internal metadata stores block scopes and package selection. The interface is `strict-blocks-4`. The build cache changes when the policy, limits, or checker binaries change.
