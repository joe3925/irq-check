# Bounded summary implementation plan

## Objective and approval boundary

Keep RUPTA as the primary pointer-analysis backend. Replace expensive execution histories with bounded summaries in stages.

Implement control-flow summaries first. Run the checks in steps 1 through 5, then stop at step 6 and report the results. Do not start steps 7 through 11 until the user approves that next stage.

Preserve the diagnostic and annotation contracts. Established Forbidden stays Forbidden. Unresolved reachability, bindings, or effects can produce Unknown. Unknown must not silently become Safe. Keep the current scope of trusted, allow_incomplete, and unreachable. A backend candidate alone does not establish a forbidden binding.

## 1. Preserve the baseline

- Preserve current source, executable pairs, hashes, logs, and audit records.
- Preserve dirty worktrees and all saved experiments. Use fresh output directories.
- Keep the permanent panic-only fixture unchanged.
- Record compiler, limits, commands, and executable hashes for comparisons.
- Start with query steps 1000000, scalar steps 100000, alternatives 128, function scratch 16 MiB, query scratch 256 MiB, process memory 10240 MiB, and timeout 300 seconds.
- Keep all limits enabled. Compare at equal limits before increasing them.

## 2. Define the control-flow summary boundary

Summaries describe reachability and return conditions. They do not replace pointer bindings or memory effects. Illustrative types:

```rust
enum Reachability {
    Excluded,
    Established,
    Unresolved,
}

struct ControlSummary {
    normal_return: Reachability,
    calls: Vec<CallReachability>,
    coverage: ControlCoverage,
}

enum SummaryOutcome<T> {
    Complete(T),
    Stopped(QueryStop),
}
```

Define the proof carried by each state. Completed conservative summaries may contain unresolved reachability. Stopped analysis is not completed analysis. Record input and context conditions required for summary reuse.

## 3. Implement control-flow summaries only

Replace separate control-path histories with merged states at relevant MIR blocks. Retain bounded constants, Boolean conditions, supported scalar ranges, symbolic argument and return conditions, reachability status, and references to existing binding and effect results.

```rust
match condition {
    BoolValue::Known(true) => follow(true_target),
    BoolValue::Known(false) => follow(false_target),
    BoolValue::Unknown => {
        follow_unresolved(true_target);
        follow_unresolved(false_target);
    }
}
```

Following an unresolved branch does not prove that it executes. For `write_message()?; forbidden();`, retain unresolved return conditions without requiring a byte-by-byte proof to complete the control summary. Apply existing diagnostic rules.

Merge states at loop headers. Broaden growing conditions to unresolved. Do not retain separate control states solely for iteration numbers. Preserve incoming-edge and loop relations still needed by the memory solver.

Compute recursive control summaries together. Unfinished callees are neither safe nor proved non-returning. Merge until stable and broaden growing conditions. Keep checking calls in recursive bodies. This stage does not replace recursive pointer-effect analysis.

## 4. Integrate without replacing pointer analysis

Keep RUPTA and its configuration unchanged. Preserve points-to sets, contexts, call graph, objects, projections, write regions, coverage, safe closures, and established-binding rules.

Keep the existing IRQ binding and memory solver. Retaining a possible return from `update_callback(&mut callback)` must not imply unchanged memory.

Use bounded requests for control conditions. Preserve stop reasons, or use an independently valid conservative summary. Cache with applicable input, context, dependency revision, and completion information. Keep annotation acceptance outside analysis results.

## 5. Measure and verify the control-only implementation

```powershell
cargo +nightly check --locked --bins
cargo +nightly build --locked --release --bins
```

Run existing focused checks sequentially: ordinary formatting, global-update, boot-registry, recursive formatting, correlated callbacks, aliases, returned callbacks, memory effects, and contract checks.

Then run the full non-panic audit and cache checks. Run panic-format safe, negative, and cache checks last among scenarios. Run bounded x86 and ARM kernel checks if no known safety defect makes them misleading.

Do not change expected results or contracts. Targets remain 113 non-panic cases, 45 applicable cache checks, existing contract checks, and permanent panic-only checks. Report failures.

Record diagnostics, elapsed time, peak process memory, control-summary count and size, temporary control storage, binding work and storage, cache reuse and invalidation, stop reasons, and separate RUPTA storage.

## 6. Required check-in and stop

Report what changed and what remains unchanged; scenario, contract, cache, and panic results; kernel results at matched limits; CPU and memory changes; remaining high-cost functions and structures; precision loss and new Unknown results; and a recommendation to keep, revise, or extend the design.

Wait for approval before step 7. If control summaries solve the practical problem, recommend reducing the later scope.

## 7. After approval: memory and call summaries

Use step 6 measurements to select work. Represent effects relative to parameters, globals, and symbolic objects instead of complete caller histories.

```rust
fn install(slot: &mut fn(), replacement: fn()) {
    *slot = replacement;
}

fn invoke(callback: fn()) {
    callback();
}
```

The summaries express `writes *argument 0 = argument 1` and `calls argument 0`. Substitute actual arguments and applicable RUPTA context at each call.

```rust
struct FunctionSummary {
    control: ControlSummary,
    return_value: AbstractValue,
    effects: EffectGraph,
    calls: Vec<CallEffect>,
    coverage: Coverage,
}
```

Preserve necessary ordering, aliases, field offsets, returned pointers, escaping objects, and globals. Strong updates require a unique destination. Unknown effects invalidate all memory they can modify.

## 8. After approval: recursive effects

Compute mutually recursive effects together. Reprocess dependencies when effects change and update groups when new call edges appear. Bound specialisations and abstract values. Broaden changing counters to unknown integers while preserving useful symbolic callback effects.

Do not check one frame and skip recursive effects. Completed summaries must conservatively cover those effects; interrupted analysis remains incomplete.

## 9. Restrict detailed refinement

Use RUPTA and summaries first. Retain bounded refinement for registry selection, reaching writes, and receiver bindings. Preserve correlations required by existing scenarios. Do not reconstruct full execution histories or add a hidden old-solver fallback. Retain established diagnostics and report remaining uncertainty.

## 10. Complete cache and storage changes

Share backend data. Bound specialisations. Track revisions. Cache reusable completed results. Release temporary states. Preserve annotation scope. Invalidate incompatible persistent caches. Remove obsolete history machinery only after replacing its uses.

Account separately for backend data, summaries, temporary states, refinement, and evidence. Retries must not silently reset overall allowances.

## 11. Final audit and kernel checks

Repeat existing scenario, contract, cache, and panic checks. Run x86 and ARM kernel checks. Increase non-memory limits only when useful progress is measured; retain the 10 GiB process cap.

Classify gaps as unsupported behaviour, deliberate precision loss, resource exhaustion, or incorrect binding, summary, or invalidation behaviour. Preserve the kernel migration. Do not add annotations to conceal checker defects. Any external-code annotation requires a specific justification.

Update the README and evidence descriptions for the implemented architecture and unchanged contract.

## Working constraints

No new tests, no new source comments, no whole-project cargo fmt, and no small single-caller wrappers. Do not alter expected results to hide failures. Preserve saved evidence. Do not replace or weaken RUPTA.

The first delivery is control-flow summaries and measured results, followed by the required stop. Later stages require further approval.

## Execution record

The baseline is saved in `target/control-summary-20261018/baseline/`. The first control-only candidate used for the full audit is `target/control-summary-20261018/stage02/`. Its driver SHA256 is `3BC4A30B14B6371D488D6CDBE2A09D01962B71F6FC673B106ACC9151CE18045E`.

The candidate adds `src/analysis/sparse/control.rs` and routes call-site control checks through it. Structural summaries are shared by function, IRQ mode, and loop-entry constraints. Block alternatives have a bound of eight or the configured alternative limit, whichever is smaller. Refined answers include frame, input choices, index choices, scalar witnesses, dependency generation, and recursion-cut mode in their cache key.

Control cycles and excess structural alternatives lose precision explicitly. An unresolved control marker survives recursive return mapping. Stopped refinement uses a conservative structural result and does not enter the refined-answer cache as a completed precise result.

RUPTA, `src/analysis/global.rs`, `src/analysis/query.rs`, `src/analysis/sparse/flow.rs`, and the driver diagnostic rules remain unchanged. Memory queries still use their existing control dependencies. The memory and recursive-effect summary stages have not started.

The compiler check and release build pass. The 18 existing contract checks pass unchanged. Detailed scenario and kernel results will be recorded at the required check-in.

Stage02 passed 89 of 94 standard cases, 19 of 19 extra cases, 45 of 45 cache checks, and 18 of 18 contract checks. The non-panic total was 108 of 113. Ordinary formatting still failed. Four additional negatives reported Unknown: cut, global-update, local-callback, and unbounded,cut. Panic-format safe and cache passed, but negative reached the 300-second analysis limit. These are not accepted improvements.

Stage03 changes only the use of an unresolved control result. If every retained path has a non-removable unresolved marker, an untrusted call query reports Unknown at that boundary instead of expanding callees that cannot produce an established path. Other paths remain checked. Locally trusted calls retain the previous expansion because moving an Unknown diagnostic to that call site could change annotation acceptance. Stage03 requires a fresh audit before the check-in.

Stage03 was rejected after focused checks. It reduced formatting transfers to 282207, but rejected global-update safe and boot-registry safe before using their binding results. Its full audit was stopped; partial records remain saved. Its 18 contract checks passed.

Stage04 retains target resolution before applying the unresolved-control boundary. It keeps safe and forbidden leaf targets and calls already in the caller chain. Only a new non-leaf callee on entirely unresolved paths is replaced with an explicit Unknown coverage gap. The diagnostic rules and annotation scope remain unchanged.

Stage04 restored the focused safe global-update and boot-registry results. Its 18 contract checks passed. Its full audit was stopped after review found duplicate scalar-work accounting. An outer optional request already charges its nested work. The control summary also charged the same work. Stage05 charges control refinement directly only when no outer refinement request owns the allowance. The main transfer counter is unchanged. Stage04 partial audit records remain saved; they are not a full-suite result.

## Control-only check-in: 20 October 2026

The retained candidate is stage05. The source matches its saved snapshot. Its driver SHA256 is `8D8396B41B4587A0C6C60627CDB254CADF0CF639D3A06C327E80A974CE59D9AF`. The runs finished on 18 October. Their results were checked on 20 October after the user resumed the task.

The candidate is not accepted. The control-only stage is not fully complete. Structural block summaries and bounded refinement are implemented, but recursive control groups do not yet have a shared fixed-point calculation. A repeated active control dependency becomes unresolved. There is also no separate summary domain for scalar ranges: condition refinement still uses the existing value solver. These limits are material, not deferred memory-effect work.

The complete result index is `target/control-summary-20261018/stage05/audit-results.json`. The same directory contains the settings, source, executables, full scenario records, and runners.

| Check | Saved baseline | Stage05 |
| --- | ---: | ---: |
| Standard cases | 93/94 | 89/94 |
| Extra cases | 19/19 | 19/19 |
| Non-panic total | 112/113 | 108/113 |
| Applicable cache checks | 45/45 | 45/45 |
| Annotation-contract checks | 18/18 | 18/18 |
| Panic-format safe and cache | Pass | Pass |
| Panic-format negative | Pass | Fail: Unknown |

The baseline full audit is the saved source46 audit, not a new full baseline run. Its records are in `target/coverage-contract-20261018/audit46/`. A fresh measured baseline formatting run used the executable saved at the start of this task.

Ordinary formatting remains a failure. Four non-panic configurations have new failures: recursive cut, global-update, local-callback, and unbounded,cut, each with the negative feature. Cut and unbounded,cut should pass without an error because their unreachable blocks cut the recursive path. Stage05 cannot prove that those calls do not return. Global-update has an established forbidden target but an unresolved path. Local-callback stops at unresolved control summaries. These are checker precision regressions, not evidence that RUPTA lost its target bindings.

The permanent panic-only fixture was not changed. Its negative run finished without a process timeout, but emitted 12 Unknown diagnostics. It did not establish the required forbidden panic route. Its directory is `target/scenarios/20261018-200757-8006508/`.

### Formatting measurements

Both measured runs used the same limits and failed.

| Measurement | Baseline | Stage05 |
| --- | ---: | ---: |
| Transfers | 3575594 | 303229 |
| Elapsed seconds, including build | 52.13 | 25.94 |
| Sampled checker CPU seconds | 25.73 | 0.31 |
| Sampled peak private MiB | 162.77 | 119.66 |
| Observed peak committed MiB | 162.83 | 163.02 |

The measured directories are `target/scenarios/memory-20261018-200833-0919782/` and `target/scenarios/memory-20261018-200925-2264845/`. Samples can miss short peaks and final CPU use. Peak committed memory is nearly unchanged. The lower transfer count partly comes from stopping before unresolved callees are expanded. It is not proof of equivalent analysis at lower cost.

Stage05 retained about 1.02 MiB of estimated control-summary storage in this formatting run. Separate temporary-control and memory-query storage peaks are not yet measured.

### Kernel checks

Both checks used a 300-second internal time limit and a 10 GiB process-memory cap. No kernel source or annotations were changed in this stage.

| Target | Result | Total elapsed, including build | Sampled kernel peak private memory |
| --- | --- | ---: | ---: |
| x86 | Analysis timeout | 510.26 seconds | 1.88 GiB |
| ARM | 98 Unknown errors; diagnostics completed | 169.38 seconds | 1.66 GiB |

The x86 directory is `target/control-summary-20261018/control05-x86_64-uefi-20261018-200951-2044749/`. Its partial precision record contains 461 unresolved-control entries, 16 scratch-limit entries in UTF-8 validation, and three transfer-limit entries in `RoundRobinDomainAlgorithm::pick_next`. The exact scratch-limit branch was not traced in this run. No partial x86 result is accepted.

The ARM directory is `target/control-summary-20261018/control05-aarch64-uefi-20261018-201821-4871739/`. Its 98 diagnostics comprise 94 unresolved-control errors, three transfer-limit errors in `pick_next`, and one assembly coverage gap. The assembly is the external `aarch64_cpu` read of `TPIDR_EL1`, reached through `per_cpu_ptr` on a panic path. That gap needs a separate review. The control and transfer-limit errors must not be concealed with external-code annotations.

RUPTA points-to-set storage was 283598280 bytes on x86 and 232152392 bytes on ARM. The largest reported final control-cache sizes were about 4.88 MiB and 3.63 MiB per worker. These are estimates, not simultaneous process peaks. Compiler storage, other backend structures, frames, and memory-query state are not fully separated by the current measurements. The old kernel runs have no matched peak-memory samples, so this report does not claim a kernel memory reduction.

### Decision at the required stop

The compiler check and release build pass. No tests, source comments, scenario changes, annotation changes, or whole-project formatting were added. RUPTA, the memory solver, and diagnostic rules remain unchanged. All saved experiments remain available.

The accounting correction did not restore the lost proofs. The next work should revise control summaries before memory summaries are introduced. It must compute recursive return information together and preserve useful branch and return relations through merges. The intended result remains:

```rust
match summary.reachability(call_site) {
    Reachability::Excluded => skip_call(),
    Reachability::Established => check_established_binding(),
    Reachability::Unresolved => check_binding_and_retain_uncertainty(),
}
```

This is a contract example, not additional implemented code. An unresolved result must not become Forbidden or Safe without the required proof.

Work stops here for the requested review. Steps 7 through 11 have not started. The candidate remains in the worktree with its failures recorded; it was not reverted or installed as an accepted replacement.

## Resumed control migration: 20 October 2026

The user approved continuation after the regression report. Continue the control stage before memory-effect migration.

First, replace immediate recursion widening with a bounded shared calculation. Recursive control requests use provisional alternatives. Repeat dependent work until the alternatives stop changing. Publish completed answers only after the group completes. A generation change invalidates the calculation. A limit stop retains uncertainty; an empty provisional answer is not proof of no return.

```rust
match recursive_group.solve_within_limits() {
    Complete(summary) => use_summary(summary),
    Stopped(reason) => retain_uncertainty(reason),
}
```

Next, retain completed independent branch proofs when another part of refinement stops. Review the eight-alternative structural merge separately. Compare each change at the same limits with cut, unbounded cut, global-update, local-callback, boot-registry, and formatting. Then repeat contract, full-suite, panic-only, and kernel checks as justified by the results. Do not change RUPTA, diagnostic contracts, or expected results to obtain a pass.

### Resumed implementation record

Artifacts for this work are under `target/control-summary-20261020/`. Each stage has source, executables, settings, and focused results.

Stage06 implements the shared recursive control calculation. Provisional alternatives start empty and grow until the group is stable. Only stable results enter the completed cache. Context changes and limit stops discard the provisional group. This restores cut and unbounded,cut at the original limits. The focused checks pass 18 of 21 checks, including cache checks. Global-update, local-callback, and ordinary formatting negative still fail.

Stage07 removes the independent scalar cap from required control work. The enclosing request now owns the allowance. Optional requests retain their scalar cap; required requests use the remaining main-query allowance. No configured limit was increased. The global-update trace in stage06 showed a stop at 100000 steps and later attempts with zero remaining scalar allowance. Stage07 restores global-update safe, negative, and cache checks. The focused result is 19 of 21 checks. Local-callback finds a real forbidden call but also reports Unknown.

Stage08 keeps specific structural alternatives and one conservative remainder when the path bound is reached. It retains loop-entry and incoming-edge relations needed by the memory solver. It does not add iteration histories. The focused result remains 19 of 21. All 18 annotation-contract checks pass.

Stage09 uses the existing branch simplification before structural widening. Paths with the same return requirements can combine exhaustive branch conditions exactly. It also compares summary alternatives as sets and accounts for provisional storage by its current retained size. The focused result remains 19 of 21. All 18 annotation-contract checks pass.

Stage10 retains fully proved paths found before a later local refinement stop. The result also retains an independent conservative fallback, and the stopped group does not enter the completed cache. Process, main-transfer, and context-change stops still propagate. The full non-panic audit passes 111 of 113 cases, with 45 of 45 cache checks and 18 of 18 annotation-contract checks. Ordinary formatting negative and local-callback negative still fail. Panic-format safe and cache checks pass. Panic-format negative establishes the forbidden panic route, but also reports Unknown on an unresolved route, so it fails.

The user increased the kernel analysis time limit to 1200 seconds on 20 October. The memory cap remains 10240 MiB. All other kernel settings remain unchanged. The 300-second x86 run in `target/control-summary-20261020/control10-x86_64-uefi-20261020-192017-0373330/` was stopped before completion. Its logs remain available; it is not an accepted result. New runs use the `control10-1200-` prefix. Both new runs reach the analysis time limit. Results are recorded below.

The resumed changes are confined to control summaries. The RUPTA adapter, memory-flow engine, query definitions, diagnostic driver, scenario source, and expected results remain unchanged.

### Stage10 measured results

The driver SHA256 is `31BD093EF4FA0F7BCD35BA37059197230994B4E620DE4B0DF799BD6C7FC4898E`. Current solver source matches the saved stage10 source. Compiler checking and the release build pass. No tests, source comments, kernel annotations, or scenario changes were added during this resumed stage.

| Check | Saved baseline | Stage05 | Stage10 |
| --- | ---: | ---: | ---: |
| Non-panic scenarios | 112/113 | 108/113 | 111/113 |
| Applicable cache checks | 45/45 | 45/45 | 45/45 |
| Annotation-contract checks | 18/18 | 18/18 | 18/18 |
| Panic-format safe and cache | Pass | Pass | Pass |
| Panic-format negative | Pass | Unknown | Forbidden and Unknown |

Stage10 restores cut, unbounded cut, and global-update. Local-callback negative remains a regression. Its binding to the forbidden service is established, but the graph retains both a proved path and an unresolved path. The full-audit log is `target/scenarios/20261020-191416-1460516/recursive-bindings-negative.log`. Its main queries use 13228900 transfers; the two input-witness attempts use another 200000 transfers.

Ordinary formatting negative reports three unresolved-control errors and does not establish the required forbidden call. Its log is `target/scenarios/20261020-190949-8905919/formatting-negative.log`. It uses 3664717 transfers. The old stage05 formatting transfer reduction does not apply to this candidate.

Panic-format negative now establishes the real route through `panic_bounds_check`, `panic_handler`, and `PanicInfo::fmt`. It also retains an unresolved path to the same forbidden service. Both diagnostics are at analysis site 260, context 750. The log is `target/scenarios/20261020-191802-6751084/recursive-bindings-negative.log`. Its main queries use 87477031 transfers; input-witness attempts use another 200000. The panic-only fixture remains unchanged. This result is a failed scenario, not an accepted negative result.

The full standard audit takes 499.13 seconds and the extra audit takes 155.84 seconds, including build work. These totals are not isolated solver CPU measurements. All records are under `target/control-summary-20261020/stage10/`; contract results are in `target/contract-checks/20261020-190619-5941637/results.json`.

The x86 kernel check with the approved 1200-second analysis limit reaches that limit. Total elapsed time is 1293.74 seconds, including build work. Sampled kernel peak private memory is 2008510464 bytes, about 1.87 GiB. Sampled kernel CPU time is 2719.75 seconds across worker threads. CPU time can exceed elapsed time. No partial result is accepted.

The x86 partial precision record contains 457 unresolved-control entries, 26 scratch-limit entries in UTF-8 validation, and four transfer-limit entries in `RoundRobinDomainAlgorithm::pick_next`. These entries are not completed diagnostics. The exact scratch-limit branch was not traced. Main queries use 1367938427 transfers, with another 800000 transfers in input-witness attempts. The timeout is therefore not explained by the 10 GiB process cap. It does not prove that a larger query or scratch allowance would help.

RUPTA points-to-set storage on x86 is 283598280 bytes. The largest final worker control-cache estimate is 7132768 bytes, about 6.80 MiB. This is not a simultaneous control-storage peak and does not account for all compiler, backend, frame, memory-query, and evidence storage. The x86 directory is `target/control-summary-20261020/control10-1200-x86_64-uefi-20261020-192405-5073613/`.

ARM also reaches the 1200-second analysis limit. Total elapsed time is 1297.81 seconds, including build work. Sampled kernel peak private memory is 1819865088 bytes, about 1.69 GiB. Sampled kernel CPU time is 2756.16 seconds. The partial precision record contains 299 unresolved-control entries and four transfer-limit entries in `pick_next`. Main queries use 1420292353 transfers, with another 800000 transfers in input-witness attempts. RUPTA points-to-set storage is 232152392 bytes. The largest final worker control-cache estimate is 5189296 bytes, about 4.95 MiB. The ARM directory is `target/control-summary-20261020/control10-1200-aarch64-uefi-20261020-194539-3818241/`.

Both kernel runs set `incomplete=true` and `diagnostics_complete=false`. Both complete RUPTA before IRQ queries reach the time limit. Neither result proves complete callback coverage. The old ARM assembly finding is not resolved by its absence from this partial record. No new external-code annotation is justified by these runs.

| Stage10 kernel target | Analysis limit | Result | Sampled peak private memory |
| --- | ---: | --- | ---: |
| x86 | 1200 seconds | Analysis timeout | 1.87 GiB |
| ARM | 1200 seconds | Analysis timeout | 1.69 GiB |

The retained source and driver are unchanged during these checks. The kernel runner now uses 1200 seconds; the completed scenario audit used 300 seconds. Results at those different time limits are recorded separately. The old stage05 ARM check finished with Unknown errors at 300 seconds; stage10 does not finish at 1200 seconds. Restoring scenario proofs has not solved kernel analysis cost. This is not an accepted performance result.

These results do not establish a new RUPTA binding defect. Unresolved control and resource stops must not be concealed with kernel annotations. The control stage still needs revision. Memory-effect summary migration has not started.

The current implementation provides bounded structural path templates and a shared recursive control calculation. It still uses frame-specific value-flow requests for branch and call-return facts. The planned scalar-range and symbolic argument/return summary domain is not complete. Thus step 3 is not complete, even though the measured candidate passes more scenarios. The next review must retain this distinction; the full summary migration has not been implemented.

## Continued migration approval: 20 October 2026

The user instructed continuation until the migration is complete and the expected results are obtained. This removes the planned stop before steps 7 through 11. It does not permit weaker diagnostic rules, hidden failures, or a memory cap above 10 GiB.

Measure structural summary construction separately from branch and return refinement. First repair any unstable summary merge or repeated work shown by this measurement. Then replace frame-specific scalar and return calculations with bounded summaries, and extend the same dependency model to call and memory effects. Retain precise binding refinement where a callback depends on a selector, alias, or reaching write.

```rust
match summary.lookup(input_facts) {
    SummaryOutcome::Complete(facts) => apply(facts),
    SummaryOutcome::Stopped(reason) => retain_incomplete(reason),
}
```

This is a contract example, not implemented code. A complete conservative result can retain uncertainty. A stopped calculation cannot become a completed proof. Repeat existing focused, contract, full, panic-only, and kernel checks after the relevant stages. Kernel checks retain the 1200-second analysis allowance unless further evidence justifies another permitted limit change.

Stage11 adds an optional `IRQ_CHECK_TRACE_CONTROL` cost trace. It does not change solver semantics. In local-callback, the formatting template takes about 0.006 seconds, while one return-refinement chain uses about 991000 transfers. In ordinary formatting, refinement of `panic_common` stops on scratch after 8.31 seconds. The ARM trace records repeated return and branch refinement, including an eight-million-transfer retry in `panic_common` that takes 35.18 seconds. The diagnostic ARM run was stopped after these records were collected. It is not a complete kernel result.

Stage12 permits an independent completed control result to enter the cache before the enclosing request group finishes. A request that consumes a provisional recursive result remains pending. The shared group must still stabilize before those results become complete. If a later dependency stops, earlier completed independent results remain reusable under their existing context and generation keys.

```rust
if dependencies_complete {
    cache_completed(key, values);
} else {
    retain_provisional(key, values);
}
```

This example describes the completion rule. Stage12 validation is pending. This change does not replace scalar or memory summaries.

Stage12 passes 19 of 21 focused checks, including cache checks. Local-callback negative and ordinary formatting negative still fail. Local-callback main transfers decrease from 13228900 in the stage10 full audit to 10347854 in the focused stage12 run. Ordinary formatting is nearly unchanged at 3664497 transfers. This is a work reduction, not a complete fix.

Stage13 introduces a frame-independent scalar summary worklist. It tracks constants, enum tags, and direct argument returns. Called-function results form a dependency graph. Only a completed calculation enters the scalar cache. The control solver uses a known scalar condition before it requests detailed value flow. It does not infer call termination or forbidden reachability from a scalar result alone. Mutable or indirectly accessed storage remains unknown in this pass.

The stage13 audit was stopped during type review. Float bit patterns needed exclusion from integer operations, and integer results needed normalization to their destination width. Stage14 includes those corrections and passes 19 of 21 focused checks. The same two negative cases fail. No stage13 partial audit is an accepted result.

Stage15 adds a return-type check before scalar summary substitution and more optional scalar trace data. Its validation is pending. The implementation is in `src/analysis/sparse/scalar.rs`; the existing binding and memory-flow solver remains active. Scalar ranges, projected symbolic loads, and memory-effect summaries are not complete.

Stage15 passes all 18 annotation-contract checks in `target/contract-checks/20261020-203216-5654532/results.json`. Its local-callback and ordinary formatting negative checks still fail. The trace proves that the scalar pass knows `Sink::write_str` returns variant zero, but loses that fact through `Argument::fmt` in the context-independent calculation.

Stage16 keys scalar summaries by the existing RUPTA context, function instance, and IRQ mode. It follows only coverage-checked context call bindings, or the existing complete global target sets. It joins facts from all contexts attached to an IRQ frame before using a condition. Missing coverage remains unknown. These facts do not establish forbidden bindings. Stage16 passes 19 of 21 focused checks. The same two negative cases fail.

Stage17 adds bounded scalar input specialization and reads constant enum tags through the compiler's constant evaluator. Argument types must match before substitution. Each function, IRQ mode, and backend context permits at most eight specialized input sets. Further inputs use the general summary. Pending inputs do not become completed unknown inputs.

```rust
match (known_input, specialized_count) {
    (Some(input), 0..8) => use_specialized_summary(input),
    _ => use_general_summary(),
}
```

This example states the bound, not the exact implementation. Stage17 passes 19 of 21 focused checks at the original matched limits. Its local-callback trace now preserves variant zero through `Argument::fmt`, `core::fmt::write`, and `SpecWriteFmt`. The scalar pass proves 35 constant conditions in that group, compared with 13 in stage16. The main local-callback calculation decreases from 10360521 to 9807238 transfers. It still fails at scalar allowance 100000.

A separate stage17 limit check increases only the scalar allowance to 1000000. Local-callback safe, negative, and cache checks pass. Both root-witness attempts still exhaust their allowance; the accepted forbidden path comes from the main calculation, which uses 8803776 transfers. Thus this result does not show that a root witness succeeded. The directory is `target/scenarios/20261020-204323-1899273/`. Settings and the driver hash are in `target/control-summary-20261020/stage17-limits/`.

Stage18 uses completed scalar conditions before control-template path merging. Impossible edges are removed before the bounded merge can lose their relation. The template key includes the exact constant-condition set. Unknown conditions remain in the graph. Compiler checking and the release build pass; scenario validation is pending. This change does not establish normal return from a known return value, and does not replace memory-effect analysis.

Stage18 passes 19 of 21 focused checks and all 18 annotation-contract checks. Its local-callback control templates use 783712 and 691248 estimated bytes, compared with 909296 and 832944 before constant-edge filtering. Local-callback still fails at scalar allowance 100000. Its main calculation uses 9669277 transfers. Contract results are in `target/contract-checks/20261020-204618-4063979/`.

The separate stage17 formatting scratch check increases function scratch from 16 to 128 MiB and query scratch from 256 to 1024 MiB. Other settings are unchanged. Safe and cache checks pass; negative still fails. The three required control queries now stop at their transfer allowance instead of the old scratch threshold. Main queries use 27009057 transfers. The three retries take 90.66, 98.01, and 98.54 seconds. This is not evidence that more scratch repairs formatting. The log is `target/scenarios/20261020-204356-9125316/formatting-negative.log`.

Stage19 adds parameter-based summaries for straight-line functions. They describe constant or argument returns and ordered stores through argument pointers, including fixed field offsets. They do not retain a caller frame. Actual argument types must match before substitution. A strong update requires a unique destination; unknown aliases and partial overlaps retain uncertainty. Unsupported calls, loops, expressions, and storage patterns remain with the existing solver. A stopped summary construction is not cached as complete.

```rust
fn replace(slot: &mut fn(), replacement: fn()) -> fn() {
    *slot = replacement;
    replacement
}
```

The summary for this example records one store and a returned argument. Both use the argument value at call entry. The implementation is in `src/analysis/sparse/effects.rs`. The existing memory solver is still present; full recursive-effect migration is not complete. Stage19 passes compiler checking, release build, and all 18 contract checks in `target/contract-checks/20261020-205211-8166782/`. Focused checks are pending.

Stage20 adds a bounded unsigned scalar-range domain. A growing interval widens to unknown instead of growing for each loop iteration. Comparisons can use interval bounds. Direct comparisons against constants narrow the local value on each successor edge, provided the source has not changed after the comparison. Compiler checking passes; release and scenario checks are pending.

Stage19 and stage20 each pass 19 of 21 focused checks. Local-callback negative and ordinary formatting negative still fail. Stage20 also passes all 18 contract checks in `target/contract-checks/20261020-205634-6856368/`. Its driver SHA256 is `DC3805582665F1BB7DEC1E42F5FF7D2CBF87E3FD74DDF5BCD18E8DF4D79D7700`.

Stage20 passes safe, negative, and cache checks for pointer-alias, correlated-dyn, erased-records, and mixed-dyn: 12 of 12 checks. Their logs are in `target/scenarios/20261020-205853-9127265/`. This is not a full memory-scenario audit.

Stage21 adds parameter-based loads that occur before the first write in a straight-line function. A copied load can be returned or stored later. Its value refers to memory at call entry, not memory after the summary's writes. New loads after a write are not summarized. Unknown addresses remain unknown. Optional statistics count summary reuse, return substitutions, stores, loads, and retained summary bytes. Compiler checking and the release build pass. The frozen driver SHA256 is `16DE67ECADACDF9420C84923E7657BA143C5B8E1721E3A832F29BDD39B1D2154`. Focused validation is pending.

Stage22 adds fixed fields of by-value arguments and addresses of sized fields reached through argument pointers. A field address does not read the pointee. Stores through such an address retain the original argument and fixed byte offset. Unsized field addresses and multiple dereferences remain with the existing analysis. Compiler checking passes; the release build is in progress. These stages do not yet implement general call composition or recursive memory effects.

Stage21 and stage22 each pass 19 of 21 focused checks. Stage22 passes all 18 contract checks in `target/contract-checks/20261020-210331-3073080/`. Its driver SHA256 is `671BC4898D476E8EF07A5869E51FD70336C966A9363E67F222631854781C23AB`. The full non-panic audit is running on that frozen executable at the matched limits and 1200-second timeout.

Stage21 local-callback main queries use 9610254 transfers and 501 frames. The new summaries supply 2746 return substitutions and 2145 loads, with 34880 estimated retained bytes. There are no summarized stores in this run. Ordinary formatting uses 3979711 transfers and still fails. These counters show use of the new path; they do not show that the main performance problem is solved.

Stage23 composes summaries across direct calls. It checks argument and return types, substitutes parameter values, and preserves store order. A callee load after an earlier caller write is not summarized. Recursive dependencies, opaque callees, and unsupported expressions prevent this fast path from completing. Thread-local access remains unsupported because the compiler describes it as a runtime operation. The compiler check and release build pass. The frozen driver SHA256 is `E49918215C5509E84426842048B6494BEB3881DCAEA42A68A61139D44BBF7AF1`. This candidate has no scenario audit yet.

Stage24 adds a separate normal-return proof. It checks that all normal successors reach a return and that every required callee has a normal-return proof. The dependency calculation starts without recursive proofs. Loops and unresolved recursive groups therefore do not become proved returns. Complete backend target sets can support this proof only if every target returns normally; this does not establish a forbidden binding. Unknown branch values need no further refinement when all branches return. A failed or stopped proof does not establish non-return.

```rust
if normal_successors_return && callees_return {
    record_normal_return();
} else {
    retain_return_obligation();
}
```

This example states the proof boundary. The existing detailed calculation still handles unresolved return obligations. Stage24 passes compiler checking; its release build and scenario validation are pending. The complete memory and recursive-effect migration remains unfinished.

The stage22 full audit finishes with 111 of 113 non-panic cases and 45 of 45 cache checks. Standard cases pass 92 of 94 in 485.98 seconds; extra cases pass 19 of 19 in 157.35 seconds. These elapsed times include build work. The only failed cases are ordinary formatting negative and recursive local-callback negative. Reports are in `target/control-summary-20261020/stage22/`. This candidate adds no further full-suite regression relative to stage10, but it has not restored the original local-callback result at the matched scalar allowance.

Stage24 passes the release build and all 18 contract checks in `target/contract-checks/20261020-211500-5039806/`. Its driver SHA256 is `5A572BD85207C4ADAE36F31D5348D6F3DB9B240112E420E72369220FCA209BBB`.

Stage25 restricts normal-return switch edges to the declared enum discriminants when the branch directly reads an enum discriminant. It also uses literal constant switches. Other switches retain all successors. A compiler-generated invalid-tag edge does not prevent a return proof for valid enum values. This does not assume that an arbitrary unreachable block returns. The compiler check and release build pass. The frozen driver SHA256 is `DFA1A011918EBAC5B5469B68AF1E35FE69213F45F173128DC875E889A7BE236D`. Contract and focused checks are running.

Stage25 passes 19 of 21 focused checks and all 18 contract checks. At scalar allowance 1000000, global-update, local-callback, boot-registry, and recursive formatting pass safe, negative, and cache checks. Ordinary formatting passes safe and cache but fails negative. This gives 14 of 15 focused checks, not a full audit. Settings and records are in `target/control-summary-20261020/stage25-scalar1m/`.

A further ordinary-formatting check uses query allowance 10000000, scalar allowance 1000000, function scratch 128 MiB, query scratch 1024 MiB, process memory 10240 MiB, and timeout 1200 seconds. Negative still fails with three Unknown errors. It uses 39606152 transfers and 1453 frames. Each of the three required control queries first uses nine million transfers, then stops at the per-function alternative-storage limit in `Buffer::write_str`. One recorded alternative set has 4194 guards, of which 3706 are value-choice guards. The first stop has 22910 demands and 543216704 estimated retained bytes. These are solver accounting figures, not a measured process-memory peak. The log is `target/scenarios/20261020-212438-0272255/formatting-negative.log`. More allowance does not remove the retained-history problem.

Stage26 adds a narrow loop-termination proof to the normal-return summary. It requires an unsigned local counter, a fixed bound, a strict comparison, one counter update by one per iteration, and no inner back edge. The counter and bound must not have escaped addresses. All calls still require normal-return proofs. Other loops and recursive calls remain unproved. This proof does not infer memory values or remove callback conditions.

```rust
while position < end {
    position += 1;
}
```

This example shows the supported progress relation. The compiler check, release build, and all 18 contract checks pass. The frozen driver SHA256 is `07EFCA416605F0704661E1046845AD0ED4C240BC10B948145DACE3A37144AF39`. Focused checks are running. Full recursive memory-effect summaries and removal of old execution histories remain incomplete.

Stage26 passes 19 of 21 focused checks at the matched limits. The same two negative cases fail. A separate panic-only check at scalar allowance 1000000 passes safe, negative, and cache checks. The negative log contains one Forbidden diagnostic and no Unknown diagnostic. Its established route includes `panic_bounds_check`, `panic_handler`, and `PanicInfo::fmt`. Main queries use 111795486 transfers and 1867 frames; the two root-witness attempts each use another 1000000 transfers. The negative run stops after the established forbidden diagnostic, so it is not complete coverage of every later route. The fixture is unchanged. Records are in `target/control-summary-20261020/stage26-panic-scalar1m/`; the log is `target/scenarios/20261020-213513-8960280/recursive-bindings-negative.log`.

Stage27 retains bounded arithmetic expressions over parameters and entry-memory loads in the effect summaries. Integer operations and pointer offsets use the actual call arguments without allocating a callee frame. Expressions have a 32-node bound. A partial or unsupported expression read remains unknown. Memory loads after a write remain unsupported. Compiler checking, the release build, and all 18 contract checks pass. The frozen driver SHA256 is `F4661EB18ACC91B94C046EE09585190DFE7C85A5A02511869B2837E18C4CF75B`. Focused checks are running.

Stage28 extends these expressions to small conditional return summaries. All retained branches must reach a return. At most eight paths and 32 expression nodes are retained. The summary evaluates each selecting condition against actual arguments. An unknown condition retains a widened proof marker; it cannot establish a forbidden binding. Branch-dependent stores and recursive effects are not implemented in this stage. Compiler checking and the release build pass. Validation is pending.

```rust
fn select(enabled: bool, first: fn(), second: fn()) -> fn() {
    if enabled { first } else { second }
}
```

The summary retains both return expressions and their selection conditions. It does not promote the two possible callbacks to established bindings.

Stage27 and stage28 each pass 19 of 21 focused checks. The same two negative cases fail. Stage28 also passes all 18 contract checks. Its frozen driver SHA256 is `23046FE897FBBA5CE5E956FF972F3823CEEC67382353E167A2E76B7FC8596CD8`.

Stage29 adds conditional stores to bounded summaries. Each store retains its selecting conditions. A false condition preserves the earlier value. An unresolved condition retains both the earlier and written values with an unresolved proof marker. Callee composition substitutes these conditions along with the store operands. Entry-memory loads must still precede writes. Compiler checking, the release build, and all 18 contract checks pass. The frozen driver SHA256 is `03A1C7AAF1CBBDAA78F02DE765FB1F8CA2ED857A8FD5266F3BD4EA391B4C0B81`. Focused checks are running.

```rust
fn replace(enabled: bool, slot: &mut fn(), replacement: fn()) {
    if enabled { *slot = replacement; }
}
```

The summary does not treat an unresolved `enabled` value as a definite replacement.

Stage30 also applies the completed normal-return proof inside the old control dependency used by memory-loop queries. Previously, only the new control-summary path used this proof. The memory worklist could still expand those same callee returns. This change does not assume that an unresolved callee returns. Compiler checking passes; the release build is running.

Stage29 and stage30 each pass 19 of 21 focused checks and all 18 contract checks. The same two negative cases fail. Stage30's frozen driver SHA256 is `EFBCD764BF51DE816ADF393D5437CB272CCFA286F6250950AA15B28F6320614D`.

Stage31 extends the normal-return proof to the existing supported leaf intrinsics. Abort, unreachable, breakpoint, unknown intrinsic effects, and never-returning signatures are excluded. The existing validity check must succeed for type-validity intrinsics. This does not remove memory effects. The compiler check, release build, and 18 contract checks pass. The frozen driver SHA256 is `B69E0E4E4DD16D09CBAD1DE292DE63BCB357A38DC8394860775986DC2AB93833`.

Stage32 records direct function-item conversions in parameter-based summaries. This uses the compiler's function-pointer instance resolution, not backend candidates. Selection conditions remain attached. The compiler check, release build, and 18 contract checks pass. The frozen driver SHA256 is `1E2D7569CC9924506D276F0AB5B14F1D55DF17461877488E6AF689DC7A8681D8`. The full non-panic audit is running at query 1000000, scalar 100000, alternatives 128, call depth 256, function scratch 16 MiB, query scratch 256 MiB, process memory 10240 MiB, and timeout 1200 seconds.

Stage33 adds bounded struct, tuple, and enum return values to effect summaries. Known fields and variants can be read without a callee frame. Unknown fields stay unknown. It also preserves supported symbolic integer relations and same-object pointer equality through arithmetic summaries. The expression bound remains 32 nodes. The compiler check and release build pass. The frozen driver SHA256 is `2F7C523D849837A96BD90525CF3A0727A476F71FB3922CB654B8E4FF56C424F2`. Contract checks are running. General recursive memory effects and removal of old execution histories remain incomplete.

Stage33 passes all 18 contract checks. Stage34 composes fixed field reads with entry-memory loads, and removes literal constant switch edges before effect-summary construction. Compiler runtime-check switches use the actual compiler setting. This avoids requiring summaries for branches already proved impossible. The compiler check, release build, and 18 contract checks pass. The frozen driver SHA256 is `656DFD0EB45C23954542E04175C3D8CE0A9DD97A0498DEE3A87246C288B51090`.

Stage35 replaces the separate control transfer inside memory queries with the bounded control templates. Callee return dependencies use the memory worklist when that worklist is active. Thus pending value reads remain dependencies of that worklist; they do not become completed control-cache answers. Loop entry relations come from the templates. An unresolved back edge must not be marked as iteration zero. The compiler check passes. The release build and focused validation are pending. This is a migration candidate, not an accepted result.

Stage32 finishes the full audit with 111 of 113 non-panic cases and 45 of 45 cache checks. Standard cases pass 92 of 94 in 528.40 seconds, including build work. Extra cases pass 19 of 19 in 155.13 seconds. The only failures are ordinary formatting negative and recursive local-callback negative. This does not establish a performance improvement or restore the original local-callback result at scalar allowance 100000. Reports are in `target/control-summary-20261020/stage32/`.

Stage35 passes the release build and all 18 contract checks. Its frozen driver SHA256 is `9C0608CF0FE7CD30ED264514044944E873C905F11294630691561CCF31FCA311`. Focused validation is pending. Stage34 is checked first at equal limits to separate its effect-summary changes from the new control integration.

Stage36 adds byte views for equal-size pointer casts and transmutation, and composes the existing pointer-offset intrinsic in effect summaries. This is needed by ordinary pointer wrappers such as `NonNull::add`. The summary does not infer a new allocation or a new pointer binding from raw integer bytes. Unsupported byte reads remain unknown. Compiler checking passes; the release build is running.

Stage34's focused audit is stopped after global-update safe and negative regress. The safe case reports two transfer-limit Unknown errors and uses 18003845 transfers. Local-callback negative also takes much longer than the prior version; its interrupted run is not accepted. No scenario result or annotation is changed. The global-update logs are in `target/scenarios/20261020-220152-3604484/`.

The aggregate summary can know a variant while lacking one of its fields. The return fast path did not distinguish those requests. Stage37 adds an explicit read-coverage check before summary application. A known enum tag cannot supply an unknown callback field. Unsupported projections remain with detailed refinement. This is separate from the meaning of a completed conservative unknown value. Stage36's frozen driver SHA256 is `83DA40122E6B714F9C13E438105CD9ADE93D8772E8241FAD6E58508B4ED3519B`; it was not scenario-audited.

Stage37 passes 13 of 15 focused checks at the original limits. Global-update and boot-registry pass safe, negative, and cache checks. Recursive formatting also passes. Local-callback negative and ordinary formatting negative fail. Global-update safe uses 811583 transfers and 31 frames. Ordinary formatting uses 6155639 transfers and 134 frames. This candidate also contains the control integration and byte views, so this is not a one-change attribution of the restored global-update result. The frozen driver SHA256 is `3E7EE97AD996F7F54CFFEB4E26C522EB72DF6A8798203EA44ED58987C77E1F03`.

Stage38 distinguishes coverage of an enum-tag read from coverage of all stored fields. A store summary requires coverage of its represented fields; knowing only the variant is not sufficient. The compiler check and release build pass. The frozen driver SHA256 is `FCF9266E51945D42C7BB9F5361F51081529A3BDD633412A755864EB2771E6F8E`. Contract checks and focused checks at scalar allowance 1000000 are running. No new kernel run has been made in these stages.

Stage38 passes 20 of 21 focused checks at scalar allowance 1000000 and all 18 contract checks. Only ordinary formatting negative fails. Its trace shows that the iterator constructor summary has no expression for either pointer field. The iterator `next` summary is unsupported. Formatting uses 8745468 transfers and 134 frames and reports three unresolved control paths. The full non-panic audit is running at the same limits. Focused traces also enabled `IRQ_CHECK_TRACE_CONTROL=1` and `IRQ_CHECK_TRACE_EFFECT=core::slice::Iter`; the original stage38 settings file did not record these inherited variables.

The next candidate adds summary expressions for pointer casts, slice metadata, and a reference to an already referenced object. For example:

```rust
fn end(bytes: &[u8]) -> *const u8 {
    bytes.as_ptr().wrapping_add(bytes.len())
}
```

The summary retains the parameter pointer and length. It does not read each byte or infer pointer provenance from an integer. Equal-size byte views remain separate from pointer casts. All expression and resource bounds remain enabled. Compiler checking passes; release compilation is running. Recursive memory groups and removal of old history machinery remain incomplete.

Stage39 passes release compilation and all 18 contract checks. Its driver SHA256 is `0507507366B04BF4517DC35C91592FBA939B6C3AC6E25E210FC7EFF6423DCAC2`. Focused scenario validation is pending.

Stage40 adds bounded entry-memory reads through pointer expressions. It composes field reads from those loads and preserves the rule against new memory loads after an earlier store. Pointer reborrows can retain a parameter-relative store destination. Conditions already known false do not require evaluation of later conditions. Compiler checking and release compilation pass. The driver SHA256 is `86DD431AC05AADEBA481F265EBD5F4223C703E85CC524F6DBFB71AAB1BF4F268`. Contract checks are running. Scenario checks still use frozen stage38 until that full audit finishes.

A new stage38 x86 kernel check uses query and scalar allowances of 1000000, timeout 1200 seconds, and the 10240 MiB process cap. Its separate build directory is `target/kernel-summary-cargo`. Its result directory is `target/control-summary-20261020/stage38-1200-x86_64-uefi-20261020-221818-9305657/`. It runs concurrently with the scenario audit in a different build directory. Kernel peak private memory is sampled separately. The old sampler can include other concurrent drivers in its whole-driver CPU and peak fields; those fields must not be treated as kernel-only measurements for this run.

Stage38 finishes the standard audit with 93 of 94 cases and all 45 cache checks passing. Only ordinary formatting negative fails. The run takes 538.25 seconds, including build work. The extra group is still running. Stage40 passes all 18 contract checks. No kernel source is changed.

Stage38 finishes the extra group with 19 of 19 accepted cases in 309.91 seconds. The combined result is 112 of 113 non-panic cases and 45 of 45 cache checks. The large writer negative passes but takes 79.49 seconds and 9051380 transfers. Stage32 took 2.10 seconds and 965678 transfers at scalar allowance 100000. Stage38 uses scalar allowance 1000000 and new control integration. This is not an equal-limit attribution. The added work must not be presented as a performance improvement.

Stage39 passes 14 of 15 focused checks; stage40 passes 20 of 21. Both fail only ordinary formatting negative. The stage39 trace confirms that both iterator-constructor pointer fields now have summary expressions. Iterator `next` remains unsupported. Stage41 retains stage40 value semantics and records the control-refinement stop reason for its diagnostic. A fixed-size field carries this reason; it does not enter the value lattice. The compiler check, release build, and all 18 contract checks pass. Its driver SHA256 is `48D66EC8F2AA1285B65DE64DD5037FF25A1074D61B76E24889EC10C81C876703`. Panic-only checks are running at scalar allowance 1000000.

The next candidate represents a shared local borrow as a captured value while composing a callee summary. Loads through this borrow use the captured value. A store through the borrow, or a pointer result that would expose the local object, remains unsupported. This addresses the local borrows used by pointer-wrapper equality. For example:

```rust
fn equal(first: core::ptr::NonNull<u8>, second: core::ptr::NonNull<u8>) -> bool {
    first == second
}
```

The summary compares the two parameter values without allocating local objects for the comparison's shared references. New memory loads after stores still remain unsupported; a read of an already captured local value is distinct from such a load. Redundant nested pointer and byte views are reduced before the 32-node bound is applied. Compiler checking passes. Release compilation is running.

Stage42 passes the release build and all 18 contract checks. Its driver SHA256 is `479036348FEC284A74555DABEDF8A677C268DFA87B6B51A8087F5AE17C7A5EAA`. Focused checks are queued after stage41's panic-only check. The stage38 x86 kernel check remains in progress. Neither pending run is accepted. The README now describes the implemented summary layers and states that recursive memory-effect migration and removal of old memory histories remain incomplete.

Stage41 passes panic-only safe, negative, and cache checks. The negative case reports one Forbidden diagnostic and no Unknown diagnostic. Its route includes `panic_bounds_check`, `panic_handler`, and `PanicInfo::fmt`. The main query group uses 172144991 transfers and 607 frames. It stops after the established forbidden path (`diagnostic_cutoff=true`), not after complete exploration of all routes. The log is `target/scenarios/20261020-223157-3835727/recursive-bindings-negative.log`. This does not establish acceptable analysis cost. Stage42 focused checks have started.

Stage42 fails global-update safe and negative, boot-registry safe, and ordinary formatting negative. Other focused cases pass. This candidate is not accepted. The global-update trace reaches AlternativeLimit in the iterator result. A separate insensitive read reaches 128 unguarded pointer values. The detailed trace is in `target/scenarios/20261020-224458-1236824/`. That trace also enabled `IRQ_CHECK_TRACE_FLOW_VALUES=1`; its settings file did not record this variable. The runner now records it.

Stage43 adds a value-choice relation to entry-memory loads in effect summaries. The same singleton storage read at the same call point must select the same value in a condition and its effect. For example:

```rust
if iterator.current != iterator.end {
    iterator.current = iterator.current.add(1);
}
```

The condition and addition must not select different values for `iterator.current`. The relation uses the existing typed choice guards. It does not constrain recursive summary objects or the insensitive approximation. Two summary load forms use the same implementation. Compiler checking and release compilation pass. Focused validation is running. This is not yet proof that the regression is fixed.

The stage38 x86 kernel check completes with 129 Unknown diagnostics: 115 unresolved control paths, 12 pointer transfer limits, and two external assembly gaps. There is no Forbidden diagnostic. The evidence reports complete diagnostics, not an analysis timeout. Total time is 1223.27 seconds including build work. Sampled kernel peak private memory is 1984589824 bytes. The whole-driver CPU measurement includes concurrent scenario drivers and is not a kernel-only measurement. No kernel annotation is added for the unresolved control or pointer limits.

The stage38 ARM measurement runner stops when a sampled process exits before its command line is read. The checker process continues, but that run has no valid complete memory measurement. The runner now checks for a missing command line and selects the frozen driver executable. It no longer assumes that xtask preserves the supplied Cargo target directory for the nested kernel build. Preserve the interrupted run and distinguish its evidence from a completed measurement.

Stage43 passes all 18 contract checks but still fails global-update and boot-registry safe. Choice relations alone do not fix the regression. Stage44 uses the same thin-pointer identity as the existing memory choices, so metadata does not create conflicting identities for the same address. Stage44 passes compilation but has no scenario audit. Its driver SHA256 is `F5B53C5911B5CB52350F23B7D6B8B8B1C9CF4986FA36BA3C5A0516A110A1ADC6`.

Stage45 removes the surrounding alias filter while evaluating a summary condition, then restores it before applying the write. A condition needs full pointer offsets even when the write query only needs object aliases. The detailed scalar path already separates these modes. Stage45 passes compiler checking and release compilation. Its driver SHA256 is `CDE484A6E4E41533A16530639BACD7F187B2D8E67241A81DB7E125D257100A09`. Validation is pending.

The x86 kernel migration adds `allow_incomplete` only around `CpuId::new`, the native `get_feature_info` call, and `Cr3::read` in `current_root`. Review used raw-cpuid 11.6.0, x86_64 0.15.5, and the current compiler's CPUID implementation. CPUID reads registers through the native reader; CR3 reads the page-table register. Neither assembly block has an indirect call. The caller's `expect` and address conversion remain outside the marks. Unknown returned values and effects are not changed to known values. No mark accepts the control or transfer-limit failures. Before-edit kernel files are saved in `target/control-summary-20261020/kernel-before-external-marks/`. No source comments are added. Kernel validation of these edits is pending.

Stage45 passes all 18 contract checks but still fails global-update at equal limits. Stage46 adds a trace of at most 64 summary-expression evaluations per solver. Stage47 also bounds pointer offsets in insensitive summary evaluation. The detailed solver already broadens offsets in this mode, but summary composition exposed a constant step and repeatedly produced exact addresses. The stage42 value trace contains offsets through 1536 bytes. Object-only results are sufficient for the coarse alias query and do not prove a callback binding. An alias query for a different object also skips offset evaluation. Stage47 validation is pending.

The interrupted ARM check is stopped after verification of its original Cargo process and command line. The measurement runner had already exited and closed its output stream. Its partial files remain available. It is not an accepted kernel result and must be rerun with the corrected runner.

Stage47 passes all 18 contract checks but has the same focused failures as stage45. Global-update safe uses 7620568 transfers. The coarse offset expansion is bounded, but the required binding still reaches AlternativeLimit. The trace confirms that the first precise comparison has the expected start and end addresses. This is not a complete root-cause result. Stage47's driver SHA256 is `02149107F6B32BA16569480FECD9740A0555298457D911BD4468ADE88025A90D`.

Stage48 changes only the diagnostic filter. It records non-empty multiple-value or uncertain expression inputs, instead of using the trace allowance on pending empty inputs. Its driver SHA256 is `C9729BD2E9B58096725E8BB7B2AB133096E0EDE3D7509C8110703F15CDC7B96F`. A focused global-update trace is running.

Stage49 evaluates one more coverage distinction. An empty enum variant has a known tag and no active payload. Its inactive payload can be represented as a guarded unknown, distinct from a missing summary for an active callback field. This lets the iterator tag and payload use the same summary conditions. The existing coverage check still rejects an active field with no expression. Compiler checking passes; release compilation and scenario checks are pending. The stage38 ARM check has restarted in `target/control-summary-20261020/stage38-1200-aarch64-uefi-20261020-225502-3018050/` with the corrected measurement runner.

Stage49 passes release compilation and all 18 contract checks. Its driver SHA256 is `9AC845F555450F0E679BBF744B353E440F47F696267A1E6BFD37EF8A971FDFD1`. Global-update safe still fails with AlternativeLimit, using 7522392 transfers and 21 frames. Boot-registry safe also fails. Local-callback and recursive formatting pass all three checks. Ordinary formatting is still running. Thus uniform tag and payload summary selection is not a complete fix.

Stage50 uses the compiler's actual `Operand::RuntimeChecks` value in the normal-return graph. The graph previously followed both edges, including disabled checks. The effect-summary builder already uses this compiler setting. Compiler checking and release compilation pass. The driver SHA256 is `D3E2BD885AAECBBEE0CEEA9C487A372D60AB5831F2D5D1413C9E2518DBEE9377`. Contract validation is running.

Stage51 changes only the expression trace. It reports an unknown result after guard merging, including the precise input pair. Mixed input sets alone do not prove an invalid pairing: the stage48 trace shows conflicting input and value-choice guards that correctly exclude some cross-allocation pairs. Do not report those pairs as established incorrect relations. Stage51 compilation passes; its release build is running.

Stage50 passes all 18 contract checks. Stage51 completes compilation and still fails global-update. Its driver SHA256 is `249A9AD1E03CCD394731B01EFC1FB0AE54FD687B7BEB512ED605B8EA423CF403`. The trace in `target/scenarios/20261020-230715-0191707/recursive-bindings-safe.log` now shows a pair that survives guard merging: iterator start `alloc22 + 0` and end `alloc29 + 24`, with iteration zero. These fields must derive from the same captured slice argument. Their unrelated allocations make the equality unresolved and admit further iterator states. This is a concrete lost relation; it is not yet a complete explanation of all formatting or kernel failures.

Stage52 applies the summary load relation to actual arguments as well as pointee loads. It passes compilation and all 18 contract checks, but global-update still fails with the same cross-allocation pair. The argument capture still used the blanket exclusion for recursive frames.

Stage53 permits the relation for an argument stored in a local of the current call frame. It does not enable it for arbitrary recursive memory loads, prior activations, or summary objects. Existing recursive entry and exit remove guards owned by the recursive component. For example:

```rust
let start = slice.as_ptr();
let end = unsafe { slice.as_ptr().add(slice.len()) };
```

Both expressions must use the same call's selected slice value. They must not select one recursion's start and another recursion's end. Compiler checking passes. Release compilation and focused validation are pending.

Stage52 and stage53 each pass all 18 contract checks but still fail global-update. Stage53's driver SHA256 is `37B1DFD8D20395879C48857A3B646672EA4BF7C9D6733B681213CF2E68376AFC`. Its trace still contains the cross-allocation iterator pair before the main query's limit. The additional argument relation is insufficient. Stage54 adds bounded choice-identity tracing and records the caller MIR. It has no intended value-semantics change. Compiler checking and release compilation pass. The next trace must identify the source of each field before another semantic change is accepted.

Stage54 still fails global-update. Its driver SHA256 is `456B3B1B3F8F5163C72AA791D38B79E8C6A2E19DD586549BA80B8F40AE9FCF42`. The caller constructs the iterator at `bb5[13]` from `_30`, a reborrow of the captured registry slice. The trace maps choice identities to that local argument. The cross-allocation pair remains, so argument capture alone is not the complete repair.

Stage55 avoids a condition read for a summary store whose destination does not overlap the requested field. It keeps each destination's guards on the unchanged result. It does not assume an empty or pending destination set is complete. This removes a condition dependency from reads of the iterator's unchanged end field. Compiler checking passes; release compilation and focused checks are pending. The expression trace also records the separate input guards.

The corrected stage38 ARM run finishes with an analysis timeout, not completed diagnostics. Evidence has `incomplete=true` and `diagnostics_complete=false`. Its detail is `the analysis time limit was reached; no partial result is accepted`. Total time including build work is 1318.99 seconds. Sampled kernel peak private memory is 1796071424 bytes. The four main query groups use 196540171, 177194065, 415979987, and 750102080 transfers. The 304 recorded precision-loss entries are partial evidence, not a complete diagnostic count. No timeout or local resource limit is accepted through an annotation. No check boots the kernel.

Stage55 passes compilation and all 18 contract checks but still fails global-update. Its separate-operand trace shows the start field coming from the constructor summary, while the end field carries detailed iterator-constructor frame guards. The end field is stored as `*const T`, but pointer comparison reads the same bytes as `*mut T`. `Source::Pointer::covers` required exact type equality, so it rejected only that field and used detailed refinement. The start and end then lost their shared argument relation across this mixed path.

Stage56 accepts a full pointer read with the same pointee type and representation when only the pointer view differs. It applies the same compatibility rule in coverage and value evaluation. It does not infer an address from integer bytes, alter metadata, or accept a partial pointer read. Compiler checking passes; release compilation and regression validation are pending. This targets the observed split, but no all-pass result is claimed.

Stage56 passes release compilation, all 18 contract checks, and global-update safe, negative, and cache checks at unchanged limits. Its driver SHA256 is `91D655459C804BC924EE9512C4EA977BA05407E00B0B338FF2D2AB9745968532`. Global-update safe's main query group uses 453837 transfers and 15 frames, with zero unknown coverage. Its two separate root-witness groups use 453854 and 453811 transfers. The cross-allocation comparison is absent from this trace. Logs are in `target/scenarios/20261020-232128-4080667/`. The remaining focused checks run under `stage56-focused`. This restores one regression; the full migration and audit remain incomplete.

Stage56 finishes the focused checks with 20 of 21 accepted results. Only ordinary formatting negative fails. Raising function scratch from 16 to 64 MiB and query scratch from 256 to 1024 MiB does not fix it. The stop changes from ScratchLimit to AlternativeLimit. That comparison is saved in `stage56-scratch64`; all other configured limits are unchanged. A separate value trace is in `target/scenarios/memory-20261020-232953-1925175/`. Its alternative stop is in the control-return join for `Buffer::write_str`. The full stage56 non-panic audit runs with the original 16/256 MiB scratch limits. The new x86 kernel run remains pending.

Stage57 merges equivalent return-path alternatives before applying the alternative-count limit. It retains the guard-count limit and adds a trace that identifies that stop. No uncertainty is converted to a proved path. The compiler check and release build pass. The frozen driver SHA256 is `2C48262FB9DB6D89DE8055A5800E5E8C34468B5FDC7F3AC08E86E58F2223402C`. Its source snapshot uses flat file names in `stage57/source`. The formatting comparison uses the same 64/1024 MiB scratch settings as the stage56 probe, not the full-audit settings.

The next candidate separates assembly memory effects from assembly call coverage. RUPTA-derived coverage already uses `nomem` and `readonly`, but the memory-effect summaries rejected all assembly. The summaries now preserve unrelated memory when the assembly contract forbids writes and no output writes through a pointer. Register output values remain unknown. Parameter-effect composition also rejects projected outputs and outputs to address-taken locals. For example:

```rust
unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
callback();
```

The assembly still has an unchecked call-path diagnostic unless the existing annotation contract accepts it. Its memory contract does not erase the callback binding. No return or termination proof is added for assembly. The reference is [Rust inline assembly](https://doc.rust-lang.org/reference/inline-assembly.html). Compiler checking passes; release compilation and validation are pending. General recursive memory-effect groups and removal of old memory histories remain incomplete.

Stage57 still fails ordinary formatting. The new trace identifies 181 constraints on one return alternative, not 181 alternative paths. The control join compares that constraint count with the limit of 128 alternatives. This is a distinct limit from the live-storage limits. Stage59 replaces that check with the configured per-function storage bound for the guard vector. The alternative bound still applies to alternatives after merging. Transfer, refinement, query-scratch, process-memory, call-depth, and time limits remain enabled. The change does not discard a constraint or mark a path as proved. Stage58 is frozen before this change; its driver SHA256 is `2BE7DD56D09E3EAD8DE92D8AF3AA1ECB9348DF293C44D237F63E0BE76250A8DB`. Stage57's measured formatting log is `target/scenarios/memory-20261020-233249-6037690/stderr.log`.

The computer restarts before the next work session. Source and release identity checks confirm stage59. Stage56's standard audit finished with 93 of 94 accepted cases and 45 of 45 cache checks. The extra audit retained 18 passing records but no final report. The missing `writer-growth/negative` case is rerun at the same limits. It reports one Forbidden and no Unknown, with `incomplete=false`. The checker stops after the established forbidden path, so `diagnostics_complete=false` is expected for this negative. The resumed result index is `stage56/extra-resume-results.json`. Stage56's combined non-panic result is 112 of 113, plus 45 of 45 cache checks. Only ordinary formatting negative fails.

The stage56 x86 kernel run finishes with 143 Unknown errors and no Forbidden error. The evidence has `incomplete=false` and `diagnostics_complete=true`. Total elapsed time is 924.06 seconds; sampled kernel peak private memory is 2017456128 bytes. The diagnostic notes identify 63 alternative-limit stops, 56 index-split results, one scratch stop, and 19 unresolved control results without a local stop reason. Four other Unknown errors require separate classification. These counts describe this run only. The two reviewed assembly gaps are no longer reported. The first stage59 ARM run fails during host compilation with compiler exit code `0xc000026b`, before the checker analyses the kernel. Its zero sampled kernel memory is not an analysis measurement. ARM is restarted in `stage59-1200-aarch64-uefi-20261021-000346-3755666/`.

Stage58 passes all 18 contract checks. Stage59 passes 20 of 21 focused checks after restart. Ordinary formatting still fails. Its separate 64/1024 MiB scratch probe reaches the per-function alternative-storage limit at 67126848 estimated bytes. Most retained guard entries are value-choice relations. The probe log is `target/scenarios/memory-20261020-233730-2106503/stderr.log`. Stage59's driver SHA256 is `939BF69A4B62F05DBEC9D075C9A5950DE39DA93F733BDC005789F4879C7D5A5B`.

Stage60 omits a new choice relation only when a completed read has one known value across all alternatives. It keeps all existing guards. Pending reads and reads with different values retain choice relations. Consumers that use this single-value result are marked for invalidation when call inputs change. Both detailed reads and summary loads use the same check. Compiler checking and release compilation pass. Its driver SHA256 is `447C12C86240AFAABD69422E29C0FA4EBE54B7C625B2C292B046CD59322D7BEC`. Focused checks are running.

Stage61 preserves `SplitIndex` across the control-summary boundary. This result requests explicit array-index cases from the existing outer query loop; it is not a completed uncertain answer. The previous catch-all handler converted it to a coarse result and did not let the split handler act. The correction is:

```rust
Err(reason @ (QueryStop::ProcessLimit | QueryStop::TransferLimit
    | QueryStop::ContextChanged | QueryStop::SplitIndex)) => Err(reason),
```

All split cases remain under the current query allowance and scratch limits. No allowance is reset. Compiler checking passes; release compilation and scenario validation are pending. The complete migration remains unfinished.

Stage60 passes all nine focused global-update, boot-registry, and recursive-formatting checks. Stage61 passes 20 of 21 focused checks and all 18 annotation-contract checks. Ordinary formatting negative remains the only focused failure. Its driver SHA256 is `837A1891E5EBEFA91D1CD2DBAE10BAC361578A241D45A0CC11B4BB5ED38BDAB8`. Its full non-panic audit and subsequent panic-only checks are running. A separate formatting probe uses query allowance 5000000, scalar allowance 1000000, function scratch 256 MiB, and query scratch 2048 MiB. The 1200-second time limit and 10240 MiB process cap remain enabled. That probe is not yet an accepted result.

Stage62 removes the separate coarse solver and its copied caller frames. The stage61 ordinary-formatting trace records 229 coarse preservation attempts: 189 stop at RefinementLimit, 36 return false, and four request SplitIndex. None proves preservation in that run. This is evidence for removing that work, not proof that the replacement passes all cases.

Memory queries now use backend write regions, read-only summaries, and parameter-effect summaries before bounded binding refinement. The safe-target fallback uses complete backend candidate coverage only:

```rust
if self.facts.covered_calls.contains(&key)
    && let Some(targets) = self.facts.safe_candidates.get(&key)
        .filter(|targets| !targets.is_empty())
{
    return Ok(targets.iter().copied().collect());
}
Err(QueryStop::Unsupported)
```

The caller can use this set only to prove that all covered candidates are safe. It cannot report Forbidden from a candidate alone. Required binding work still has its detailed, bounded path. General recursive value/effect groups and removal of the remaining memory histories are not complete.

Stage62 passes compiler checking, release compilation, and all 18 annotation-contract checks. The compiler reports one unused allowance helper left by the removed solver. Its frozen driver SHA256 is `7AAAB716FB2021CC5B8AFB5A5C26700DF16CC9A421D1FE9AEE5385F7455F8AE4`. Focused checks run in `target/solver-refactor-extra-fast-cargo`, separate from the stage61 full audit. The existing scenario runner now accepts an optional `CargoTargetDirectory`; its default is unchanged. No new test or expected result is added.

Stage61 finishes the full non-panic audit with 112 of 113 cases and all 45 cache checks passing. Standard cases take 466.97 seconds and extra cases take 272.08 seconds, including build work. Panic-only validation follows that audit. Stage62 finishes the focused checks with 20 of 21 accepted results. Its ordinary-formatting negative uses 7880211 transfers and 593 frames and reports three Unknown errors. Its x86 kernel check starts in `stage62-1200-x86_64-uefi-20261021-002237-9601883/`.

Stage63 removes the separate memory control-path cache and builder. Memory path conditions now use the bounded structural control builder. The cache key distinguishes a memory path from a control-return request. A memory path does not itself prove a callee's return. Branch assumptions remain constraints, and widened paths retain an unresolved marker:

```rust
if memory {
    return if refine { self.feasible(coarse) } else { self.bounded(coarse) };
}
```

Compiler checking, release compilation, and all 18 annotation checks pass. The unused allowance helper is removed. Stage63's driver SHA256 is `939067DCC185701609EF5981BFE5D4733261146CEFD9448DD564486D8C1EC149`. Focused checks are pending; global-update, local-callback, and both cut configurations pass so far. Boot-registry takes longer than the preceding candidate. This is not an accepted performance result.

Stage64 keeps the shared builder but leaves memory iteration selection with memory SSA. The control template must not impose a second iteration-zero relation on those reads. Memory back edges retain the scoped widened loop guard; control-return templates retain their existing incoming-edge relations. Compiler checking passes. Release compilation and equal-limit validation are pending. This is an invariant correction found during source review, not yet a proved cause of the stage63 cost increase.

Stage63 finishes with 17 of 20 focused results accepted. Boot-registry safe and negative regress; the safe failure prevents its cache check. Ordinary formatting negative also fails. All other focused cases pass. Stage64 passes release compilation and starts the same focused matrix. Do not accept the shared-path change until these regressions are resolved.

The stage59 ARM kernel run stops at the 1200-second analysis limit. Total time including build work is 1324.57 seconds. Sampled kernel peak private memory is 1831272448 bytes. Its evidence has `incomplete=true` and `diagnostics_complete=false`; no partial result is accepted. The completed run directory is `stage59-1200-aarch64-uefi-20261021-000346-3755666/`.

After that run finishes, the ARM migration adds a narrow `allow_incomplete` block around `TPIDR_EL1.get()` in `kernel/src/arch/aarch64/cpu.rs::per_cpu_ptr`. Review of aarch64-cpu 11.2.0 confirms that this method expands to one `mrs` instruction with a register output and `nomem, nostack`. It contains no call or memory write. The cast remains outside the block. Unknown register data is not converted to a known pointer. The before-edit file is `kernel-before-external-marks/aarch64-cpu.rs`. This annotation does not accept the control, transfer, or time-limit failures. Kernel validation is pending.

Stage64 restores boot-registry safe, negative, and cache checks. Its main safe query uses 486913 transfers, compared with stage63's failed 9877630-transfer run. Both use the same configured limits. The memory iteration change is the only semantic difference between these candidates. The trace is `target/scenarios/20261021-002649-4325324/recursive-bindings-safe.log`. Stage64 passes 20 of 21 focused checks and all 18 annotation checks. Ordinary formatting negative still fails with three scratch-limit Unknown errors, 8537500 transfers, and 569 frames. Stage64's driver SHA256 is `D8FFCCF77D1156DFA69FFC687A3A1B90BD0AD93BD5A41B249F7EF5D44E7B0557`.

Stage65 removes the unused coarse-mode field and its dead branches. No code can set that mode after stage62 removes the separate solver. Call contexts, value guards, and dependency generations now use the retained precise mode directly. Compiler checking and release compilation pass without warnings. Focused validation is running. A new full audit will use the separate `target/solver-refactor-extra-fast-cargo` directory; the stage65 audit runner accepts that directory explicitly. The previous audit remains unchanged.

Stage65 passes 20 of 21 focused checks and all 18 annotation checks. Only ordinary formatting negative fails. Its driver SHA256 is `A43F8114AF1F21CC32AF65CB679E52054C3D49804D5474F8AD2D85AF3F7C2C87`. The full audit is running, with panic-only checks last. The ARM run with the reviewed register-read annotation starts in `stage65-1200-aarch64-uefi-20261021-003029-7052620/`. The older stage62 x86 comparison remains in progress.

The stage61 larger formatting probe reaches the 1200-second analysis limit. Total time is 1226.53 seconds, including build work; sampled checker peak private memory is 1237782528 bytes. It uses 34994741 transfers and 1224 frames. The query allowance of 5000000 and scratch limits of 256/2048 MiB do not produce a complete result. Its log is `target/scenarios/memory-20261021-001003-0324523/stderr.log`. It is a failed time-limit probe, not a memory-cap failure or proof that larger limits fix formatting.

Stage61 passes panic-only safe, negative, and cache checks after the full audit. The negative log is `target/scenarios/20261021-002403-1361967/recursive-bindings-negative.log`. It reports one Forbidden error and no Unknown error. The main query group uses 200278136 transfers and 840 frames, with `diagnostic_cutoff=true`. The two separate root-input attempts each use 1000000 transfers. This confirms the established panic route but does not establish acceptable cost. Stage65's panic check remains queued after its full audit.

Stage65's formatting cost trace identifies a per-function alternative-storage stop for `Buffer::write_str`. At the first stop, all 11256 allocated demands are in the active dependency set. Thus unrelated cached demands do not explain that first stop. The log is `target/scenarios/memory-20261021-003154-5124238/stderr.log`. This run takes 76.97 seconds including build work and reaches 181084160 sampled private bytes. A separate MIR trace uses the runner's explicit `TraceFunction` parameter and is saved in `target/scenarios/memory-20261021-003349-1465667/stderr.log`. The first runner removes the inherited `IRQ_CHECK_TRACE_FUNCTION` variable, so its log is not a MIR trace.

Stage66 removes a caller selection only when the frame has exactly one registered caller and is not also an IRQ root. The sole valid selection is zero. Memory and control cache keys omit that redundant selection. Guard merging removes its tautological input guard and rejects a nonzero selection. For example, a key need not retain `inputs: [(caller_frame, 0)]` when no other input exists. Adding a second caller advances the dependency generation and invalidates the old read/control keys. Multiple callers and root/caller combinations retain their selections. The owner, index, loop, and context rules remain unchanged. Compiler checking passes; release compilation and equal-limit validation are pending.

Stage67 checks a constant-size array's full byte range before evaluating an indexed write destination. The previous code evaluates the index before checking overlap with the requested bytes. For example:

```rust
buffer.bytes[index] = byte;
let used = buffer.used;
```

If `bytes` occupies `[0, 512)` and `used` occupies `[512, 520)`, no valid array index can make that write overlap `used`. The new check returns an unchanged-memory effect without reading the index. It applies only when every known base is disjoint and no later projection dereferences a pointer. Unknown bases, overlapping ranges, slices, and arithmetic overflow retain the existing address analysis. This does not prove that an out-of-bounds operation returns; its MIR assertion remains a control dependency. No callback binding or forbidden-path rule changes. Compiler checking passes; release compilation and validation are pending.

Stage66 and stage67 each pass 20 of 21 focused checks and all 18 annotation checks. Ordinary formatting negative still fails. Stage67's driver SHA256 is `A814DBC1828CBD1C6B4D43238043F17AA092E0FC78CD49EEA7AC2BAC6ABCDEFF`. Its formatting log is `target/scenarios/20261021-004329-3507518/formatting-negative.log`: three scratch-limit Unknown errors, 8935841 transfers, and 569 frames. Neither change is a complete formatting repair.

Stage65 completes the non-panic audit with 112 of 113 accepted cases and 45 of 45 cache checks. Standard cases pass 93 of 94; extra cases pass 19 of 19. Only ordinary formatting negative fails. The panic-only check follows this audit and remains in progress.

The stage62 x86 kernel run reaches the 1200-second analysis limit. Total time, including build work, is 1303.10 seconds. Sampled kernel peak private memory is 1956085760 bytes. The log is `stage62-1200-x86_64-uefi-20261021-002237-9601883/check.log`. No partial diagnostic result is accepted. This is not a memory-cap failure. The stage65 ARM run remains in progress.

The stage67 effect trace is `target/scenarios/memory-20261021-004902-2138576/stderr.log`. The slice iterator has a parameter-effect summary. `Option::<&u8>::copied` does not, so `Copied::next` and `Bytes::next` use detailed queries. Result branching also has no effect summary. These are acyclic gaps before the loop boundary, not proof of a recursive-analysis defect.

Stage68 permits an explicit enum downcast followed by a field projection. It uses the selected variant's layout and retains the enclosing branch conditions. For example:

```rust
match input {
    Some(value) => Some(*value),
    None => None,
}
```

The payload read remains conditional on the `Some` variant. A MIR unreachable branch with no accumulated stores contributes no return value. If such a branch follows stores, the builder still rejects the summary. The change does not broaden callback coverage or discard unknown pointer effects. Compiler checking passes; release compilation and focused validation are pending.

Stage68 passes 20 of 21 focused checks and all 18 annotation checks. Ordinary formatting negative still fails. Its trace now contains a completed `Option::<&u8>::copied` summary. The driver SHA256 is `76BF3E233974F13024F11267CC3DC4AADB7F020EE0DA12D8B48DC02694E0D032`. The formatting log is `target/scenarios/20261021-005227-8521767/formatting-negative.log`.

Stage69 separates return-field coverage from store coverage during summary composition. After earlier writes, a new memory load cannot use entry memory. An aggregate can still retain its known tag and other captured fields when that load is unknown. For example:

```rust
update_storage();
let result = Some(*pointer);
```

The result can retain `Some`, but its payload must remain unknown unless the memory value is established. Store expressions and store conditions still require complete supported composition. The change does not accept unknown callback payloads. Compiler checking passes; release compilation and validation are pending.

Stage65 passes panic-only safe, negative, and cache checks. The negative log is `target/scenarios/20261021-004541-0623599/recursive-bindings-negative.log`. It contains one Forbidden error and no Unknown error. The main query uses 193803051 transfers and 848 frames, with diagnostic cutoff after the established forbidden path. The two separate root-input attempts each use 1000000 transfers. The panic check passes, but this does not establish acceptable cost.

The stage65 ARM kernel run reaches the 1200-second analysis limit. Total time including build work is 1298.97 seconds. Sampled kernel peak private memory is 2311589888 bytes. The log is `stage65-1200-aarch64-uefi-20261021-003029-7052620/check.log`. No partial result is accepted. The reviewed register-read annotation does not resolve the control and value-flow cost.

Stage69 passes 20 of 21 focused checks and all 18 annotation checks. Its formatting run still has three Unknown errors, with 8429441 transfers and 518 frames. `Bytes::next` now has a store summary, but no supported return expression. The log is `target/scenarios/20261021-005439-9574220/formatting-negative.log`. No all-pass result is established.

Stage70 adds a bounded shared calculation for parameter-effect summaries. Calls in the active group read provisional summaries. The group repeats until every evaluated summary is unchanged. Only that completed group enters the cache. A resource stop discards provisional results. The expression-domain bound also limits repeated calculation; a group that does not stabilize supplies no summary and still needs bounded detailed refinement. This does not establish normal return or safe call coverage.

An unconditional full overwrite removes an earlier store to the same symbolic destination and byte range:

```rust
recurse(slot, replacement);
*slot = replacement;
```

The final store can replace the same earlier store in the effect summary. Conditional stores, different ranges, and different symbolic destinations remain ordered. Loads after previous writes still need supported memory composition. Recursion with growing expressions can still require detailed refinement. General loop effects and final removal of memory histories remain unfinished. Compiler checking passes; release compilation and validation are pending.

Stage70 passes 20 of 21 focused checks and all 18 annotation checks. Its driver SHA256 is `74E92D2119B3E956CC9A1E73B1AD4A906CF01B449409B1080F9979595A452149`. Ordinary formatting still fails with three Unknown errors and 8431664 transfers. Its effect-summary counters report zero active recursive reuses in this case. Thus this run does not demonstrate a recursive-summary improvement. The full non-panic audit is running, with panic-only checks queued last.

Stage71 evaluates a 128-node bound for composed branch expressions. Individual operators and loads retain their 32-node bound. The branch-count, transfer, scratch, process-memory, and time limits do not change. The iterator's return branches and `Option::copied` branches need composition; keeping the known result tag must not require a known byte payload. For example:

```rust
let item: Option<&u8> = iterator.next();
let copied: Option<u8> = item.copied();
```

The two values have the same variant even when the byte cannot be established. This candidate also retains newly allocated group-summary bytes when a nested summary builder restores temporary scratch accounting. Compiler checking passes; release compilation and validation are pending.

Stage71 passes 20 of 21 focused checks and all 18 annotation checks. Its driver SHA256 is `C35A4D6DB30E0D01ABD49BA504905A5B4F968E9A71AB14870649A8996450CF52`. `Bytes::next` now retains its return tag and store summary. Formatting still reports three scratch-limit Unknown errors. It uses 7652140 transfers and 502 frames, compared with stage70's 8431664 transfers and 518 frames. The log is `target/scenarios/20261021-010244-1294106/formatting-negative.log`.

Stage72 simplifies composed enum-tag conditions. A discriminant of a conditional aggregate becomes the original conditions for the selected variants. It retains every such condition and does not supply a missing payload. For example, `item.copied().is_some()` can use the same variant conditions as `item.is_some()`. Branch alternatives remain bounded. Compiler checking passes; release compilation and validation are pending.

Stage72 passes 20 of 21 focused checks and all 18 annotation checks. Its driver SHA256 is `6CEA2651AECA04CD50222728547EBE5502319CE0AECC573F3B2AD4AECBB14B89`. Formatting still reports three scratch-limit Unknown errors, with 7303933 transfers and 502 frames. The summary-load count falls from stage71's 8896 to 5744. These are focused-run counters, not complete kernel measurements. The log is `target/scenarios/20261021-010743-8584949/formatting-negative.log`.

The matched formatting storage probe uses stage72 with function scratch 64 MiB and query scratch 1024 MiB. Query and scalar allowances remain 1000000. Its records are in `stage72-scratch64/`. The default-scratch x86 and ARM kernel comparisons also run with stage72, the existing reviewed kernel marks, a 1200-second timeout, and a 10240 MiB process cap. Their results are pending. The stage70 full audit remains in progress. The migration is not complete.

The stage72 larger-storage probe still fails ordinary formatting negative with three scratch-limit Unknown errors. It uses 14290612 transfers and 611 frames. The log is `target/scenarios/20261021-010846-2647079/formatting-negative.log`. More local storage does not resolve the remaining proof at these transfer allowances. The default settings remain unchanged.

Stage70 completes the standard audit with 93 of 94 accepted cases and 45 of 45 cache checks in 515.24 seconds. Only ordinary formatting negative fails. Its extra group and subsequent panic-only checks remain in progress.

Stage73 uses local inline-assembly acceptance in the instance and context safety summaries. These summaries previously rejected assembly even when its own unsafe block had `trusted` or `allow_incomplete`. For example:

```rust
#[cfg_attr(irq_check, irq::trusted(unsafe))]
unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
```

Only the assembly's call-coverage gap is accepted. Other calls in the helper still need safe coverage. Assembly outputs and memory effects remain unchanged, and acceptance does not pass to returned values. The RUPTA configuration and pointer facts are unchanged. Compiler checking passes; release compilation and contract validation are pending.

Stage73 is saved without scenario runs. Its driver SHA256 is `837548F38B7DA71E2A29640E6BE34E37C5E05B8DB186C59C08F486F184925994`. Review finds that the existing control-return shortcut also reads the safety set. Expanding that set must not add a new return assumption merely because an assembly gap is accepted.

Stage74 separates these uses. Call safety can use the local assembly acceptance. The existing return shortcut uses a separate set that excludes those accepted assembly functions and their callers. For example, accepting an assembly operation in `wait()` must not itself establish that `wait()` returns before a later forbidden call. The summary graph propagates this distinction through callers. Compiler checking passes; release compilation and validation are pending.
