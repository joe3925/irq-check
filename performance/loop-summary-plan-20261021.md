# Bounded loop correction

## Current state

Both stage160 kernel diagnostic runs are complete. The completion audit is target/control-summary-20261020/stage160/kernel-completion-audit.json. X86 takes 1210.341 seconds with peak private memory 2582548480 bytes. ARM takes 2731.142 seconds with peak private memory 2439020544 bytes. Both evidence files have incomplete=false, diagnostics_complete=true, and no whole-analysis limit detail. They include all 17 x86 and 15 ARM IRQ roots. Both compiler runs exit101 because each reports 33 Unknown errors. Neither is a clean kernel check. No kernel was booted.

The x86 error groups are 11 external assembly gaps, 14 local resource stops, and 8 summary or storage precision losses. ARM has 10 external assembly gaps, 11 local resource stops, and 12 summary or storage precision losses. Per-site records are kernel-diagnostic-review.json and kernel-arm-diagnostic-review.json in stage160. Resource and precision failures remain visible; they are not relabeled as proved inherent coverage gaps. Neither run reports a Forbidden error, and neither accepted-gap file contains an accepted established Forbidden target with unresolved reachability. No new kernel annotation was made for this comparison.

Validation for the same driver remains 113/113 non-panic cases, 45/45 cache checks, 21/21 focused checks, and 18/18 annotation checks. Panic safe and cache pass. Panic negative still fails with two Unknown errors and the known Forbidden error; six bound Forbidden::service edges remain. The panic-only fixture still has no early Task::fmt service call. Thus the complete-kernel-run milestone is verified, but a clean kernel check and an all-pass panic-negative result are not achieved. The prior migration and precision work is not declared fully complete by this result.

ARM session46762 is terminal, and PID42752 has exited. The ARM report is stage160/kernel-arm-results.json. Its evidence contains 141033 edges. The four main worker query totals are 28298, 36562, 34414, and 34740; transfer totals are 1056554748, 1136302571, 1160983935, and 1163661321. A second stack sample, kernel-arm-stack-2.txt, shows guard hashing and branch grouping in bounded during feasible_with_paths. The debugger explicitly detached and exited. Both short debugger pauses are included in ARM elapsed time. No run was restarted and no source or limit changed during the comparison.

At 21:57 UTC, ARM session46762 and PID42752 remain active. The checkpoint is stage160/kernel-arm-progress-review.json. The remaining worker has passed 33900 queries; the other three progress records are unchanged while their threads wait. Private memory is about 1.74 GB. Keep the same process under its original 4800-second limit. The x86 run is terminal and complete, with its 33 Unknown errors still visible. No source, annotation, expected result, or limit changed during these runs. The current verification does not establish a clean kernel check or an all-pass panic-negative result.

The completed x86 evidence identifies a repeated resource cost at MonoFont::glyph bb10[11]. There are 200 saved query records at that callback site. Exactly 64 records stop with TransferLimit, in 64 distinct contexts: 16 per worker. Each stopped record has steps=9000000, attempts=2, and cut_recursive_returns=false. These records account for 576000000 charged transfers. They are required callback-binding attempts, not runtime byte-loop iterations. This gives a concrete reason why the caller-context bound changes the total work. It does not prove that all those binding failures are necessary or that the first lost pointer relation is correct.

The x86 accepted-gap report contains 4307 records across roots and contexts. It contains no unresolved-reachability acceptance for an established Forbidden target. Existing marks accept 217 trusted assembly records, 2004 allow_incomplete assembly records, 1092 allow_incomplete virtual-call records, 993 allow_incomplete function-pointer records, and one trusted function-pointer record. These are not counts of distinct source operations. No new mark was added. All nine source snapshots and the stage160 driver hash still match. Focused checks are 21/21 and annotation checks are 18/18. Panic evidence retains six Forbidden::service bound edges; the panic-only fixture still has no early service call in Task::fmt.

At 21:42 UTC, ARM session46762 and PID42752 remain active. A native stack sample is saved in stage160/kernel-arm-stack-1.txt. The remaining worker is in targets, read_edge, read_uncached, feasible_with_paths, operand, and bounded, with QueryBudget::charge and reserve on the stack. Other threads wait for parallel work. This is one sample, not a full cost profile. The debugger explicitly detached with keep-stopped=false and exited; CPU time then increased. No debugger remains active. The brief attach pause is included in elapsed time. No source or annotation changed. The x86 typed debug-metadata callback must be reviewed separately from the AtomicUsize interrupt-control callbacks; it is not another integer-pointer conversion gap.

Stage160 x86 kernel analysis is complete. Session63993 is terminal, and PID30004 has exited. Evidence has incomplete=false, diagnostics_complete=true, an empty limit detail, 17 IRQ roots, and 145732 edges. Total measured time is 1210.341 seconds. Checker peak private memory is 2582548480 bytes. The run reports 33 Unknown errors and no Forbidden errors. It is a complete diagnostic run, not an accepted kernel build. The runner reports accepted=false because compilation exits101. The diagnostic review is stage160/kernel-diagnostic-review.json: 11 external assembly gaps, 14 local resource stops, and 8 summary or storage precision losses. Do not annotate the last two groups as if they were proved inherent coverage gaps. No kernel annotation or source change was made. ARM session46762, PID42752, remains active under its original limit.

The four x86 main workers report 34628, 35643, 34231, and 32730 queries. Their transfer totals are 1319901559, 1409647415, 1247745778, and 1632114168. The corresponding frame counts are 18675, 19124, 18298, and 17703; revision counts are 1, 1, 1, and 10. The previous stage159 x86 run timed out after 4800 analysis seconds. This result establishes that the 16-context candidate completes this diagnostic run at the same limits. It does not establish that all Unknown errors are acceptable or that the kernel is IRQ-safe.

At 21:25 UTC, both stage160 kernel sessions remain active: x86 session63993, PID30004; ARM session46762, PID42752. The early checkpoint is stage160/kernel-early-checkpoint.json. Kernel-process time is 354.693 seconds on x86 and 341.769 seconds on ARM. Private memory is about 2.25 GB and 2.27 GB. All eight workers advanced. ARM worker0 changed from query13100 with 6235 graph nodes to query18100 with 1839 nodes in the same process. Other workers also show rebuilt graphs. Thus the 16-context policy does not remove generation-based diagnostic graph repetition. This observation does not establish its total cost or the final result. Both original 4800-second limits remain in effect. No source change, annotation, or restart was made. The checkpoint records the driver hash and live process and worker states.

At 21:20 UTC, both stage160 main kernel analyses are active. X86 driver PID30004 has 17 IRQ roots; ARM PID42752 has 15. Both executable paths identify the frozen stage160 driver. All eight worker query files advance. The backend graphs match stage159: x86 has 46760 contexts and 617 coverage gaps; ARM has 42265 contexts and 638 gaps. These are backend gap counts, not final diagnostics. No completed kernel result is available. No source change, annotation, or restart was made.

Stage160 panic negative is terminal. Safe and cache pass. Negative reports two Unknown errors and the known Forbidden::service error, so the scenario still fails. The Unknown results are unknown storage addresses at Sink::write_str bb2[5], context252, and Argument::fmt bb3[10], context426. The diagnostic notes identify earlier analysis step limits. These results do not prove inherent coverage gaps. Evidence has incomplete=false, diagnostics_complete=true, and six bound edges to Forbidden::service. Main counters are 3347 queries, 94766107 transfers, 1172 frames, 15 revisions, 1898391 evaluations, and 908636 changes. No expectation or annotation changed. The retained-Forbidden launch gate is met.

Stage160 x86 and ARM kernel checks started at 21:17 UTC with the saved stage159 limits: 4800 seconds and 10240 MiB. X86 session63993 uses target/stage118-profile-cargo and logs to target/scenarios/memory-20261023-211720-7894439/stderr.log. ARM session46762 uses target/stage151-arm-kernel-cargo and logs to target/scenarios/memory-20261023-211735-3590169/stderr.log. Driver SHA256 is 046A6CDE3A8AE2667E1D39E94F50AE5E54E97CBB150969B8DDD2ECBA438085D7. All nine frozen source files match working source. The kernel worktree has the same 17 modified paths. No source change or new annotation was made for these runs. Keep each process under its original limit; do not restart it when an observation interval ends.

Stage160 passes all 113 non-panic cases and all 45 cache checks. Standard time is 464.581 seconds; extra time is 153.141 seconds. The 21 focused and 18 annotation checks also pass. Panic-format runs last. Safe and cache pass. At 21:13 UTC, negative is active with driver PID40756 in session7091. Its log is target/scenarios/20261023-211154-1571118/recursive-bindings-negative.log. Keep this process under its existing 1200-second limit. No stage160 kernel run has started, and no source or annotation changed during validation.

Stage160 passes all 94 standard cases and all 45 cache checks in 464.581 seconds. The result identifies driver 046A6CDE3A8AE2667E1D39E94F50AE5E54E97CBB150969B8DDD2ECBA438085D7. The local-callback negative log records widened=9 in each of its three analysis passes and is accepted. Thus this existing case uses the generic diagnostic entry under the new bound. At 21:10 UTC, the extra group is active in the same session7091. Panic-format follows only after all 19 extra cases pass. No kernel run has started. Source, expectations, and annotations remain unchanged during validation.

Stage160 passes all 21 focused checks and all 18 annotation checks. The annotation report is target/contract-checks/20261023-210046-5981371/results.json. All nine frozen source files match working source. At 21:02 UTC, full standard validation is active in session7091 on target/contract-checks-cargo. Extra cases and panic-format remain gated behind it. Kernel settings are prepared in stage160/kernel-settings.json and kernel-arm-settings.json at the stage159 limits, but neither kernel run has started. The known panic-route Forbidden result must remain before launch. No tests, source comments, fixture expectations, or kernel annotations were changed.

Stage160 is built and under validation. Compiler checking and release compilation pass. Driver SHA256 is 046A6CDE3A8AE2667E1D39E94F50AE5E54E97CBB150969B8DDD2ECBA438085D7. The only solver difference from stage159 is the diagnostic-context cap expression in analysis.rs. Value alternatives remain 128. At 20:59 UTC, focused validation is active in session7091 on target/contract-checks-cargo. Cut, unbounded-cut, and global-update safe, negative, and cache checks pass. The same session gates annotation checks, the full non-panic audit, and panic-format in that order. No kernel run is active. Do not start a stage160 kernel check before the known panic-route Forbidden result is verified.

Stage159 kernel validation is terminal on both targets. Both reached the 4800-second analysis time limit. X86 total measurement time is 4932.334 seconds and peak private memory is 3536850944 bytes. ARM total time is 4916.535 seconds and peak private memory is 3557031936 bytes. Both evidence files have incomplete=true, diagnostics_complete=false, and no accepted partial graph. These are checker resource failures, not established kernel coverage gaps. No annotation was added. Reports are stage159/kernel-results.json and kernel-arm-results.json. Sessions 55549 and 94007 are terminal; processes 42140 and 39408 have exited.

The ARM stack sample is stage159/kernel-arm-stack-1.txt. One thread was waiting at the parallel-analysis join. The other three worker stacks show alternative cloning in feasible_with_paths, guard hashing in bounded, and worklist filtering in read_edge. The first debugger launch failed before attach because python311.dll was not on PATH. A batch attach then exited without a backtrace; the target continued. An interactive attach supplied the saved sample and explicitly detached with keep-stopped=false. CPU time increased after detach, and no debugger remains active. The brief pauses are included in elapsed time. The final log contains the waiting worker's main counters: 189110 queries, 9500226408 transfers, and 81143 frames. The whole ARM analysis still timed out. Timing-site counts include resumed query segments; they are not counts of distinct caller contexts.

Stage160 will now run the recorded 16-context comparison. Keep all other stage159 solver behavior and validation limits. Frozen stage159 artifacts remain unchanged.

If stage159 reaches the existing kernel timeout without complete diagnostics, the next candidate will bound caller-specific diagnostic entries at 16 per function. The current diagnostic bound uses the value-alternative allowance, which is 128 in this audit. Further callers already use a checked generic entry with Unknown inputs; the next candidate changes only when that existing entry is selected. Keep the established target-query code, pointer-effect frames, bounded witness attempts, RUPTA information, incoming proof status, and annotation rules. Do not change the active stage159 processes or their frozen source. The initial comparison will use this expression:

```rust
.is_some_and(|inputs| inputs.len() >= limits.alternatives.clamp(1, 16));
```

The value-alternative allowance stays at 128. The bound of 16 is an experimental precision policy, not a number required by another analyzer. After implementation, check compilation, the 21 focused checks, 18 annotation checks, all 113 non-panic cases and 45 cache checks, then panic-format. Reject a candidate that loses the known panic-route Forbidden result. Record any new Unknown or Forbidden diagnostic; do not change scenario expectations or add annotations to conceal a regression. Only then run the kernel at the same limits. If stage159 completes, inspect that result before deciding whether this comparison is necessary. No tests or source comments are planned.

At 20:37 UTC, both stage159 kernel checks remain active after approximately 3621 seconds. The checkpoint is stage159/kernel-checkpoint-60m.json. All eight workers advanced; all are now traversing rebuilt diagnostic graphs. No main-worker terminal counter or analysis limit message is present. Private memory is approximately 3.25 GB per process. The original 4800-second allowance remains in effect. All nine source snapshots still match working source. Saved reports were checked again: 113 non-panic cases, 45 cache checks, 21 focused checks, and 18 annotation checks pass. The kernel worktree still has the same 17 modified paths. No solver or kernel source change, annotation, or restart was made.

At 20:16 UTC, both stage159 kernel checks passed 2400 seconds and remain active under their original 4800-second limit. The checkpoint is stage159/kernel-checkpoint-40m.json. It records the live process state, frozen driver hash, memory, CPU, and all eight worker progress files. Private memory is approximately 2.94 GB on x86 and 2.90 GB on ARM. Neither analysis is complete. The current ARM worker0 graph dropped from 48299 nodes at query100300 to 5851 nodes at query113500 while the same process continued. The current source clears the diagnostic graph when call-input generation changes. This confirms repeated graph work in stage159; it does not measure its full cost. No solver or kernel source was changed.

The saved stage159 panic evidence has six bound edges to Forbidden::service. All six come from Sink::write_str bb2[5] with cut_recursive_returns=true and path_proven=true. Contexts 101, 116, and 248 used 8793 steps; contexts 106, 121, and 251 used 8749 steps. The incoming root path still has uncertainty, as the diagnostic reports. In rejected stage157, all 16 saved query records at this callback site have cut_recursive_returns=false, and there is no bound Forbidden edge. This confirms that the retained binding depends on the bounded witness work in these saved runs. Do not remove this work as an optional cost cut. The records do not identify every earlier transition that removed a witness opportunity in stage157. Both stage159 kernel runs remain active; no solver or kernel source changed during this comparison.

At 20:01 UTC, both stage159 kernel runs remain active after approximately 1498 seconds of kernel-process time. Eight polls used the same sessions 55549 and 94007; neither returned a terminal result. X86 CPU time increased to approximately 5925 seconds and ARM to 5931 seconds. All eight worker query counts advanced since 19:56. Current counts are x86 55500, 40800, 52600, 54000 and ARM 74200, 38700, 43500, 50500. Private memory is approximately 2.74 GB and 2.67 GB. This is a verified wait, not a completed analysis or a new speed claim. No source change, annotation, or restart was made.

Clang's current SValBuilder represents some pointer-to-integer conversions as LocAsInteger and can recover the location on a later pointer cast. SimpleSValBuilder returns Unknown for unsupported non-comparison operations on that representation. Sources: https://clang.llvm.org/doxygen/SValBuilder_8cpp_source.html and https://clang.llvm.org/doxygen/SimpleSValBuilder_8cpp_source.html. This is a model for retaining pointer origin while limiting further calculation. It does not make arbitrary integer-to-function-pointer conversions complete, and it does not prove that the current kernel callback queries can be omitted. No new model was added during the active runs.

At 19:55 UTC, the same stage159 kernel processes remain active. X86 kernel-process time is 1108.809 seconds; ARM is 1110.127 seconds. Private memory is 2633793536 bytes and 2657431552 bytes, respectively. CPU time increased. No kernel result, source change, annotation, or restart was made.

The x86 query40000 timing record for worker2 attributes 243508 ms to 380 queries at kernel_sync::locks::irq::interrupts_enabled bb3[11], 218449 ms to 342 queries at core::fmt::write bb2[6], and 211484 ms to 317 queries at Argument::fmt bb3[10]. The backend coverage file records incomplete pointer-to-integer conversions in set_irq_interrupt_control and incomplete integer-to-function-pointer conversions in interrupts_enabled, interrupts_disable, and interrupts_enable. kernel-call-coverage.txt marks all three callback sites covered=false and unknown_receiver=true. The kernel helper source stores function pointers in AtomicUsize. The current backend copy_flattened_fields only transfers pointer-typed fields; these integer conversions require value-flow refinement. Thus the expensive interrupt-helper work is not only loop control analysis. These records identify a concrete backend coverage gap and repeated binding work. They do not prove that every repeated query is unnecessary, or that a candidate can replace an established binding. No fallback or annotation was added for these sites.

At 19:48 UTC, both stage159 kernel runs remain active on the same processes and sessions. X86 kernel-process time is approximately 696 seconds and ARM is 697 seconds. Private memory is approximately 2.48 GB and 2.51 GB. CPU time increased throughout the observations. Neither session has a terminal result. No solver or kernel source change, annotation, or restart was made during this verified wait.

At approximately 19:43 UTC, both stage159 kernel sessions remain active. An early live checkpoint is saved in stage159/kernel-early-checkpoint.json, including the driver hash, CPU and memory samples, and all eight worker progress records. No main worker has reported a terminal result. A partial x86 timing record for worker1 at query20000 attributes 97715 ms to 228 queries at core::fmt::write bb2[6], and 27778 ms to 249 queries at kernel_sync::locks::irq::interrupts_enabled bb3[11]. These identify current work, not a complete performance comparison or a proved root cause. No solver or kernel source changed during these runs.

At 19:37 UTC, both stage159 main kernel analyses are active. X86 PID42140 has 17 IRQ roots; ARM PID39408 has 15. Each executable path matches the frozen stage159 driver. X86's pointer graph has 46760 contexts and 617 coverage gaps; ARM has 42265 contexts and 638 gaps. Backend gap counts are not final IRQ diagnostics. At approximately 48 seconds of kernel-process time, x86 private memory was 1764519936 bytes and ARM was 1558470656 bytes. All eight worker progress files exist and advance. No completed kernel result is established.

Stage159 panic negative is terminal. It retains the actual Forbidden::service error, with reachability uncertainty, and reports five Unknown errors: three AlternativeLimit stops and two TransferLimit stops. Therefore the panic negative scenario still fails. Evidence has incomplete=false and diagnostics_complete=true. Main counters are 3144 queries, 555950034 transfers, 2278 frames, 137 revisions, 10722293 evaluations, and 3949715 changes. The log is target/scenarios/20261023-192408-2165502/recursive-bindings-negative.log. Safe and cache pass. The five limit failures are not established inherent coverage gaps and were not annotated away.

At 19:34 UTC, stage159 x86 and ARM kernel checks were launched after the retained-Forbidden gate passed. X86 session55549 uses target/stage118-profile-cargo and writes target/scenarios/memory-20261023-193428-3067085/stderr.log. ARM session94007 uses target/stage151-arm-kernel-cargo and writes target/scenarios/memory-20261023-193443-4854905/stderr.log. Each uses the saved stage154-comparison limits: 4800 seconds and 10240 MiB, with all other limits enabled. Settings and the frozen driver hash are in stage159/kernel-settings.json and kernel-arm-settings.json. The launches were separate, and the Cargo target directories differ. No kernel result is established yet. Keep polling these sessions; do not restart them because an observation ends.

At 19:30 UTC, stage159 panic negative remains active with PID13932 and session41155. CPU time increased to 330.688 seconds. The latest progress file records 1500 queries, 305925159 transfers, and 1436 frames. Private memory is 185327616 bytes. This is an active analysis, not a terminal result. Keep the same process under its 1200-second limit. No source change, new annotation, or kernel run was made during this wait.

Stage159 passes the full non-panic audit: 113 of 113 cases and 45 of 45 cache checks. Standard time was 467.023 seconds; extra time was 153.800 seconds. Its 21 focused and 18 annotation checks also pass. Panic-format runs last. Safe and cache pass; at 19:25 UTC, negative is active with driver PID13932 in target/scenarios/20261023-192408-2165502/recursive-bindings-negative.log. Validation session41155 remains active. No kernel run has started. No source change was made during this audit.

Stage159 passes all 94 standard cases and all 45 cache checks. The standard audit took 467.023 seconds. Its report identifies driver 016E9AA6D8600A33DB1DD3403C30B26886356B46EB6DE7E598707791298EFF98. At 19:22 UTC, the extra group is active in the same session41155. Panic-format follows only after all 19 extra cases pass. No solver or kernel source changed during the full audit. No kernel run has started.

At 19:16 UTC, stage159 standard validation is active in session41155; its first 18 cases pass. Four inactive evidence JSON files were compressed with NTFS compression. SHA256 was checked before and after each operation; all four hashes are unchanged. No files were deleted. Their reported storage reduction is approximately 6.9 GB. C: has approximately 16.9 GB free after compression. The paths, hashes, and compression results are in stage159/artifact-compression.txt. The compression session is terminal.

Stage159 passes all 18 annotation checks in target/contract-checks/20261023-191259-2077934/results.json. At 19:14 UTC, the full standard audit is active in session41155. Extra and panic-format remain gated behind it. No kernel run is active. C: has approximately 6.5 GB free; no artifact was removed.

Stage159 is the current candidate. Compiler checking and release compilation pass. All 21 focused checks pass. The frozen driver SHA256 is 016E9AA6D8600A33DB1DD3403C30B26886356B46EB6DE7E598707791298EFF98. The nine source snapshots match working source. Only flow.rs differs from stage154; the difference removes the four-line loop fallback reset in begin_query. Ordinary formatting negative reports only Forbidden, with 6035343 transfers, 50489 evaluations, and 39 frames in target/scenarios/20261023-191203-4878122/formatting-negative.log. Stage154 used 10250878 transfers and 107 frames. This case-level work reduction does not establish kernel performance.

At 19:13 UTC, annotation checks are active in session41155. The command then runs standard, extra, and panic-format in order, with acceptance gates before each later group. It uses target/contract-checks-cargo. Session61598 ended after the focused checks because an array wrapper made the PowerShell count check see one array instead of 21 records. Saved focused results prove 21 accepted checks; session41155 resumes from annotations without repeating them. Stage157 and stage158 sessions are terminal. No kernel process is active. Stage159 kernel settings are prepared at the stage154 limits, but are not launched. Keep the known-Forbidden gate before launch.

The stage157 evidence comparison confirms that the missing Forbidden is a binding loss, not an annotation filter. Stage154 has four bound Sink::write_str edges to Forbidden::service. Stage157 has none. Its six Unknown reasons are three AlternativeLimit stops, two TransferLimit stops, and an unknown storage address. The fixture and its expected result were not changed.

Stage157 panic negative finished at 19:08 UTC and is rejected. It reports six Unknown errors and no Forbidden error. Evidence has incomplete=false and diagnostics_complete=true. The main run used 4090 queries, 1050730031 transfers, 4133 frames, 141 revisions, and 453 coarse queries in three functions. Stage154 retained one Forbidden error. No stage157 kernel run was started. Removing optional witness cases and caller reachability relations did not preserve the required diagnostic, even though normal target queries remained enabled. The exact first lost binding is not yet isolated.

Stage158 passes all 21 focused checks, but uses the now-rejected stage157 base. Its frozen driver SHA256 is 8B10FEC24BCED55CD748D4FE2B9356269EEA7BFA76F34123F00AB63DA10DA203. Keep this result as evidence; it is not a full audit or a panic result. The stage158 session is terminal.

Stage159 removes the cumulative reachability cutoff and tests only retained loop fallback policy against stage154. Working analysis.rs was restored with a patch and matches the stage154 snapshot hash. The intended solver difference from stage154 is the four-line removal in begin_query described below. Run compiler checks, focused checks, annotations, the full non-panic audit, and panic-format last. No kernel run is permitted until the known panic-route Forbidden call is retained. No tests, source comments, fixture expectations, or kernel annotations changed.

Stage158 will test retention of conservative loop fallback policy between queries. Clang documents that FunctionSummaries can stop repeated inlining after an earlier context exceeded its block limit: https://clang.llvm.org/docs/analyzer/developer-docs/IPA.html#deciding-when-to-inline. Current begin_query clears loop_fallbacks and advances the control epoch; this makes later queries repeat precise work before selecting the same fallback. Stage158 will remove that reset. Adding a fallback still advances the epoch and invalidates affected cached reads through the existing request handling.

The query reset will keep these operations, without clearing loop_fallbacks:

```rust
self.controls.causes.clear();
self.controls.attempted_loops.clear();
self.index_choices.clear();
self.loop_entries = Box::new([]);
self.cut_recursive_returns = false;
self.retain_completed();
```

This retains a conservative analysis policy, not a proof of a precise value or of normal return. Existing call summaries still return Unknown for scalar return data and storage whose preservation is not proved. Exact pointer-return work, complete storage-preservation checks, required target work, and annotation rules remain unchanged. This may lose precision in later contexts; the existing focused and annotation checks must determine whether that loss causes a regression. Use a separate Cargo target and keep the frozen stage157 panic process active. If focused checks pass, run the full non-panic audit before panic-format. No new tests, source comments, or fixture changes are planned.

At 19:00 UTC, stage157 panic negative remains active with PID3672 and validation session24013. CPU time increased to 444.078 seconds. Its progress file records 1600 queries, 483600539 transfers, and 2116 frames. No stage157 kernel run has started. The debugger was attached once, then detached with keep-stopped=false. CPU time increased after detach. The sample is saved in stage157/panic-stack-1.txt. The sampled worker was in bounded guard hashing, called through feasible_with_paths, control_step, read_uncached, and targets at analysis.rs:538. It was inside target-value computation, not the outer target-only feasibility call. This sample does not measure the share of total time.

Current flow.rs requests control(previous) for an IRQ loop back-edge in a value merge. Control demands call control_step with refinement enabled. Thus the outer cumulative reachability cutoff does not remove all branch evaluation inside required binding work. Do not change this rule from one sample alone: those branch relations can exclude incorrect callback values. No solver, fixture, or kernel source changed during this observation.

Stage157 passes the complete non-panic audit: 113 of 113 cases and 45 of 45 cache checks. Standard time was 479.483 seconds; extra time was 151.478 seconds. The 21 focused and 18 annotation checks also pass. These timings do not prove a kernel speed improvement; concurrent work differed from earlier runs.

Panic-format ran last. Safe and cache checks pass. At 18:53 UTC, negative is active in target/scenarios/20261023-185217-3743793/recursive-bindings-negative.log. Its driver was verified live with PID3672. The validation session remains active. No stage157 kernel run has started. The kernel worktree still has the same 17 modified paths; no new kernel edit or annotation was made.

Stage157 passed all 94 standard cases and all 45 cache checks in 479.483 seconds. The extra group is active in the same validation session, followed by panic-format. The x86 and ARM settings files are prepared in stage157, but neither kernel run has started. They retain the stage154 limits and identify the frozen stage157 driver. Keep the panic diagnostic gate before launch. No solver or kernel source changed during this validation.

Stage157 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Ordinary formatting negative reports its required Forbidden error without an extra Unknown. The annotation report is target/contract-checks/20261023-184058-1749892/results.json. Driver SHA256 is AE2B368F1BB96C60FEA138E8B5B1C18A782A4D991431CCF3EE6FECA0F8CFAFA2. All nine frozen source files match working source.

At 18:42 UTC, stage157 full standard and extra validation is active on target/contract-checks-cargo. The same runner will check panic-format last only if all 113 non-panic cases and 45 cache checks pass. No kernel process is active. Do not apply stage156's full-audit counts to stage157. Before a new kernel run, require the stage157 panic negative result to retain the known Forbidden call. The complete kernel goal remains open. No tests, source comments, kernel annotations, or expected-result changes were added in this investigation.

At 18:37 UTC, both stage156 kernel driver processes were stopped after their executable paths were checked. This was a rejection of the candidate after panic-format lost the established Forbidden call, not an observation timeout. Preserve all logs. The measurement wrappers report reason=completed because the processes exited; the analyses did not complete. X86 total measured time was 1399.633 seconds with peak private memory 2624692224 bytes. ARM time was 1299.920 seconds with peak private memory 2333589504 bytes. Do not treat either as a completed kernel check.

Stage157 retains its coarse-mode flag in QueryContinuation, so index-split continuations cannot silently restart full reachability refinement. All target guards are marked required before target-only feasibility checking. The marker uses the existing widened control-guard form. Compiler checking and release compilation pass. The next validation sequence is focused checks, annotation checks, all non-panic checks, then panic-format. Do not start another kernel run before the panic result confirms that the established Forbidden call is retained.

Stage156 panic-format negative is terminal and rejected. It completes with two Unknown errors and no Forbidden error. The baseline stage154 reported the real Forbidden call as well as six Unknown errors. Stage156 lost required callback bindings at Argument::fmt and Sink::write_str after the cumulative cutoff. Evidence has incomplete=false and diagnostics_complete=true. Main counters are 1391 queries, 125921596 transfers, 1527 frames, 93 revisions, and 11 coarse queries in three functions. The negative log write interval is 186.875 seconds, not an isolated CPU measurement. Reduced work does not justify losing this diagnostic. Keep the frozen stage156 kernel runs as comparison evidence; do not accept their final result as the repaired checker.

Stage157 will restrict the cumulative fallback to reachability refinement. Keep the normal target query and check the guards that establish its binding. Do not replace caller-specific diagnostic inputs solely because the work threshold was reached. Keep the existing independent-summary and alternative-count rules from stage154. A fallback path gets a widened reachability guard, but target values are still obtained through the required binding request. Existing query retries, index splits, and context-change handling remain in use.

```rust
let targets = solver.targets(point)?;
let targets = if coarse { solver.feasible(targets)? } else { targets };
```

In coarse mode, combine these target values with the inherited guards and a widened reachability guard. Do not run another feasibility query over that combined reachability state. This preserves required selector work and avoids treating a backend candidate as an established binding. Recheck focused cases and annotations, then the complete non-panic audit, and panic-format last. No kernel annotation or expected-result change is planned.

Stage156 passes all 113 non-panic cases and all 45 cache checks. The standard group took 546.391 seconds; the extra group took 198.713 seconds. All 18 annotation checks also pass. Panic-format ran last; safe and cache checks pass, and negative is active in target/scenarios/20261023-182624-4796436/recursive-bindings-negative.log. Both kernel runs remain active. No complete kernel result is established. The saved standard logs have no nonzero coarse-query count, so those passes establish compatibility but do not validate every fallback path.

Stage156 passed all 94 standard cases and all 45 cache checks in 546.391 seconds. Extra cases are active; panic-format follows only after they pass. Both kernel analyses remain active. At the 18:24 UTC sample, x86 had 497.548 seconds of kernel-process time and 2228834304 private bytes. ARM had 388.857 seconds and 2017304576 private bytes. These samples do not establish a complete kernel result or a full-run speed improvement.

Stage156 ARM validation is active at target/scenarios/memory-20261023-181539-1738074/stderr.log, on target/stage151-arm-kernel-cargo. Settings are in stage156/kernel-arm-settings.json. The first launch at memory-20261023-181523-1221777 stopped before analysis because PowerShell passed the unquoted BuildStd list as separate arguments. The corrected command quotes 'core,alloc,compiler_builtins'. Keep that failed launch log; it is not a checker result. Both active kernel runs use the frozen stage156 binary and separate Cargo directories.

Stage156 passes compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks. Ordinary formatting negative reports only Forbidden. Its log is target/scenarios/20261023-181133-3742228/formatting-negative.log, with 10348624 transfers and zero coarse queries. Its active queries completed after the cumulative admission threshold was reached. The annotation report is target/contract-checks/20261023-181227-2980666/results.json. All nine saved source files match working source. Driver SHA256 is 312A3B7FED0EA6AB0C9118F3B90C72025893BA851C9BC9A09FF847C1752C4CFC.

The full non-panic audit is active on target/contract-checks-cargo. Panic-format runs last only if that audit passes. The stage156 x86 kernel comparison is active at target/scenarios/memory-20261023-181359-2493563/stderr.log, using target/stage118-profile-cargo. Kernel limits match stage154, including 4800 seconds and 10240 MiB. Settings are in stage156/kernel-settings.json. No full kernel result or speed improvement is established yet. No kernel source or expected result changed.

Stage155 passed 20 of 21 focused checks. Ordinary formatting negative added one Unknown at Argument::fmt, plus the required Forbidden call. Its log is target/scenarios/20261023-180641-1690429/formatting-negative.log. This candidate is not accepted. Stage154 completed that call query with 2661425 steps; stage155 cut it at 1496672 steps because other contexts had already used the shared function allowance. The baseline total was 10250878 transfers, while stage155 stopped at 9159202. This is a confirmed precision regression from cutting an admitted query, not a new kernel coverage gap.

Stage156 will use the cumulative account to stop new precise queries, not to truncate queries that have already started. Each admitted query retains its existing allowance and one retry. Resumed queries also retain that allowance. New queries after the cumulative threshold use the conservative diagnostic fallback. Thus query_steps * 9 is an admission threshold, not a strict bound on all work for a function. Outstanding queries can use their remaining allowances. Keep their normal limits enabled. The account still survives graph revisions.

```rust
let coarse = !scalar_witness && remaining == 0 && resume.is_none();
solver.budget.limit = diagnostic_limit;
```

The second line applies only to the existing one-time query retry. Stage155 source and binary remain frozen. Repeat the focused and annotation gates before a new kernel run.

Stage154 ARM reached the 4800-second analysis limit. Total measured time was 4920.389 seconds; peak checker private memory was 3168464896 bytes. Evidence has incomplete=true and diagnostics_complete=false. The four terminal worker query counts were 155353, 147476, 103420, and 164245. The log is target/scenarios/memory-20261023-164536-9318251/stderr.log. Both stage154 kernel runs are now terminal resource failures. No kernel annotation was added.

The user asked to reduce checker precision where necessary and to compare other static analyzers. The stage151 80-minute comparison is deferred and has not started. Stage155 will instead bound cumulative diagnostic work for each function. The allowance is query_steps * 9, equal to one existing query allowance with its retry. Count work across caller contexts and graph revisions. Do not reset this account during a graph restart.

After this allowance is used, inspect the remaining indexed call sites without caller-specific value or condition refinement. Preserve direct targets and complete singleton backend bindings. Other targets stay Unknown. Keep valid unreachable exclusions from BodyIndex. Mark reachability as uncertain and use shared unknown-entry diagnostic frames. This is a diagnostic fallback, not a memory-effect summary: it must not preserve unknown writes or change pointer values in the value-flow cache. Keep RUPTA and the current annotation contracts.

```rust
let coarse = diagnostic_steps.get(&instance).copied().unwrap_or(0)
    >= limits.query_steps.saturating_mul(9);
let target = direct.or_else(|| solver.bound_target(point));
let value = target.map_or(Value::Unknown("the cumulative function allowance does not establish this target"), Value::Function);
```

The implementation will add a widened guard to this fallback result. It will retain known assertion panic entries. Account for safety checks and target-query work. Validate existing focused and annotation checks, then all non-panic cases and cache checks. Run panic-format last. No tests, source comments, expected-result changes, or kernel annotations are planned. Preserve the active frozen stage154 ARM run.

GCC merges and prunes states and limits exploration at program points: https://gcc.gnu.org/onlinedocs/gccint/Analyzer-Internals.html. Clang limits repeated inlining and symbolic expression complexity: https://clang.llvm.org/docs/analyzer/user-docs/Options.html. These tools do not provide a general proof that abandoned paths are safe. Stage155 will report unresolved bindings instead of treating omitted analysis as Safe. The proposed allowance is a local design choice, not a threshold prescribed by either tool.

Stage154 x86 is terminal. It reached the 4800-second analysis time limit. Total measured time was 4956.723 seconds including build work, and peak checker private memory was 3266236416 bytes. Evidence has incomplete=true and diagnostics_complete=false, with the detail that no partial result is accepted. Final worker query counts were 116349, 125512, 108436, and 153302. No worker's main analysis completed. This is a checker resource failure, not an established kernel coverage gap. The terminal report is stage154/kernel-results.json and the log is target/scenarios/memory-20261023-163024-4779367/stderr.log. Stage154 ARM remains active under its original allowance.

The next x86 comparison uses the frozen stage151 binary at the same 4800-second kernel limit. Stage151 predates the dependency-repair and queue changes and already passed the non-panic and annotation gates. Its earlier 2400-second run completed one worker. The comparison will help separate the effect of those changes from the time allowance. It does not revert working source or establish that stage151 is the final solver. Keep the stage154 ARM run active. Use the now-free target/stage118-profile-cargo directory, and save the new evidence under stage154-control151-80m/kernel-evidence. Settings are in stage154-control151-80m/kernel-settings.json. Retain all other limits and the 10240 MiB cap. No kernel annotation or expected result will change.

```powershell
./scenarios/measure-memory.ps1 -Checker ./target/control-summary-20261020/stage151/bin/irq-check.exe -Scenario kernel -ManifestPath C:/Users/Boden/Documents/rustOS/kernel/Cargo.toml -WorkingDirectory C:/Users/Boden/Documents/rustOS -CargoTargetDirectory ./target/stage118-profile-cargo -Target C:/Users/Boden/Documents/rustOS/x86_64-rustos-kernel.json -BuildStd core,alloc,compiler_builtins -DumpDirectory ./target/control-summary-20261020/stage154-control151-80m/kernel-evidence
```

The longer stage154 runs show diagnostic graph repetition. ARM worker0 advanced from 99900 queries and 48563 graph nodes at 17:32 UTC to 110000 queries and 3470 nodes at 17:33 UTC. X86 worker3 advanced from 112600 queries and 54185 nodes at 17:33 UTC to 130300 queries and 4550 nodes at 17:34 UTC. The same driver processes remained active and their CPU time increased. Current analysis.rs clears the diagnostic graph after a pass when the value-flow input generation changed. These samples establish repeated graph work, not its total cost or the number of all passes. No complete kernel result is established, and no solver change was made.

Stage154 ARM also passed the earlier 2400-second cutoff without a complete result. Its live checkpoint at 2426.556 seconds is saved in stage154/kernel-arm-checkpoint-40m.json. Private memory was 2502991872 bytes. Periodic worker query counts were 88300, 49200, 71800, and 50800. Stage153 ARM's terminal progress files recorded 54100, 45600, 53900, and 50200. These are progress-file samples, not terminal counters or proof of a full-kernel speed improvement. At the same observation, stage154 x86 was at 3303.010 seconds and remained active. Both checks continue under the existing 4800-second allowance. No solver or kernel source change was made.

The stage154 x86 run passed the earlier 2400-second cutoff without a complete result. A live checkpoint at 2418.848 seconds of kernel-process time is saved in stage154/kernel-checkpoint-40m.json. Private memory was 2515292160 bytes. The periodic worker query counts were 47500, 45400, 55300, and 55000. Stage153's terminal progress files recorded 47100, 42900, 54800, and 52100 at its 2400-second stop. Stage151's corresponding files recorded 143000, 135400, 108400, and 233200. The stage152 dependency repair changed budget charges, so counts across that boundary are not a direct measure of speed or precision. This checkpoint does not establish a full-kernel speed improvement. Both stage154 checks remain active under 4800 seconds; the ARM process was at 1541.943 seconds. No run was restarted.

Stage154 panic-format negative is terminal. It completed without the analysis timeout, but the scenario fails with six Unknown errors and one Forbidden error. The Unknown reasons are two TransferLimit stops, three AlternativeLimit stops, and one unknown storage address. The log is target/scenarios/20261023-163742-3749204/recursive-bindings-negative.log. Evidence has incomplete=false and diagnostics_complete=true. The established Forbidden call is Sink::write_str -> Forbidden::service on the panic route, with a reachability uncertainty note. Safe and cache checks pass. The negative log write interval was 1127.677 seconds; this is not an isolated CPU measurement. Main counters are 4405 queries, 1001708605 transfers, 4156 frames, 25589780 value-flow evaluations, 241464 dependency repairs, 8998784 affected nodes, and 27326390 repair visits. The extra Unknown errors remain unresolved. Do not suppress them with a fixture annotation or claim that this scenario passes.

At 2026-10-23 16:57 UTC, both stage154 kernel checks remain active. The x86 driver is PID39012; the ARM driver is PID45656. CPU time increases in both processes. Private memory is approximately 2.44 GB and 2.12 GB, respectively. Both checks retain the 4800-second analysis allowance and 10240 MiB memory cap. All nine source snapshots still match working source, and the frozen driver hash is unchanged. The 21 focused checks and 18 annotation checks were rechecked in their result files; all pass. No solver or kernel source change was made while these checks ran. Keep the solver unchanged until the kernel runs finish. There is still no complete kernel result.

Both stage154 kernel checks are active. The ARM log is target/scenarios/memory-20261023-164536-9318251/stderr.log. It uses target/stage151-arm-kernel-cargo after stage153 ARM exited, with separate stage154/kernel-arm-evidence and kernel-arm-settings.json. Both targets use 4800 seconds and the unchanged 10240 MiB memory cap. Keep the current solver unchanged while these full runs finish. The final panic-format result is recorded above.

Stage153 x86 reached the 2400-second analysis time limit. Total measured time was 2560.480 seconds, and peak checker private memory was 2570428416 bytes. Stage153 ARM also reached that limit. Its total time was 2550.161 seconds, and peak checker private memory was 2547929088 bytes. Both evidence files have incomplete=true and diagnostics_complete=false. No partial result is accepted. These are resource failures, not established kernel coverage gaps.

The stage154 stack sample is stage154/kernel-stack-1.txt. Three sampled worker stacks show call-effect summary work, hashing of alternative guards in bounded, and a trace environment-variable read in control_step. The fourth worker was not identified by the selected thread numbers. Local vector lengths were not available from the debugger. The sample does not prove a total cost breakdown or a new binding defect. The debugger was detached, and CPU progress resumed. The brief pause is part of the run's elapsed time.

Stage154 passed all 113 non-panic cases and all 45 cache checks. Standard time was 673.604 seconds, and extra time was 210.060 seconds. Panic-format ran last on target/contract-checks-cargo. The x86 and ARM kernel runs remain active under the 4800-second allowance. Early timing samples do not establish a speed improvement.

Stage153 panic-format negative is terminal and fails at the 1200-second analysis time limit. Its log is target/scenarios/20261023-161327-2146985/recursive-bindings-negative.log. Evidence has incomplete=true and diagnostics_complete=false. It records 3899 queries, 883148126 transfers, 4112 frames, 211930 repairs, 6798237 affected nodes, and 20619506 repair visits. No partial result is accepted. The queue-preservation correction did not remove the panic-format timeout regression.

Stage154 x86 kernel validation is active at target/scenarios/memory-20261023-163024-4779367/stderr.log. It uses target/stage118-profile-cargo after the stage152 run exited. Settings are in stage154/kernel-settings.json. The driver was verified active with PID39012 at 53.723 seconds of kernel-process time, with 1622691840 private bytes. The time allowance is 4800 seconds; other limits are unchanged. Full scenario validation continues separately on target/contract-checks-cargo.

Stage152 x86 is terminal. It reached the 2400-second analysis time limit. Total measured time was 2515.089 seconds, and peak checker private memory was 2594197504 bytes. Evidence has incomplete=true and diagnostics_complete=false. The four terminal worker query counts are 47070, 42798, 54948, and 49424. Their evaluation counts range from 58138770 to 152340229. The dependency repair change did not establish a kernel speed improvement. No partial result is accepted. Keep its source, binary, PDB, logs, and evidence for comparison.

Inactive stage124-focused-cargo policy compression is complete. The final free-space sample was 20658933760 bytes. No file was deleted, and active policy 0c8897da6db4b1ee was excluded. The kernel worktree still has the same 17 modified paths; no kernel edit was made in these stages.

The next stage154 kernel run will use 4800 seconds. The stage151 x86 and ARM checks reached 2400 seconds without a complete result, while private memory stayed below 3 GB. One stage151 x86 worker completed its traversal. The stage152 and stage153 workers continue to advance below 3 GB, but no full kernel result is established. This evidence supports a kernel-only time extension under the approved limit policy. Keep query, scalar, alternatives, depth, scratch, and the 10240 MiB memory cap unchanged. Scenario checks keep 1200 seconds. Compare query counters and the initial common time interval separately from the longer run's final result. Do not restart a live run to apply this change.

```powershell
$env:IRQ_CHECK_TIMEOUT_SECONDS = '4800'
```

Stage154 passes all 21 focused checks and all 18 annotation checks. The annotation report is target/contract-checks/20261023-162054-6067741/results.json. Its full standard and extra audit is now active on target/contract-checks-cargo, followed by panic-format. No stage154 kernel run has started. Do not reuse target/stage118-profile-cargo until the stage152 kernel run is terminal.

Stage152 panic-format negative is terminal and fails at the 1200-second analysis time limit. Its log is target/scenarios/20261023-160129-5518924/recursive-bindings-negative.log. Evidence has incomplete=true and diagnostics_complete=false. Counters are 4009 queries, 906293757 transfers, 4122 frames, 222648 dependency repairs, 6187631 affected nodes, and 18628340 repair visits. The full-set counter is 1673579504 nodes. This is worse than stage151's completed failed panic run, whose negative-log write interval was 559.186 seconds. The runs overlap other work, so that interval is not an isolated CPU measurement. Fewer bookkeeping transfers do not prove lower analysis cost. Do not classify this timeout as a genuine program coverage gap or hide it with an annotation.

The latest free-space sample was 15138406400 bytes. Inactive-cache compression is still active. Kernel runs on stage152 and stage153 continue without a restart. No complete kernel result is established.

Stage154 is built and frozen. Its driver SHA256 is E57D76F1E735DC656D0B82C3739C89F2D09FDD46F84B76D44E0C710125118A75. Compiler checking and release compilation pass. All nine source snapshot files match the working files. Focused validation is active on target/contract-checks-cargo. Annotation validation follows in the same process only if all 21 focused checks pass. No stage154 kernel run has started. Its runner uses target/stage118-profile-cargo, which remains occupied by stage152; verify that run is terminal before reuse.

Stage153 passed all 113 non-panic cases and all 45 cache checks. Standard time was 640.945 seconds, and extra time was 230.206 seconds. Panic-format runs last. Its x86 and ARM kernel processes remain active. The ARM log is target/scenarios/memory-20261023-160246-3080936/stderr.log. No new kernel annotation or expected-result change was made.

Disk space fell below 7 GB. The first eight selected inactive policy folders already had compressed data, so compressing their directory attributes recovered little space. Compression now selects inactive stage124-focused-cargo policy folders with uncompressed files larger than 1 MiB. Each folder is checked against its exact parent and live process paths. Active policy 0c8897da6db4b1ee is excluded. No files are deleted. The later free-space sample was 7684272128 bytes while compression was still active. New validation reuses the compressed contract-checks-cargo folder.

The stage153 stack sample is stage153/kernel-stack-1.txt. Two workers were in bounded, at the guard-subset comparison and the branch-group key hash. The other two were in call-effect reads. No sampled worker was in dependency repair. This is one sample, not a complete cost profile. The debugger was detached and CPU progress resumed. Early stage153 kernel timings do not show a clear speed improvement over stage152.

Stage154 will index guard membership inside bounded. The current alternative-pair check scans a guard list for each guard in the other list. Long guard lists will get one sorted copy per alternative. Exact membership includes all guard fields. The relaxed Unknown comparison ignores only proof and required, as the current code does. Lists shorter than 16 guards keep the linear check. This threshold changes only the lookup method, not precision. Cache path_proven for each alternative during the comparison pass. Charge temporary storage against scratch. Do not change removal order, proof preference, branch grouping, or alternative limits.

```rust
guards.sort_unstable_by_key(|entry|
    (guard_key(entry), entry.1.proof as u8, entry.1.required));
let found = guards.binary_search_by_key(&guard_key(&needle), guard_key).is_ok();
```

The example shows the relaxed lookup. Exact lookup also compares proof and required. Compile and validate the existing focused and annotation cases, then the full non-panic audit and panic-format last. Keep the active frozen stage152 and stage153 kernel runs unchanged. No test, source comment, kernel annotation, or expected result will be added.

Stage152 has passed all 113 non-panic cases and all 45 cache checks. Standard time was 598.011 seconds; extra time was 209.783 seconds. Panic-format runs last. Its x86 kernel comparison continues. These runs overlap other checks, so their wall times are not isolated performance measurements.

Stage151 ARM is terminal. It reached the 2400-second analysis time limit. Total measured time was 2513.170 seconds, and peak checker private memory was 2865659904 bytes. Evidence has incomplete=true and diagnostics_complete=false. The four terminal worker query counts are 147194, 154648, 167451, and 104396. No partial result is accepted. Stage153 ARM validation now uses the free target/stage151-arm-kernel-cargo directory with a new checker policy cache and separate stage153/kernel-arm-evidence. Its limits match stage153 x86. The frozen stage151 evidence remains unchanged.

Stage153 is built and frozen. Its driver SHA256 is 015893B72797EAA97B3CA9513C77EF9835B889899963216B5EF5D7220BEA4048. All nine source snapshot files match the working source. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. The annotation report is target/contract-checks/20261023-155621-2996635/results.json. Ordinary formatting negative reports its Forbidden error without an extra Unknown. Its counters are 130400 evaluations, 70603 changes, 847 repairs, and 6457 affected nodes.

The stage153 full non-panic audit is active on target/stage119-focused-cargo, followed by panic-format. Its kernel run is target/scenarios/memory-20261023-155906-1035571/stderr.log on target/stage153-kernel-cargo. It uses the same limits as stage152, including 2400 seconds for kernel analysis and 10240 MiB. All kernel runs are compiler checks, not boots. Stage152 has passed 94 standard cases and 45 cache checks; its extra cases and kernel run remain active. No complete kernel result is established.

Stage153 will remove two queue costs shown by the stage152 native stack sample. One worker was in the pending-queue HashSet construction at flow.rs:2135. Another was allocating require's one-item work list from the nested read path. The transcript is stage152/kernel-stack-1.txt. The debugger was detached and CPU progress was verified. The stage152 kernel run continues unchanged. Its early timing samples do not establish a speed improvement.

During dependency repair, restored nodes were required before the repair. Their queued work is still in the pending queue. Add a scheduling flag to require: normal activation schedules work, but restoration only repairs required membership. Keep the existing pending order. Filter pending only if required nodes were actually removed. Do not build a second queue-membership set. Also return from require before allocating its work list if the input is already required. Retain the same step charge on that fast path.

```rust
let required = active.required.len();
for input in &affected { active.required.remove(input); }
for input in anchors { self.require(input, false)?; }
let active = self.flow.active.as_mut().unwrap();
if active.required.len() != required {
    active.pending.retain(|id| active.required.contains(id) && self.flow.queued.contains(id));
}
```

This is a code sketch, not the full function. The implementation will add no source comment. Validate with compiler checking, release compilation, the existing focused and annotation checks, then full non-panic checks and panic-format last. Preserve the frozen stage152 run for comparison. No pointer, guard, annotation, or expected-result change is planned.

Stage152 is now built and frozen. Its driver SHA256 is E8B2352A983C3904901D1B1B053D850C63514CA9257A88298A0C6695D531B530. All nine source snapshot files match the working files. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Annotation results are in target/contract-checks/20261023-154223-0526053/results.json. Ordinary formatting negative recorded 846 dependency repairs, 6576 affected nodes, 12466 repair visits, and 1343361 full-set nodes. The last count is the sum of the old required-set sizes, not a measured count of old edge visits. It does not prove a kernel speed improvement.

The stage152 full standard and extra audit is active on target/stage124-focused-cargo. Panic-format runs last. Its x86 kernel comparison is active at target/scenarios/memory-20261023-154811-7961032/stderr.log on target/stage118-profile-cargo. The kernel keeps the stage151 limits, including 2400 seconds and 10240 MiB. Scenario checks keep 1200 seconds. No kernel source or expected result changed.

The stage151 x86 run is terminal. It reached the 2400-second analysis time limit. Total measured time was 2535.534 seconds, and peak checker private memory was 2891091968 bytes. Its evidence marks incomplete=true and diagnostics_complete=false. No partial result is accepted. The stage151 ARM process was verified active with PID21604 at 1567.819 seconds of kernel-process time, with 2688692224 private bytes. It continues without a restart. These results replace the older active-x86 statements below.

The next candidate, stage152, will limit dependency-set repair to the affected part of the active request. The first native stack sample found all three active x86 workers in Solver::require, called from flow.rs:2088 after active.required.clear(). That code rebuilds the whole root dependency set after each removal batch. The second sample found control-path allocation work instead; the samples do not prove that dependency repair is the only cost. Transcripts are stage151/kernel-stack-1.txt and kernel-stack-2.txt. The first transcript was resolved with the matching release PDB, GUID 9364FBE5-9A75-4C33-9F94-BFF1FDFE6BD7. The debugger was detached after each sample. These samples paused the x86 process briefly, so its wall time includes that overhead.

For an edge removed from the current demand, collect its dependent region through the remaining input edges. The request root and the current demand remain reachable; their surviving direct inputs also remain reachable. Do not collect those anchors. In the affected region, find nodes with a surviving required user outside that region. Remove the region from the required set, then restore nodes reachable from those boundary nodes. A disconnected cycle must not keep itself required. Keep the existing demand values and completion rules. Filter the pending queue against the repaired set. Retain all limits and account for the temporary sets.

```rust
let anchors: Vec<_> = affected.iter().copied().filter(|input|
    self.flow.demands[*input].users.iter().any(|user|
        self.flow.active.as_ref().unwrap().required.contains(user)
            && !affected.contains(user))).collect();
for input in &affected {
    self.flow.active.as_mut().unwrap().required.remove(input);
}
for input in anchors {
    self.require(input)?;
}
```

Add work counters for dependency repair. Compile and run the existing focused and annotation checks, then the full non-panic audit and panic-format last. Compare kernel work at the same limits. No new test, source comment, wrapper helper, kernel annotation, or expected-result change is part of this correction. Stage151 remains frozen and its active x86 and ARM runs must finish without a restart.

Stage151 is the current candidate. Its driver SHA256 is AF2DB55590B99FCD43332B4881FB4CFC74DD6EA16469B5515A7CDBF0FEB39FC5. All nine saved source files match the working source. It passes compiler checking, release compilation, all 21 focused checks after the separate disk-failure repair, and all 18 annotation checks. Annotation results are in target/contract-checks/20261023-145036-5021203/results.json. Formatting repair results are in target/control-summary-20261020/stage151/focused-repair-results.json. The original failed build logs and focused report are retained.

Stage151 passed all 113 non-panic cases and 45 cache checks. Standard time was 516.745 seconds, and extra time was 158.234 seconds. Panic-format ran last on target/stage124-focused-cargo. Its safe and cache checks pass. Negative fails with six Unknown errors and one Forbidden error in target/scenarios/20261023-150308-9087744/recursive-bindings-negative.log. The Unknown reasons are three TransferLimit stops and three AlternativeLimit stops. The Forbidden binding is established through panic_handler and Sink::write_str; its path has a reachability uncertainty note. The run used 1574803437 transfers, 3407 frames, and 139 call-input revisions. This is not a passing panic-format result.

The full kernel run is target/scenarios/memory-20261023-145204-0075631/stderr.log on target/stage118-profile-cargo. Its analysis time allowance is 2400 seconds. Query, scalar, alternatives, call depth, scratch, and memory limits are unchanged. Actual kernel settings are saved in stage151/kernel-settings.json. Scenario checks retain the 1200-second allowance. No complete kernel result is established. No kernel source, scenario expectation, test, or source comment was changed during these stages.

The stage151 ARM kernel validation is also active. Its log is target/scenarios/memory-20261023-151940-5606894/stderr.log, and its build directory is target/stage151-arm-kernel-cargo. Settings are in stage151/kernel-arm-settings.json. It uses the same driver, memory cap, and 2400-second analysis allowance as x86. The ARM evidence directory is stage151/kernel-arm-evidence. This is a compiler analysis run, not a kernel boot.

The x86 worker whose final progress file shows 233200 queries has finished its traversal. Its terminal counters are 233260 queries, 14024681207 transfers, 106383 frames, 323448 sites, and 133 call-input revisions. diagnostic_cutoff is false. Three other workers were still active at this observation. This proves completion of one worker's traversal, not completion of the kernel check. Peak sampled private memory at 1909.566 seconds of kernel-process time was below 2.9 GB. If the remaining workers reach the 2400-second time limit, a further kernel-only time extension is justified by this progress. Keep the same binary and all other limits; do not restart the active run early.

The kernel driver was verified active at 512 seconds of kernel-process time, with 2156412928 private bytes. Its timing trace identifies repeated Argument::fmt and interrupts_enabled queries as large costs. These interrupt helpers load callbacks from AtomicUsize statics; they are not merely assembly sites. This trace does not establish a new solver defect. The driver must reach a terminal result before its full-run status can be determined.

At 1170.844 seconds of kernel-process time, the kernel driver was still active with 2487455744 private bytes. Backend coverage records mark the integer-to-function-pointer conversions in interrupts_enabled, interrupts_disable, and interrupts_enable as incomplete. The call coverage report marks their receivers unknown. These gaps prevent the complete-singleton binding shortcut. Worker3 has started another diagnostic graph pass: its query count increased from 56300 to 132700 while graph nodes fell from 27307 to 5555. The main work loop clears the diagnostic graph when call-input generation changes. This is evidence of repeated graph work, not a count of all graph passes or proof that every revision restarts the graph. Windows refused a CPU profile request; no recording remained active.

Lossless compression of stage119-focused-cargo and contract-checks-cargo is complete. Their allocated data sizes are 14275106770 and 13356744959 bytes. The free-space sample after compression was 30538743808 bytes. No files were deleted. A separate cleanup command was supplied for three inactive build caches, but it was not executed by the agent. Do not use those caches for new runs without checking their current state.

Stage149 kernel validation reached the 1200-second analysis time limit. Total measured time was 1308.663 seconds, and peak checker private memory was 8971526144 bytes. Stage150's kernel log writer failed when the disk filled up. The driver exited, but the wrapper waited on its blocked stderr forwarding thread. After verification that no Cargo or checker worker remained, the wrapper was stopped. The runner then reported the disk-space IOException. That run has no valid completion result and must not be counted as a time-limit or complete-kernel result. Its last memory samples were below 2.5 GB, but terminal diagnostics were lost.

Stage150 panic-format negative completed with seven Unknown errors and one Forbidden error. It recorded 795 widened call-summary observations. Its log is target/scenarios/20261023-143531-8880088/recursive-bindings-negative.log. This exercises the new bound, but it is not an accepted scenario result.

Stage151 contains the proof-status correction below. Compiler checking and release compilation pass. Its six recursive focused groups passed all 18 checks. Ordinary formatting could not build because core metadata writes failed with disk error 112, so its failed run is not checker-regression evidence. The logs are target/scenarios/20261023-144323-6704022/formatting-safe.log and formatting-negative.log. Rerun formatting after space recovery. Preserve the original focused report and record the repair separately.

The measurement script now checks whether its asynchronous log-copy task has failed inside the process loop. A log-write failure raises the original error and runs cleanup instead of waiting forever for a blocked wrapper. No test or source comment was added. Entire inactive stage119-focused-cargo and contract-checks-cargo caches are being compressed, including directory attributes for new files. No files were deleted. The free-space sample after the wrapper check exceeded 9 GB.

Stage150 passed all 113 non-panic cases and 45 cache checks. Standard time was 536.921 seconds and extra time was 187.136 seconds. Panic-format and its full kernel run remain active on the frozen binary. The kernel has remained below 2.3 GB private memory through 878 seconds of kernel-process time, but has not completed.

Source review found a proof-status error in stage150. Its widened incoming edge sets path_proven=false for the whole callee. This can also weaken an unconditional Forbidden path. The next candidate retains the original incoming path proof and supplies Unknown for every abstract entry argument, including integers and booleans. Those inputs must not become symbolic root witnesses. The existing feasibility code then marks input-dependent conditions Unresolved, while independent or unconditional paths retain their proof. The diagnostic note about lost caller inputs remains. No annotation may be added to hide this proof-status error.

```rust
if frame.unknown_entry {
    vec![Alternative {
        value: Value::Unknown("the caller inputs are not retained in this function summary"),
        guards: Vec::new(),
    }]
}
```

The separate stage150 annotation run at one alternative passed all 18 cases, but its counters show no caller-input widening. It validates the existing annotation rules under that setting, not the new widening path. The one-alternative ordinary formatting run also did not reach that path: negative stopped with two AlternativeLimit diagnostics and widened=0. Its log is target/scenarios/20261023-142718-9286150/formatting-negative.log. Do not use either run as evidence that the caller-input bound is exercised. Main validation remains at 128 alternatives.

Eight inactive policy directories under target/stage124-focused-cargo/irq-check-policy and eight under target/stage119-kernel-cargo/irq-check-policy were compressed in place. Each target was checked against its exact parent and live process command lines. No file was deleted. Saved logs, source snapshots, checker binaries, and active policy directories were retained.

Stage150 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Its driver SHA256 is C2CF58A544D342D9E2307DDD8BF08204DAA4815CEFB2B28B8BE59081BC30B6E5. All nine frozen source files match the working files. Annotation results are in target/contract-checks/20261023-142135-6293756/results.json. The full standard and extra audit is active on target/stage124-focused-cargo and will run panic-format last. The full kernel log is target/scenarios/memory-20261023-142337-8589845/stderr.log and uses target/stage118-profile-cargo. The stage149 full kernel comparison remains active at target/scenarios/memory-20261023-142047-7600055/stderr.log on target/solver-refactor-kernel-cargo. Both kernel runs retain the same limits and timing/storage traces. No kernel source was changed.

Stage148 kernel validation stopped at the memory limit after 1202.657 seconds including build work. Peak checker private memory was 9395228672 bytes. Panic-format negative reports five Unknown errors and one Forbidden error, with 1559515066 transfers and 3847 frames. No complete kernel result is established by stage148. Stage149 passed all 18 annotation checks as well as its 21 focused checks. The annotation report is target/contract-checks/20261023-141915-8998995/results.json.

Stage149 passed compiler checking, release compilation, and all 21 focused checks. Annotation checks are active. Its driver SHA256 is A7EF16E86588AB82A3EB28D553ED26AF7B9055E6BB1D0356A13377A70D0FF366. The change shares only input-independent diagnostic graphs. It does not bound the remaining caller-specific graph.

The next step bounds diagnostic specializations per function with the existing alternative allowance. Already registered caller inputs keep their own frames. A new caller beyond that allowance uses a separate summary entry with unknown inputs. Precise memory-effect reads still construct their own matched call frames. The summary is an over-approximation, not a completed precise answer or a safe summary. The incoming diagnostic edge records lost caller inputs and retains its own path proof. An input-dependent unresolved condition makes a later Forbidden path uncertain; an unconditional Forbidden path retains its proof. An unresolved callback remains Unknown. Pointer candidates do not become established targets. The full non-panic and annotation gates must still pass.

```rust
let widened = !diagnostic_independent.contains(&target)
    && inputs.len() >= limits.alternatives.max(1)
    && !inputs.contains_key(&point);
let path_proven = value.path_proven();
```

The generic entry must not receive the concrete caller as a flow input. Otherwise the entry would join precise and unknown activations and lose the boundary between the diagnostic summary and pointer-effect analysis. The diagnostic graph uses the same generic entry for later calls, while the precise solver retains its current frame rules. This implements the bounded-specialization step of the approved summary plan. Validation must report any new conservative diagnostics instead of hiding them with annotations.

Stage148 passed all 113 non-panic cases, 45 cache checks, 21 focused checks, and 18 annotation checks. Standard time was 448.827 seconds and extra time was 156.652 seconds. Its driver SHA256 is F4F849562AE9EC73EB2AA1DA28F32BFE3D10AF4465F22AF60E7EA89E293C6565. The full kernel run remains active at target/scenarios/memory-20261023-135913-1147545/stderr.log. Panic-format runs last at target/scenarios/20261023-140908-4587521. Stage147 passed 113 non-panic cases and 45 cache checks, but its kernel run stopped at the memory limit after 964.225 seconds, with 9382879232 peak checker private bytes. These results supersede the older active-run statements below.

The next change adds a shared diagnostic graph only for a closed set of functions with fixed call targets and no caller-dependent control flow. The existing binding-dependence scan supplies the call edges. Branches, assertions, exclusions, and unsupported control terminators exclude a function and its callers from this set. A diagnostic root has no caller input. It cannot replace the separate call frames used for pointer values or memory effects. Unknown diagnostics and Forbidden targets remain in the shared graph. Incoming edges retain their own path proof and annotation permission.

```rust
let context = if diagnostic_independent.contains(&target) && !leaf {
    *diagnostic_frames.entry(target).or_insert_with(|| solver.frame(Frame {
        instance: target, caller: None, irq: true, unknown_entry: true,
    }))
} else {
    solver.frame(Frame { instance: target, caller, irq: true, unknown_entry: false })
};
```

The stage128 frame trace contains 47060 creations. Of these, 46104 have parent-chain depth below 20. This is evidence of broad path growth in that saved run, not a current measurement of stage148. The new change must pass compiler checking, the existing focused and annotation checks, the full non-panic audit, panic-format last, and kernel measurement at the same limits. No new tests, source comments, or kernel annotations are planned for this change.

Stage148 is an unvalidated evidence-sharing change on stage147. Source scope and successor data are shared by instance and MIR location. Identical loop-entry lists are shared by their full Point values. Call frames, guards, and bindings are not merged. The site-state formatter retains the same fields and text. The retained-storage trace counts the shared records once. Compiler checking, focused validation, and annotation validation are required before the full audit and kernel run.

```rust
let mut site_sources = HashMap::<(Instance<'tcx>, Location), Arc<CallSiteSource>>::new();
let mut evidence_loops = HashSet::<Arc<[Point]>>::new();
```

Stage147 passed all 94 standard cases and 45 cache checks in 509.661 seconds. Its extra audit and full kernel run remain active. At worker2 query 40000, stage146 retained 50840540 site bytes and 5267718 precision-message bytes. Stage147 retained 41537144 site bytes and 83786 precision-message bytes. The two samples have the same 89008 call edges, but different frame counts, so this is not a whole-run speed comparison.

Stage146 passed all 113 non-panic cases and 45 cache checks. Its kernel run stopped at the analysis memory limit after 958.975 seconds including build work, with 9391587328 peak checker private bytes. Stage145 panic-format negative reports five Unknown errors and one Forbidden error, using 1602013638 transfers, in target/scenarios/20261023-132832-8162235/recursive-bindings-negative.log. Stage146 panic-format negative also remains a failed scenario; its log is target/scenarios/20261023-133644-3346360/recursive-bindings-negative.log.

Disk space fell to about 1.1 GB during validation. Old trace logs and inactive build-cache directories were compressed in place with NTFS compression. No file was deleted. Hash checks on the twelve selected large trace logs passed after compression. Active log files and active build-cache directories were excluded. The later free-space sample was 4580388864 bytes.

Stage147 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Its driver SHA256 is 41FC58BA7DCDD1DFE567C6A9A2050FF88D640C3724BA689A09D8526D2F594934. The annotation report is target/contract-checks/20261023-133207-9520162/results.json. The nine source snapshot files match the working files. The full non-panic audit is active on target/stage124-focused-cargo and runs panic-format last. The full kernel log is target/scenarios/memory-20261023-133730-3206956/stderr.log. The kernel uses target/stage119-kernel-cargo. Query timing and retained-storage tracing are enabled. Limits remain query 1000000, scalar 1000000, alternatives 128, memory 10240 MiB, and timeout 1200 seconds. No complete kernel result is established.

Stage145 kernel validation stopped at the analysis memory limit after 921.187 seconds including build work. Peak checker private memory was 9394814976 bytes. This result supersedes its earlier active-run status. Stage146 has passed all 94 standard cases and 45 cache checks; its extra audit and full kernel run remain active. No kernel annotation, scenario expectation, test, or source comment was added in stages145 through147.

Stage145 passed all 113 non-panic cases and 45 cache checks. Standard time was 477.048 seconds and extra time was 186.149 seconds. Its full kernel run remains active. Later retained-storage samples show 3349190316 call-site bytes across the four workers at their separate latest counters. These samples are not simultaneous, but they identify a large retained-storage component. Stage144 panic-format negative finished with five Unknown errors and one Forbidden error, using 3969352692 transfers. Its log is target/scenarios/20261023-130924-9320022/recursive-bindings-negative.log. Stage143 panic-format negative also reports five Unknown errors and one Forbidden error. Neither is a passing panic-format result.

Stage147 is an unvalidated evidence-storage change on stage146. The stage145 retained-storage trace records 301931752 site bytes in worker0 at query 190000. Call sites now retain typed query fields and format the same state text only for tracing, diagnostics, or saved evidence. Repeated precision-loss messages use one key and an observation count. The precision_loss evidence array keeps the unique messages, and precision_loss_observations supplies the counts. The unknown_coverage statistic still counts observations. No binding, guard, error, unknown effect, or acceptance decision is removed.

```rust
struct QueryEvidence {
    steps: usize,
    attempts: usize,
    cases: usize,
    loop_entries: Box<[Point]>,
    cut_recursive_returns: bool,
    witness: Option<u128>,
    result: Result<[usize; 4], QueryStop>,
    conservative_safe: bool,
    worker: Option<usize>,
}
```

Stage146 passed compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks. Its driver SHA256 is 4C6DB46EC387FA475789E33085544F2CE9DFDC9500F869048A33FAC783F2B942. The annotation report is target/contract-checks/20261023-132139-2662924/results.json. Its full non-panic audit is active on target/stage124-low-focused-cargo, with panic-format last. Its full kernel run is target/scenarios/memory-20261023-132444-6794637/stderr.log and uses target/stage118-profile-cargo. Limits are unchanged. Stage145's full kernel and audit runs remain separate and active.

Stage146 is an unvalidated proof-order change on stage145. In call_effect, a direct MIR target with a complete read-only proof can now preserve storage before caller-path refinement. The previous code used the same proof after paths(point, true). Heap initialization, recursive-return witnesses, explicit return exclusions, unwind mode, and unknown targets retain their existing handling. This does not alter the memory effects of assembly. The change must pass the focused and annotation checks before full validation.

```rust
if !self.cut_recursive_returns && address.object != Object::Heap(point)
    && (unwind || !self.excludes_return(point, instance))
    && self.read_only(instance, !unwind)?
{
    return Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }]);
}
```

Stage145 passed compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks. Its driver SHA256 is 81DA14FD9CDD7230B082564B7B182DCE111E7BBD4523538F72A506EA43665293. The annotation report is target/contract-checks/20261023-131527-7933173/results.json. Its full non-panic audit is active on target/stage119-focused-cargo, with panic-format last. Its full kernel run is target/scenarios/memory-20261023-131739-5843099/stderr.log. This run also records query timing and retained storage. It has no separate measurement timeout. The analysis timeout remains 1200 seconds and the memory limit remains 10240 MiB.

Stage145 is an unvalidated caller-path scope change on stage144. The stage144 binding trace at target/scenarios/memory-20261023-130154-8783100/stderr.log:274117 shows a required interrupts_enabled callback read that reaches PANIC_RUNTIME_READY, then run_utf8_validation and raw_cpuid. Its full query stops at 9000000 steps. The new scope applies to added caller-path guards on static-object reads, as well as the concrete metadata case retained in stage136. Guards already required by the value or target remain required. Local objects, cursor arguments, and selector inputs keep the existing rules. A stopped path proof retains its cause and unresolved guard; it does not replace a pointer value or claim unchanged memory. Validation must include the same 21 focused checks, 18 annotation checks, full non-panic audit, panic-format last, and a full kernel run.

```rust
let scoped_path = matches!(storage, Some(Object::Static(_)))
    || storage.is_none() && values.iter().all(|value|
        matches!(value.value, Value::Metadata(Metadata::Concrete(_))
            | Value::Pointer(_, Metadata::Concrete(_))));
```

Stage144 passed all 113 non-panic cases and 45 cache checks. Its kernel run stopped at the analysis memory limit after 886.991 seconds including build work, with 9392582656 peak checker private bytes. The 300-second binding trace is separate from that full run. It stopped at its measurement time limit with 2025115648 peak checker private bytes. Stage143 also passed all 113 non-panic cases and 45 cache checks. Their panic-format runs remain separate from these counts.

The stage144 full kernel log is target/scenarios/memory-20261023-125758-4628709/stderr.log. The measurement file in that directory records its process tree and memory samples. It is a full validation run, not a 300-second diagnostic trace.

Stage144 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Its driver SHA256 is 8040D8936D8CB0E19DCBC50CB5BA7B865139096D722784F98EDD7C6E65516C3A. All nine source snapshot files match the working files. The annotation report is target/contract-checks/20261023-125611-7423206/results.json. Its full kernel validation and full non-panic audit are active. The kernel uses target/solver-refactor-kernel-cargo. The audit uses target/stage124-focused-cargo and runs panic-format last. Limits remain query 1000000, scalar 1000000, alternatives 128, memory 10240 MiB, and timeout 1200 seconds. No complete kernel result is established.

Stage143's full audit remains active on target/stage124-low-focused-cargo. Stage141 panic-format remains active on target/stage119-focused-cargo. These runs use frozen binaries. Stage143's bounded trace ended at the separate 300-second measurement limit, with 2409906176 peak checker private bytes. At worker0 query 40000, interrupts_enabled bb3[11] used 67.497 seconds across 858 query records. Argument::fmt bb3[10] used 35.069 seconds across 160 records. The reverse index removes a confirmed whole-graph scan, but this trace does not prove that it resolves the main kernel cost.

Stage144 is the next candidate. It groups already-covered call records by instance, MIR location, and proof type. The previous representation stored a site and candidate edges for each frame, even though those candidate edges do not participate in diagnostics. The new covered_call_summaries evidence records the proof and observation count. It explicitly says that coverage is not an established binding. Bound edges, Forbidden paths, Unknown paths, and accepted gaps keep their existing representation. Compiler checking passes; release compilation and validation are pending.

```rust
*output.covered_calls.entry((instance, location, proof)).or_default() += 1;
```

Stage143 passed all 21 focused checks and 18 annotation checks. Its driver SHA256 is 2794EE6DFCBAD11ED490097812E31AB625D1A61B892B7C01673F7240530F5F8C. The annotation report is target/contract-checks/20261023-124954-1982440/results.json. Stage141 kernel validation stopped at the analysis memory limit after 945.234 seconds including build work, with 9390788608 peak checker private bytes. Its specialized safety summaries were used, but they did not produce a complete kernel result. No kernel annotation or scenario expectation was changed.

Stage143 is the current candidate. It adds the reverse call-input index described below. Compiler checking and release compilation pass. Focused and annotation checks are active. The bounded kernel timing trace is target/scenarios/memory-20261023-124943-9643160/stderr.log. The full stage141 kernel run is still active; it has not been replaced or restarted.

Stage141 passed all 113 non-panic cases and 45 cache checks. Standard time was 522.778 seconds and extra time was 172.967 seconds. Panic-format runs last and is active. Stage142's bounded timing trace ended at the separate 300-second measurement limit, with 2264698880 peak checker private bytes. At worker2 query 30000, interrupts_enabled bb3[11] accounts for 44.630 seconds across 749 query records, and Argument::fmt bb3[10] accounts for 24.259 seconds across 140 records. This identifies required callback bindings as another remaining cost; optional safety probes took 205 ms and had 1983 hits.

Stage142's timing trace is target/scenarios/memory-20261023-124324-6045233/stderr.log. At worker2 query 10000, 4791 scalar safety probes have 689 hits and take 74 ms. The largest site cost is core::fmt::write bb2[6]: 70 completed query records take 5493 ms and 34778411 transfers. Setup and fallback each round down to 0 ms for that site. These are cumulative site measurements, not a complete kernel result.

Source review found a whole-graph scan in forget_loop_guards. Each call scans all flow.inputs to build caller-to-callee links before it visits the loop's callees. The next change maintains the same reverse links in flow_input, the sole insertion point for call inputs. Loop guard removal can then visit only the selected loop blocks and their callees. It must keep the same reachable set and guard rules; it must not merge frame identities or remove input guards.

```rust
callees.entry(caller.frame).or_default().push((caller.location, frame));

for &(location, callee) in callees.get(&frame).into_iter().flatten() {
    if blocks.contains(&location.block) {
        pending.push(callee);
    }
}
```

The working source is stage142, which adds optional query timing to stage141. It separates query setup, fallback, and scalar safety proof time under IRQ_CHECK_TRACE_TIME. Compiler checking and release compilation pass. The full stage141 audit and kernel run remain active on their frozen executable. Windows CPU sampling could not start because the session could not enable the profiling policy. No policy was changed, and no Windows recorder remains active.

Stage141's bounded trace finished at the 300-second measurement limit with 2408259584 peak checker private bytes. This is close to stage139's bounded trace and is not evidence of a whole-kernel speed improvement. The stage141 full kernel log is target/scenarios/memory-20261023-123444-8103633/stderr.log.

Stage138 panic-format negative finished with five Unknown errors and one Forbidden error with an uncertainty note. It used 1779047054 transfers and 4100 frames in target/scenarios/20261023-122448-7346151/recursive-bindings-negative.log. The reported service call is reached through the panic handler. This is still a failed scenario because the extra Unknown errors remain.

Stage141 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. The driver SHA256 is B44946878599F40CF62DD98D5B7901D9E2459CA12349DFCEBA4E306F2D884D20. The annotation report is target/contract-checks/20261023-123246-2820231/results.json. Its full non-panic audit is active on target/stage119-focused-cargo, with panic-format last. Its full kernel validation uses target/solver-refactor-kernel-cargo. A separate 300-second diagnostic trace uses target/stage119-kernel-cargo and target/scenarios/memory-20261023-123206-2698163/stderr.log. No full kernel result is established.

Stage141 stores call safety with each completed scalar summary. The main solver uses a 4096-step optional proof before it expands a frame. If that proof stops or cannot establish safety, the existing queries still run. Pointer effects are unchanged. The evidence lists specialized safe frames separately from globally safe functions. The scalar keys still include the instance, IRQ mode, backend context, and scalar inputs. Incomplete scalar branches cannot establish call safety. Ordinary formatting negative passes with 11325080 transfers and 106 frames; its 15 safety probes have no hits. That case does not establish a benefit from summary reuse.

Stage138 passed all 113 non-panic cases and 45 cache checks. Its kernel run stopped at the analysis memory limit after 933.262 seconds including build work, with 9390321664 peak checker private bytes. Its log is target/scenarios/memory-20261023-121341-9268377/stderr.log. Stage136 panic-format negative finished with six Unknown errors and no Forbidden error. It used 3469344144 transfers and 4933 frames in target/scenarios/20261023-120959-5502047/recursive-bindings-negative.log. Stage138 panic-format is still active.

Stage140 passed all 21 focused checks and 18 annotation checks. It keeps fixed diagnostic messages as borrowed strings. Its annotation report is target/contract-checks/20261023-123115-0484077/results.json. Stage139's bounded trace ended at its separate 300-second measurement limit, with 2396778496 peak checker private bytes. Neither result is a complete kernel validation.

Stage139 measures retained graph storage without a semantic change. Its bounded kernel trace is target/scenarios/memory-20261023-122344-3195164/stderr.log. At 10000 worker queries, atomic_load::<u8, false> has 463 to 496 frames. Completed scalar summaries already distinguish constant ordering arguments, but the main graph has no completed call-safety result for those summaries. Stage140 changes fixed diagnostic text to borrowed strings; it does not change analysis policy. Its focused audit is active.

The next change adds a call-safety result to the existing finite scalar summaries. It must be false for unresolved calls, assembly, unresolved assertion failure, and incomplete scalar dependencies. A true result can skip repeated diagnostic traversal for that frame only. It must not mark the function safe for all inputs or change pointer effects. Existing input and context keys remain in use. A stopped summary proof keeps the existing query path.

```rust
struct Summary {
    returned: Fact,
    conditions: HashMap<Location, Fact>,
    arguments: HashMap<Location, Box<[Fact]>>,
    calls_safe: bool,
}

if matches!(solver.scalar_summary(point), Ok((_, true))) {
    continue;
}
```

Stage136 kernel validation stopped at the analysis memory limit. It did not reach the timeout. The measurement took 991.114 seconds and peaked at 9387503616 checker private bytes. The four workers retained 881973 frames and 4827757 site records in total. The log is target/scenarios/memory-20261023-115858-1893950/stderr.log. Stage138 kernel validation is still active. The next diagnostic change adds retained graph storage and frame counts under IRQ_CHECK_TRACE_MEMORY. It does not change solver results.

Stage138 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Its driver SHA256 is 6B4BCB59F9D4E68A1C5DBD45716AAADC0C78476A004261E2ED387E70C93C6E1C. The working source matches its frozen source files. Reports are target/control-summary-20261020/stage138/focused-results.json and target/contract-checks/20261023-121135-8817477/results.json. Ordinary formatting negative passes, but uses 11865104 transfers and 144 frames, compared with stage136's 7620682 transfers and 63 frames. A smaller optional-proof allowance is not a measured whole-analysis speed improvement yet.

The stage138 full audit is active on target/stage124-low-focused-cargo, with panic-format last. This directory name does not change the audit settings: scalar_steps remains 1000000. Kernel validation is active at target/scenarios/memory-20261023-121341-9268377/stderr.log and uses target/solver-refactor-kernel-cargo. The analysis timeout is 1200 seconds and the process memory limit is 10240 MiB. No kernel source annotation, scenario expectation, test, or source comment was added in this continuation.

Stage136 passed all 113 non-panic cases and 45 cache checks. Standard time was 483.976 seconds and extra time was 167.089 seconds. Its kernel and panic-format runs remain active. A complete kernel result is still unproved.

Stage138 keeps the full scalar allowance for optional proofs in functions that contain or transitively reach an explicit exclusion. Other optional control and assertion proofs keep the 4096-step cap from stage137. The existing exclusions set supplies this distinction; no function name or scenario name is used. Stage137 passed 20 of 21 focused checks and all 18 annotation checks. Only recursive cut negative failed. Its log, target/scenarios/20261023-120721-7539260/recursive-bindings-negative.log, reports an uncertain Forbidden after the condition at Task::drop stops. The recursive return proof is needed to apply the explicit exclusion before service. Stage138 therefore preserves that proof's prior allowance. Compiler checking and focused validation are pending.

```rust
let allowance = if self.facts.exclusions.contains(&self.frames[point.frame].instance) {
    limits.scalar_steps
} else { limits.scalar_steps.min(4096) };
let allowance = allowance.min(self.budget.limit.saturating_sub(before) / 2);
```

Stage137 driver SHA256 is 1CB7E9C53370A21A0E9E4FB5A042FDD817679F3AA8B7A54584E8F2C2E3CBA70D. Its annotation report is target/contract-checks/20261023-120731-5768267/results.json. Stage136 has passed all 94 standard cases and 45 cache checks; its extra audit and kernel run remain active. Stage133 panic-format negative finished with six Unknown errors and no Forbidden error. Its main solver used 3463129745 transfers and 4953 frames in target/scenarios/20261023-114653-5190162/recursive-bindings-negative.log. This is not a passing panic result.

Stage137 tests a separate work cap for optional control and assertion proofs. Their allowance is min(scalar_steps, 4096, remaining_query_steps / 2). Required pointer, selector, and scalar reads retain their existing allowances. The first coarse control pass and completed scalar summaries remain available. A stopped optional proof retains the conservative path and its uncertainty; it does not store unknown pointer effects as unchanged effects. This differs from stage124-scalar1k, which reduced the scalar allowance for required binding work too. Compiler checking and equal-limit focused validation are pending.

```rust
let allowance = limits.scalar_steps.min(4096)
    .min(self.budget.limit.saturating_sub(before) / 2);
```

The completed stage136 bounded trace took 300.867 seconds including build work and peaked at 2016313344 checker private bytes. Its required core::fmt::write queries at bb2[6] return bound targets at 2380542 and 2384382 steps; corresponding stage133 site queries reached 9000000 steps. The trace still records repeated optional proofs near 500000 steps in Scheduler::handle_switch_out and core::fmt::write. poll_rx_once at bb41[1] has 746 query records across 362 frames. Reported step totals include resumed queries and must not be treated as exclusive costs or a whole-kernel speed ratio.

Stage136 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. Its driver SHA256 is C0927535043D4F9B2360098BEC75D127A23E6CC0262BBE2D84E6EBF07D356234. Reports are target/control-summary-20261020/stage136/focused-results.json and target/contract-checks/20261023-115726-3490580/results.json. Ordinary formatting negative reports its Forbidden error without an extra Unknown error, using 7620682 transfers and 63 frames. Its log is target/scenarios/20261023-115803-0775412/formatting-negative.log.

The stage136 full audit is active on target/stage119-focused-cargo, with panic-format last. Kernel validation is active at target/scenarios/memory-20261023-115858-1893950/stderr.log and uses target/stage118-profile-cargo. The bounded trace is target/scenarios/memory-20261023-115745-8888811/stderr.log and uses target/stage119-kernel-cargo. Validation limits are unchanged. The trace has a separate 300-second measurement limit and is not validation. No kernel annotation or scenario expectation was changed.

Stage133 kernel validation reached the 1200-second analysis timeout. The measurement took 1350.819 seconds and peaked at 7012782080 checker private bytes. The log states that no partial result is accepted. The scalar-argument cache improved the measured atomic control templates, but did not produce a complete kernel result. Stage133 panic-format negative is still running separately from its passing non-panic audit.

Stage136 restricts the new caller-path scope to argument values that already carry concrete vtable metadata. It does not apply to arbitrary memory reads, scalar inputs, thin pointers, unknown metadata, or argument indexes. Stage135 passed 20 of 21 focused checks and all 18 annotation checks, but ordinary formatting negative failed with two Unknown errors plus its Forbidden error. Its trace at target/scenarios/20261023-115419-7140529/formatting-negative.log:1002 shows a new stopped condition at core::fmt::write bb9[5]. That condition reads the format-template byte through the cursor. It participates in selector evaluation, so a general caller-path rule is insufficient. The trace also identifies the 16 MiB per-function alternative-storage stop at line 1017. Stage136 retains required evaluation for these input kinds.

```rust
let concrete_metadata = storage.is_none() && values.iter().all(|value|
    matches!(value.value, Value::Metadata(Metadata::Concrete(_))
        | Value::Pointer(_, Metadata::Concrete(_))));
```

Stage135 driver SHA256 is 9A5B75AD462DA55571D009EB6A86FB14EB674BBF5178A2DF03CE83C6721C8563. Stage134 passed 16 of 20 applicable focused checks and all 18 annotation checks; its failed global-update safe case did not produce a cache check. Its bounded kernel trace completed its measurement window after 300.692 seconds with 2019659776 peak checker private bytes. Neither stage134 nor stage135 has an accepted full audit or kernel validation result.

Stage135 replaces the stage134 experiment. Stage134 failed cut negative, global-update safe, and recursive formatting negative with alternative-limit errors. Skipping all added caller conditions loses useful pruning and is not retained. Stage135 still evaluates those conditions. It gives only added caller-path conditions a stopped-proof scope. Conditions already required by an argument value or selected target retain required binding work. The existing stop and retry code can leave a scoped path condition unresolved; it does not replace pointer values with Unknown. The new feasible_with_paths method has two callers and contains the existing feasibility solver. Its default feasible entry has multiple callers.

```rust
let optional_condition = self.controls.solving || self.controls.proving_assertion
    || self.cut_recursive_returns || matches!(value.value, Value::Function(_))
    || path_conditions.contains(&Point { frame, location: guard.branch });
```

Stage133 passed all 113 non-panic cases and all 45 cache checks. Stage132 panic-format negative failed with six Unknown errors and no Forbidden error in target/scenarios/20261023-112928-5877564/recursive-bindings-negative.log. Its main solver used 3523619968 transfers and 4911 frames. This is not a passing panic result. Stage134 validation and its bounded trace use its frozen binary; later source changes do not alter those runs.

Stage134 is an argument-boundary experiment on stage133. input_values keeps the guards from the argument value and the selected target. It does not require a second proof of the caller path to transport that value into the callee. Added caller-path guards remain present and pending, but are not required scalar reads. If the same guard is already required by the value, merge retains that requirement. These pending guards do not establish a proven path. The main call-site analysis still checks reachability. Compiler and scenario validation are pending.

```rust
for (_, guard) in &mut path_guards {
    guard.required = false;
}
```

The stage133 trace at target/scenarios/memory-20261023-113254-5602786/stderr.log:131900 shows a formatter vtable read depending on PANIC_RUNTIME_READY and UTF-8 loop conditions. Its core::fmt::write query at bb2[6] stops at 9000000 steps. The experiment separates call reachability from argument selection; it must not remove a guard that selects a pointer or registry entry. All existing focused and annotation checks are required before a full audit.

Stage132 kernel validation reached the 1200-second analysis timeout. The measurement took 1308.139 seconds and peaked at 5347917824 checker private bytes. It did not produce a complete result. Stage133 full kernel validation is still active. Its bounded diagnostic measurement ended after 301.149 seconds and peaked at 2018471936 checker private bytes; that measurement is not validation.

Stage133 is the current candidate. Compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks pass. It retains completed scalar argument facts at call sites and uses them to select callee control summaries. Previously scalar_condition always selected a summary with an empty argument list. A scalar summary could already compute a specialized callee result internally, but a later condition query in that callee did not select it.

The new selection combines all registered callers and all completed caller summary contexts. It keeps only type-compatible integer constants and enum variants. Unknown or incompatible inputs remain unknown. A missing caller summary, a cycle, or an unavailable call-argument record falls back to the generic summary. Selection uses an explicit work queue. Its frame cache follows the call-input generation and has a separate size bound. Pointer values and effects are not part of this scalar input cache.

```rust
if let Some(inputs) = &mut inputs {
    if inputs.len() != arguments.len() { complete = false; break; }
    for (input, &argument) in inputs.iter_mut().zip(arguments.iter()) {
        *input = input.join(argument);
    }
} else {
    inputs = Some(arguments.to_vec());
}
```

The stage133 driver SHA256 is 1D9273228CF8AB1413126AC1849E0D8B11C90E4D611BDE2035A3207E63BF9E47. Reports are target/control-summary-20261020/stage133/focused-results.json and target/contract-checks/20261023-113244-3435139/results.json. Ordinary formatting negative passes with 7599501 transfers and 63 frames in target/scenarios/20261023-113327-3602688/formatting-negative.log. The full audit is active on target/stage124-focused-cargo, with panic-format last. Kernel validation is active at target/scenarios/memory-20261023-113502-5743964/stderr.log and uses target/solver-refactor-kernel-cargo. Limits are unchanged.

The bounded stage133 trace is target/scenarios/memory-20261023-113254-5602786/stderr.log. It has the 300-second measurement limit and is not kernel validation. At lines 1013 and 1030, atomic_load receives the constant Ordering variant 2 and its branch is Bits(2). Its control template visits 4 of 13 blocks. The corresponding stage127 trace visits all 13 blocks. The kernel source calls these loads with Ordering::Acquire at libs/kernel_sync/src/locks/irq.rs:28, :40, and :52. This proves a reduction in these control templates, not a whole-kernel speed ratio.

Stage132 passed the complete non-panic audit: 94 standard cases, 19 extra cases, and 45 cache checks. Standard time was 438.230 seconds and extra time was 156.989 seconds. Its panic-format and kernel runs remain active. No kernel source annotation or scenario expectation was changed for stage133.

Stage132 is the current candidate. It passes compiler checking, release compilation, all 21 focused checks, and all 18 annotation checks. It limits condition-stop fallback to optional control, assertion, and witness work, or to the path of an already established function target. A condition needed to select a pointer or argument keeps required-binding evaluation. The query keeps its spent allowance when it repeats after a condition stop.

The driver SHA256 is 2BEB28098FE685ED6927165610E3FFB19BF87122977CC3691CA3146829C374DC. Reports are target/control-summary-20261020/stage132/focused-results.json and target/contract-checks/20261023-111725-2101293/results.json. Ordinary formatting negative reports its expected Forbidden error without an extra Unknown error. It uses 7532011 transfers and 63 frames in target/scenarios/20261023-111759-2473372/formatting-negative.log. This is more work than stage127 for this case; no whole-kernel speed improvement is claimed.

The stage132 full audit is active, with panic-format last. Its Cargo target is target/stage119-focused-cargo. Kernel validation is active at target/scenarios/memory-20261023-111923-1296995/stderr.log, with evidence in target/control-summary-20261020/stage132/kernel-evidence. The kernel Cargo target is target/stage118-profile-cargo. All high-allowance settings remain unchanged: 1200 seconds, 10240 MiB, query_steps=1000000, scalar_steps=1000000, alternatives=128, and call_depth=256. No kernel source annotations were added. A complete kernel result is still required.

```rust
let condition_key = (self.controls.solving || self.controls.proving_assertion
    || self.cut_recursive_returns || matches!(value.value, Value::Function(_)))
    .then(|| control::ConditionKey {
        point, generation: self.flow.generation,
        required_binding: !self.controls.solving
            && !self.controls.proving_assertion && !self.cut_recursive_returns,
        choices, inputs, witnesses, loop_entries: self.loop_entries.clone(),
        cut_recursive_returns: self.cut_recursive_returns,
    });
```

Stage130 separates required-query and optional-query condition stop records and restores required global-entry searches. Global-update and boot-registry pass safe, negative, and cache checks again. Ordinary formatting negative still fails. Stage131 also restores required loop and array-index handling. It passes 20 of 21 focused checks and all 18 annotation checks; ordinary formatting negative still fails. Its driver SHA256 is E7CA9CCFBB89B9FD06B8A4BEAD52FDFA020CCAA38D673E93557E3286A00ADFA3. The trace at target/scenarios/20261023-111332-6755271/formatting-negative.log shows a required formatter argument read under condition bb7[9] in core::fmt::write. Its prerequisite chain includes the template byte and cursor pointer. This is selector work, not only a proof that a known function call occurs. Stage132 therefore does not give that selector a condition-stop fallback.

Stage127 kernel validation finished at the 1200-second analysis limit. Its measurement took 1345.146 seconds including build work and peaked at 4823715840 checker private bytes. Evidence has incomplete=true and diagnostics_complete=false. The four main workers created 118934, 122515, 58404, and 202835 frames. No partial result was accepted. Stage124 and stage125 panic-format negative each still report four unknown paths and one Forbidden error. Their logs are target/scenarios/20261023-104213-5971614/recursive-bindings-negative.log and target/scenarios/20261023-104638-1180260/recursive-bindings-negative.log.

The working source has an unvalidated branch-request correction after stage129. A feasibility condition now gives its queued dependencies a separate ConditionKey scope. This scope is not the scalar budget owner. When a scoped proof stops, the solver records the condition and cause without storing an unknown pointer value. A required binding request can repeat with that condition unresolved. The repeat keeps its spent query steps. Process limits and main-query transfer stops still propagate. Completed scalar summaries can also answer feasibility conditions outside the control-summary solver.

Stage129 passed all 18 annotation checks but only 14 of 19 applicable focused checks. Both global-update cases, both boot-registry cases, and ordinary formatting negative failed. The failed safe cases did not produce their cache checks. Its driver SHA256 is 72EBD1CD8F039E116330EAEBA1D0BBB99EE6AB2A47F2E6D5DFCCEC44CD652F91. This is not an accepted candidate. The registry logs show an early ControlMemory cause, an unknown storage address, and an uncertain forbidden path. The ordinary formatting log reports TransferLimit and AlternativeLimit without its expected Forbidden error. Reports are target/control-summary-20261020/stage129/focused-results.json and target/contract-checks/20261023-110532-1794365/results.json.

The next correction separates optional and required condition stops in ConditionKey. Required selector reads retain the global-entry search. The early ControlMemory boundary applies only to the existing optional control, assertion, and witness scopes. The stage129 bounded kernel trace is target/scenarios/memory-20261023-110606-3092005/stderr.log; it is diagnostic only.

```rust
condition_origin: Option<Arc<control::ConditionKey>>,
```

```rust
if mode == RequestMode::RequiredBinding
    && self.controls.stopped_conditions.len() > stopped_conditions
    && self.flow.active.is_none()
{
    self.controls.epoch += 1;
    self.retain_completed();
    continue;
}
```

The actual retry also checks the stop reason, the active refinement limit, and the control solver state. It does not retry a process stop. This closes a scope gap: feasible could start a condition read within required target work, but the queued prerequisites did not retain a condition-proof scope. The stage127 trace contains required core::fmt::write requests at bb2[6], bb29[14], and bb13[13] that stop at 9000000 steps while reading PANIC_RUNTIME_READY through earlier calls.

Stage127 has now passed all 113 non-panic cases and all 45 cache checks. The standard run took 537.124 seconds and the extra run took 190.412 seconds. Its full kernel run remains active. Stage125 kernel validation finished at the analysis timeout; total time was 1378.600 seconds and peak checker private memory was 3152461824 bytes. No partial result was accepted.

The trace-only stage128-trace snapshot adds frame creation records to stage127. Its driver SHA256 is 8754071FB392144D71A525BAC14B5BEFC55F4EF7536D647D49318201E247C141. Its bounded kernel trace is target/scenarios/memory-20261023-105705-4209235/stderr.log. The measurement ended after 300.895 seconds, including build work. It is not kernel validation. Frame duplication exists for the same static caller and backend context. Frame merging is not implemented: the diagnostic graph must also keep each input relation, or shared outgoing edges could mix different callers. The trace instrumentation is absent from the current semantic candidate.

Stage127 is the last candidate with a complete passing non-panic audit. Compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. An optional control, assertion, or witness proof stops before splitting an unknown array index into more cases than the configured alternative allowance. The stop is AlternativeLimit at the index read. Required binding requests retain their existing index handling. Panic-format runs last.

The stage127 driver SHA256 is 12F958DE1DDFA6A727107B6F98B5F52458202A5F7BC6D8533DE224B2F0089EA9. Reports are target/control-summary-20261020/stage127/focused-results.json and target/contract-checks/20261023-104651-1378063/results.json. The ordinary formatting negative log is target/scenarios/20261023-104737-7065019/formatting-negative.log. Kernel validation is active at target/scenarios/memory-20261023-104934-1842120/stderr.log. A separate bounded trace uses target/scenarios/memory-20261023-104954-5296703/stderr.log. Both use the unchanged high-allowance settings. The bounded trace has an additional 300-second measurement limit and is not a validation run.

Stage123, stage124, and stage125 have each passed all 113 non-panic cases and all 45 cache checks. Their standard and extra result files are in their saved stage directories. Stage123 panic-format negative still fails with four unknown paths and one Forbidden error. Its evidence has incomplete=false and diagnostics_complete=true. The log is target/scenarios/20261023-103139-0274744/recursive-bindings-negative.log. Its safe and cache checks pass. The stage124 and stage125 panic-format runs are still separate from their non-panic counts.

Stage123 and stage124 kernel runs both reached the 1200-second analysis limit. They have incomplete=true and diagnostics_complete=false. Stage123 took 1332.068 seconds including build work and peaked at 2722500608 checker private bytes. Stage124 took 1379.668 seconds including build work and peaked at 2746925056 checker private bytes. Their logs are target/scenarios/memory-20261023-101616-0201978/stderr.log and target/scenarios/memory-20261023-102402-3963940/stderr.log. No partial result was accepted. These results supersede earlier in-progress notes below.

```rust
if (self.controls.solving || self.controls.proving_assertion || self.cut_recursive_returns)
    && length > limits.alternatives as u128
{
    self.controls.dependency_stop.get_or_insert((point, QueryStop::AlternativeLimit));
    return Err(QueryStop::AlternativeLimit);
}
```

The stage125 trace at line 41849 shows an optional run_utf8_validation query that spends 999136 steps. Its dependency keys include index choice 255 in utf8_char_width. The installed core source, str/validations.rs:254 and :279, declares the 256-byte UTF8_CHAR_WIDTH table and indexes it with an input byte. The unknown byte comes through raw_cpuid::VendorInfo::as_str. The audit alternative allowance is 128. The earlier optional request split enumerated these indexes until the step allowance stopped the proof.

Stage126 compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. Its driver SHA256 is 167673581048663C9A60C4868F9E8CE8843E07AE163C51C00233E6251BAB5FDF. Reports are target/control-summary-20261020/stage126/focused-results.json and target/contract-checks/20261023-104205-1023531/results.json. Ordinary formatting negative uses 3217304 transfers and 63 frames. It reports the expected Forbidden error and no unknown-target error in target/scenarios/20261023-104306-0461913/formatting-negative.log. Stage127 replaces the planned full validation of stage126; no full-suite or kernel result is claimed for stage126.

Stage126 stops an optional control or witness proof when a loop demand reaches the existing relation-summary threshold. The stop is AlternativeLimit, with the loop demand as its cause. It does not store an unknown value in the stopped demand. Required binding and memory-effect work outside the optional proof retain the existing loop summary rules.

```rust
if summarize && (self.controls.solving
    || self.controls.proving_assertion || self.cut_recursive_returns)
{
    self.controls.dependency_stop.get_or_insert((key.point, QueryStop::AlternativeLimit));
    self.flow.enqueue(current);
    return Err(QueryStop::AlternativeLimit);
}
```

The stage125 bounded trace is target/scenarios/memory-20261023-103217-7655210/stderr.log. It stopped after 301.010 seconds under its measurement limit, including about 183 seconds of build work. It is not a validation result. At line 52427, console::_print summarizes loop relations for counter values 0 through 8. At line 53168, the same optional proof has spent 500488 steps. Its stopped dependency chain has 43 scalar alternatives and 42 control alternatives with 1175 guard entries. The loop bound in the kernel is 256. Stage126 stops at the relation-summary boundary instead of continuing this optional enumeration after it has lost detailed iteration relations.

The stage125 trace confirms the earlier targeted improvement. The proof-only core::fmt::write queries at bb29[14] and bb13[13] now use at most 1853 and 1154 steps in the measured sample. Earlier samples reached 1000000 steps at these sites. This does not establish a whole-kernel speed ratio. The remaining leading work includes console::_print and UTF-8 validation proofs.

Stage125 compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. Its driver SHA256 is 01A62FD51E3A859C03E70934F696E9EE27341896833ED293B37E38AD448C9457. It extends the optional entry-storage boundary to proof-only cut_recursive_returns queries. A ControlMemory stop cancels that optional case, not the required case that remains queued. No stopped pointer value is stored as a completed unknown value.

Stage125 reports are target/control-summary-20261020/stage125/focused-results.json and target/contract-checks/20261023-102805-1878120/results.json. Ordinary formatting negative reports one Forbidden error and no unknown-target error. It uses 8556525 transfers and 96 frames. Its log is target/scenarios/20261023-102859-6671135/formatting-negative.log. The full non-panic audit is in progress and will run panic-format last. Kernel validation uses target/scenarios/memory-20261023-103032-6591159/stderr.log. All high-allowance settings remain unchanged. The saved source files and driver were compared with the working files and match.

Stage122 kernel validation has now finished at the 1200-second analysis limit. Total time was 1326.720 seconds, including build work. Peak checker private memory was 2695241728 bytes. The four main workers used 19638129757 transfers. The evidence has incomplete=true and diagnostics_complete=false. This supersedes the earlier in-progress note below.

The stage123 trace at target/scenarios/memory-20261023-101718-5880782/stderr.log gives the direct evidence. Line 79183 records a proof-only core::fmt::write query at bb13[13] that uses 1000000 steps and returns no accepted values. Its dependency chain reads PANIC_RUNTIME_READY through panic_common, panic_fmt, and RoundRobinDomainAlgorithm::pick_next, then expands irq_handle_create and create_irq_handle_inner. These reads occur after the control proof, during proof-only target work. Therefore controls.solving is false and the stage122 boundary does not apply. The required target pass remains unchanged in stage125.

```rust
Err(QueryStop::RefinementLimit | QueryStop::AlternativeLimit
    | QueryStop::ScratchLimit | QueryStop::ControlMemory)
    if solver.cut_recursive_returns =>
{
    pending.retain(|case| !case.cut_recursive_returns);
    break;
}
```

Stage124 compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. Its full audit and kernel run are in progress. The driver SHA256 is B309AC6467531BDF7D86F7E52D7EFEA87D2F1D07564F74F0006C4C387EB19EE6. Reports are target/control-summary-20261020/stage124/focused-results.json and target/contract-checks/20261023-102130-3031842/results.json. Ordinary formatting negative uses 8310644 transfers and 72 frames. It reports one Forbidden error and no unknown-target error. The log is target/scenarios/20261023-102234-2792995/formatting-negative.log. The kernel log is target/scenarios/memory-20261023-102402-3963940/stderr.log.

The separate stage124-scalar1k comparison uses the same binary with scalar_steps=1000. All other limits stay the same. It accepts 12 of 19 applicable focused checks and all 18 annotation checks. It rejects cut negative, unbounded-cut negative, both global-update cases, both boot-registry cases, and ordinary formatting negative. The two failed safe cases do not produce cache checks. It is not an accepted configuration. It has not been used for a kernel validation run. The high-allowance validation remains scalar_steps=1000000.

Stage121 panic-format negative has now finished. It fails with one Forbidden error and four unknown paths. Its evidence has incomplete=false and diagnostics_complete=true. The bound forbidden route passes through panic_handler and PanicInfo::fmt. The log is target/scenarios/20261023-101009-2091343/recursive-bindings-negative.log. The earlier in-progress note below is superseded by this result.

Stage124 assertion proofs use the same entry-storage boundary as other optional control proofs. The proving_assertion flag applies only while the optional assertion condition is evaluated. A possible global write stops that proof with ControlMemory. The known panic target remains with an unresolved failure guard. This does not set controls.solving or change required pointer reads outside the proof.

```rust
if self.controls.solving || self.controls.proving_assertion {
    self.controls.dependency_stop.get_or_insert((
        self.refinement_origin.unwrap_or(point), QueryStop::ControlMemory,
    ));
    return Err(QueryStop::ControlMemory);
}
```

Stage123 compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. Reports are target/control-summary-20261020/stage123/focused-results.json and target/contract-checks/20261023-101140-9664053/results.json. Its driver SHA256 is A11CF50BCB810FB3F4DDAFAE1B972B8EA9F6DC6B0955DDA6F79085EB014E9322. Its full audit and kernel validation are in progress. The kernel log is target/scenarios/memory-20261023-101616-0201978/stderr.log. The separate bounded trace is target/scenarios/memory-20261023-101718-5880782/stderr.log.

Source inspection shows that a non-phi Widened guard survives merging and feasibility checks. A proof-only cut_recursive_returns pass cannot produce an accepted result while that guard remains. Stage123 skips these proof-only cases when an inherited path has such a guard. It also omits target work when all paths from the optional control proof contain that guard. The required pass remains queued. It still checks targets, effects, and possible continuations. Statistics now count skipped_witnesses.

```rust
if inherited.iter().any(|(_, guard)|
    guard.proof == query::GuardProof::Widened && !guard.phi)
{
    pending.retain(|case| !case.cut_recursive_returns);
}
```

The stage122 bounded trace is target/scenarios/memory-20261023-100621-1481868/stderr.log. It still records proof-only core::fmt::write queries at bb29[14] and bb13[13] that spend 1000000 steps and produce no accepted values. The trace alone does not count how many of those cases have an inherited widening marker. The stage123 skip counter and validation runs must establish the effect of this change. It does not lower any limit or change required-query completion rules.

Stage122 compiler checking and release compilation pass. All 21 focused checks and all 18 annotation checks pass. Kernel validation is in progress. It extends the stage120 entry-storage boundary to all reads within an optional control proof, including pointer prerequisites. It stops the whole proof and does not insert an unknown pointer value. Required binding and memory-effect requests retain their existing evaluation. Complete storage-preservation and read-only checks still precede the stop.

The stage122 driver SHA256 is 12F8EAADF9A167A8AEF5BC547C79212F27E66034DB669D29F511AEFC836E344B. Reports are target/control-summary-20261020/stage122/focused-results.json and target/contract-checks/20261023-100306-0493172/results.json. The ordinary formatting negative log is target/scenarios/20261023-100402-1672577/formatting-negative.log. The kernel log is target/scenarios/memory-20261023-100548-5115720/stderr.log. The stage123 full audit replaces the planned stage122 full audit. The stage122 bounded trace stopped after 300.765 seconds under its measurement limit. It is not a completed kernel check.

```rust
if self.controls.solving {
    self.controls.dependency_stop.get_or_insert((
        self.refinement_origin.unwrap_or(point), QueryStop::ControlMemory,
    ));
    return Err(QueryStop::ControlMemory);
}
```

The stage121 trace at target/scenarios/memory-20261023-095426-9269530/stderr.log shows an optional pop_queued_task condition reading the global CoreScheduler pointer through Scheduler::core and Atomic::load. The scalar-only boundary does not stop this pointer prerequisite. Other optional proofs reach Backtrace::from_start and expand thousands of reads. This is not a reason to widen cached pointer values. It is a reason to stop the optional request before it expands global memory history.

Stage121 compiler checking and release compilation pass. All 21 focused checks, all 18 annotation checks, all 113 non-panic cases, and all 45 cache checks pass. Panic-format safe and cache pass; negative is still running. Kernel validation reached the 1200-second analysis limit. Total time was 1330.355 seconds, including build work. Peak checker private memory was 2762080256 bytes. The four main workers used 20285067298 transfers. Their frame counts were 53755, 39612, 70678, and 68667. No partial result was accepted. Assertion conditions now use a bounded optional proof at the top-level target request. The compiler panic target remains known if that proof stops. Its failure guard is unresolved, and the diagnostic retains the cause. A proved successful assertion excludes the panic target. A proved failed assertion retains an established failure guard. Required pointer reads, process limits, and context changes keep their existing rules.

The stage121 driver SHA256 is 855B5A7E5D5000CB38541D28A85F328B2E292D0A05BE11F21B3C3D5BD8ABB959. Reports are target/control-summary-20261020/stage121/focused-results.json and target/contract-checks/20261023-095032-9137626/results.json. Ordinary formatting negative uses 10639450 transfers and 116 frames. Its log is target/scenarios/20261023-095127-3424744/formatting-negative.log. It reports the expected forbidden target with reachability uncertainty and no unknown-target error. The kernel log is target/scenarios/memory-20261023-095248-5611688/stderr.log. The full non-panic audit is in progress. Its runner will check panic-format last if all non-panic cases and cache checks pass. A separate bounded diagnostic run uses target/scenarios/memory-20261023-095426-9269530/stderr.log.

```rust
let proof = if matches!(condition.value, Value::Bits(_)) {
    GuardProof::Established
} else {
    GuardProof::Unresolved
};
```

The stage120 trace is target/scenarios/memory-20261023-094215-0789793/stderr.log. At line 34730, a required assertion condition read in Scheduler::num_cores uses 4972522 steps before the main query stops. The compiler already supplies the panic function. Before stage121, an unknown assertion condition also produced that function without an unresolved failure guard. This could incorrectly mark the assertion failure as proved. The change separates target identity from assertion reachability. The trace stopped at 300.644 seconds under its measurement limit and is not a completed kernel check.

Stage120 compiler checking and release compilation pass. All 21 focused checks, all 18 annotation checks, all 113 non-panic cases, and all 45 cache checks pass. The standard audit took 640.958 seconds. The extra audit took 179.383 seconds. Its kernel run reached the 1200-second analysis limit and took 1324.304 seconds including build work. Peak checker private memory was 2346528768 bytes. The four main workers used 17528833449 transfers. The evidence has incomplete=true and diagnostics_complete=false. Stage120 moves the stage119 optional global-scalar boundary before caller traversal at a function entry. The same complete storage-preservation and read-only checks still determine whether a discovery root can write the global. Local definitions and required binding reads retain their existing rules.

The stage120 driver SHA256 is 959331E3EC0E00992BF3739CB8CB579B73EB19E7ACF4548FF88BD4EC00BD1012. Reports are target/control-summary-20261020/stage120/focused-results.json and target/contract-checks/20261023-093928-5197465/results.json. The ordinary formatting negative log is target/scenarios/20261023-094012-8840965/formatting-negative.log. The kernel log is target/scenarios/memory-20261023-094127-6709186/stderr.log. All limits remain enabled at the same settings as stage119. No kernel source change was made during this investigation.

The stage119 trace at target/scenarios/memory-20261023-093356-5253057/stderr.log shows why the boundary must occur earlier. At line 90695, a PANIC_RUNTIME_READY proof uses 499996 steps, 905 required reads, and a 65-read dependency chain. It reaches scheduler selection and queue state through caller conditions before it reaches the IRQ entry. The stage119 boundary stops other global proofs early, but it cannot stop this caller traversal before the read reaches the root.

Stage119 compiler checking and release compilation pass. All 21 focused checks, all 18 annotation checks, all 113 non-panic cases, and all 45 cache checks pass. The standard audit took 569.896 seconds. The extra audit took 173.638 seconds. Its kernel run reached the 1200-second analysis limit and took 1317.050 seconds including build work. Peak checker private memory was 2019270656 bytes. The four main workers used 9034906360 transfers. The evidence has incomplete=true and diagnostics_complete=false. Its separate bounded trace stopped at 301.247 seconds and is not a validation result. Stage119 stops an optional scalar control proof when it reaches IRQ-entry storage and a discovery root can change that storage. It does not expand that root's initialization history for this optional proof. Required binding and memory-effect work retain their existing rules. The stop remains separate from stored values. It does not mark loop returns as unknown merely because entry storage is unresolved.

The stage119 driver SHA256 is A7777568802AC1308A7AF57473B42FAB152557577C0FE91C7B7F10BFDA65386A. Reports are target/control-summary-20261020/stage119/focused-results.json and target/contract-checks/20261023-092815-6197745/results.json. Ordinary formatting negative uses 10659900 transfers and 94 frames. It reports one Forbidden error with reachability uncertainty. The log is target/scenarios/20261023-092856-2289798/formatting-negative.log. The kernel log is target/scenarios/memory-20261023-093020-4594358/stderr.log. It uses a separate build folder, target/stage119-kernel-cargo, so the stage118 kernel run can finish without interference.

```rust
if control_scalar {
    self.controls.dependency_stop.get_or_insert((
        self.refinement_origin.unwrap_or(point), QueryStop::ControlMemory,
    ));
    return Err(QueryStop::ControlMemory);
}
```

This boundary follows the existing complete storage-preservation and read-only checks. It does not assert that a possible write occurs. The diagnostic states that a possible write prevents a control-flow proof. The stage118 bounded trace is target/scenarios/memory-20261023-091949-9032607/stderr.log. Its measurement runner stopped it at 300.775 seconds. It is diagnostic evidence, not a completed kernel check.

Stage118 compiler checking and release compilation pass. All 21 focused checks, all 18 annotation checks, all 113 non-panic cases, and all 45 cache checks pass. The standard audit took 505.521 seconds. The extra audit took 155.246 seconds. Its kernel run reached the 1200-second analysis limit and took 1307.286 seconds including build work. Peak checker private memory was 2015948800 bytes. The four main workers used 12573249701 transfers. The evidence has incomplete=true and diagnostics_complete=false. The result is not a complete kernel check.

Stage118 retains stopped optional condition proofs with their frame, branch, generation, input choices, loop choices, scalar witnesses, and recursive-return mode. A matching optional condition can remain unresolved without repeating the stopped proof. Required value reads do not use this fallback. The cause remains available for diagnostics. No new test or source comment was added.

The driver SHA256 is 77817FB5715A400E81B2A6FB1172EA176C175C3F0371D2BD68B6084BFED3E64D. The reports are target/control-summary-20261020/stage118/focused-results.json and target/contract-checks/20261023-091033-8167970/results.json. Ordinary formatting negative uses 10631117 transfers and 95 frames. It reports one Forbidden error with reachability uncertainty and records 73834 stopped-condition reuses. Its log is target/scenarios/20261023-091050-9366856/formatting-negative.log. The kernel run uses the unchanged stage117 limits and writes to target/scenarios/memory-20261023-091201-4722264/stderr.log.

Stage117 passed all 21 focused checks, all 18 annotation checks, all 113 non-panic cases, and all 45 cache checks. Its kernel run reached the 1200-second analysis timeout. The log is target/scenarios/memory-20261023-082859-4059277/stderr.log. Peak checker private memory was 2018656256 bytes. The four main workers used 7761662424 transfers and recorded two precise control-cache hits in 103128 evaluations. No partial result was accepted.

The stage117 diagnostic samples are target/scenarios/memory-20261023-085359-5011365/stderr.log and target/scenarios/memory-20261023-085817-8710378/stderr.log. Both were stopped by the measurement runner after 240 seconds. They are not validation runs. The first sample records 1234 stopped control refinements at 29 panic_common sites. The second shows that a PANIC_RUNTIME_READY read creates 704 dependencies. One attempt uses 499586 steps across 3299 required reads. The same condition in frame1237 repeats under epochs 66, 68, 70, 72, 74, and 76. New call-site queries clear loop fallback results, and precise control answers include the epoch in their key.

The stage118 change uses this rule only during optional control solving:

```rust
if let Some(key) = &condition_key
    && let Some(&cause) = self.controls.stopped_conditions.get(key)
{
    self.controls.causes.insert(key.point, cause);
    for entry in &mut value.guards {
        if *entry == (frame, guard) { entry.1.proof = GuardProof::Unresolved; }
    }
}
```

The user requires a complete kernel run. Real forbidden calls must remain for the user to audit. Kernel annotations are permitted for genuine coverage gaps, not as a substitute for correcting checker defects. Keep the existing Forbidden diagnostic rule and all resource limits. First run the existing focused and annotation cases, then the kernel and the full scenario audit. Keep panic-format separate and check it last. A timeout is not completion.

Stage116 completed the preceding full non-panic audit with 112 of 113 cases and all 45 cache checks. All 18 annotation checks passed. Its sole rejected case was ordinary formatting negative. Callback binding completed, but the driver incorrectly classified the established forbidden target as Unknown when its path was unresolved. The historical diagnostic rule below describes that version and is superseded by the user's correction.

The investigation is not complete. The changes remove unnecessary memory dependencies and check call paths before return-value and memory-effect work. They do not provide a compact numeric loop relation. Do not describe stage102 as an all-pass fix, or apply stage97's full-audit count to stage102. The later sections record the experiments in time order; earlier retained-version statements describe their state at that time.

## Diagnostic correction

Stage117 changes the driver, not the solver's pointer or control facts. A known forbidden target uses the forbidden error even when path_proven is false. In that case, the driver retains source uncertainty notes and adds this note:

```text
reachability is uncertain: analysis could not determine whether execution reaches this forbidden call
```

It does not emit the note that a feasible path is established unless path_proven is true. Backend candidates remain excluded from forbidden reports. Existing trusted and allow_incomplete permissions still apply before the driver emits a diagnostic.

The existing scenario runner now accepts a bound route with unresolved reachability only if the forbidden error and uncertainty note are present. It still rejects unknown-target errors, missing bindings, crashes, and incomplete analysis. It does not change path_proven in evidence. The existing unmarked,uncertain annotation case must report two unknown assembly operations and one forbidden error with an uncertainty note. No new test case was added.

The focused report is target/control-summary-20261020/stage117/focused-results.json. The annotation report is target/contract-checks/20261021-083107-6786912/results.json. Ordinary formatting negative passes in target/scenarios/20261021-083136-7973117/formatting-negative.log. It reports one Forbidden error, with the reachability uncertainty note and source causes. It uses 11624179 transfers and 100 frames. The driver SHA256 is C4F739424F547E0C06302593040CD250E7982B75C25EA0347FC196F0DB6EF08D. Source and executable hashes were checked again after resuming on 2026-10-23. They match the saved stage117 files. The full standard and extra audits use the same limits as stage116. No limit was disabled.

The full audit completed on 2026-10-23. Standard passes 94 of 94 cases and 45 of 45 cache checks. Extra passes 19 of 19 cases. The reports are target/control-summary-20261020/stage117/standard-results.json and target/control-summary-20261020/stage117/extra-results.json. They take 744.553 and 239.220 seconds respectively, including build work. The full-audit formatting negative log is target/scenarios/20261023-081254-0570210/formatting-negative.log. It reports one Forbidden error with reachability uncertainty and uses 11623356 transfers and 100 frames. There are no unknown-target errors in that run. These results do not include the two panic-format cases or the actual kernel.

## Function-level fallback investigation

Stage115 changes the conservative fallback key from a call frame to the function instance and IRQ mode. Precise values and call inputs remain separate for each frame. The fallback does not assert a return value or unchanged storage. It provides an unknown result, and uses the existing complete write information to preserve disjoint storage.

The stage114 trace at target/scenarios/20261021-073118-9727153/formatting-negative.log shows the remaining dependency. The callback selector in core::fmt::write reads its previous callback Result through the loop backedge. This reaches Argument::fmt, LowerHex::fmt, pad_integral, write_prefix, and a new Buffer::write_str frame. The new frame starts precise return work although an earlier frame of the same function has already used the fallback. At line 6002, the request selects 22 loop frames for fallback only after it has spent all 9000000 steps. A frame-specific fallback does not bound this repeated work across call frames.

The candidate uses the same conservative result for a new frame of that function:

```rust
let key = (instance, self.frames[point.frame].irq);
let Some(&cause) = self.controls.loop_fallbacks.get(&key)
    else { return Ok(None); };
```

Stage115 compiler checking and release compilation pass. The focused result is 20 of 21 checks. All 18 annotation checks pass. Ordinary formatting negative still fails. Its log is target/scenarios/20261021-080844-3388343/formatting-negative.log. It uses 25824032 transfers and 281 frames. The earlier stage114 focused run used 34153232 transfers and 374 frames. These are separate runs, not a guarantee of a fixed speed change.

The stage115 first failed binding has a 113-read dependency chain. It reaches pad_integral through the previous callback Result, DebugStruct::finish, DebugStruct::field, and the u16 formatting method. pad_integral has no local loop. Its calls perform the iteration. Thus selecting only functions with local loops leaves this return calculation precise after the writer fallback.

Stage116 also records scalar return calculations from functions in the complete IRQ-safe call closure. If the request stops, those return calculations can use the same conservative fallback. This does not mark their writes unchanged and does not change the separate call-coverage check. Compiler checking and release compilation pass. No tests, scenario expectations, or source comments are added.

Stage116 passes 20 of 21 focused checks and all 18 annotation checks. The annotation results are target/contract-checks/20261021-081250-7855959/results.json. The ordinary negative log is target/scenarios/20261021-081320-9248601/formatting-negative.log. It uses 11660705 transfers and 100 frames. All final callback queries finish. The only reported error is the established forbidden_format target with an unresolved IRQ path. The diagnostic includes the first panic_message Result check, the writer loop limit, and lost formatting-loop relations. It does not report an unfinished callback binding. Local limits still stop precise attempts; fallback then completes the required target work. This is not a claim that no limit is reached.

The full standard and extra audits are complete with trace output disabled. Standard passes 93 of 94 cases and 45 of 45 cache checks. Extra passes 19 of 19 cases. The reports are target/control-summary-20261020/stage116/standard-results.json and target/control-summary-20261020/stage116/extra-results.json. They take 500.750 and 153.216 seconds respectively, including build work. Their settings remain query 1000000, scalar 1000000, alternatives 128, memory 10240 MiB, timeout 1200 seconds, call depth 256, function scratch 16 MiB, and query scratch 256 MiB. The existing transfer retry keeps spent work and extends the main allowance to 9000000. The audit excludes panic-format and the actual kernel. Do not change Unknown to Forbidden without a proved path, or change the scenario expectation to hide the remaining precision gap.

The stage116 driver SHA256 is 32CDD0E3E54EEB6B66BDBEC67583F3E2E6C55B43E457CDEAAA8367DB3BC9F9D4. All nine saved source files match the working source. The full-audit ordinary negative log is target/scenarios/20261021-081756-0729109/formatting-negative.log. With tracing disabled, it uses 11622022 transfers and 100 frames. It reports the same one established forbidden target with unresolved reachability. There is no unfinished callback-binding diagnostic.

## Original work

The later continuation rule replaces the earlier unresolved-callee cutoff. An unresolved condition must not, by itself, stop inspection of a callee. Inspect all possible continuations. Keep unknown return values and memory effects conservative. Report uncertainty if it prevents a safety decision, not merely because the loop result is unknown. A forbidden target on an unresolved IRQ path remains an Unknown error. A proved forbidden path remains Forbidden. Do not change scenario expectations to hide this distinction.

Stage103 removes the cutoff in analysis.rs and reserves part of the main query allowance for target work. Diagnostics retain source locations for unresolved conditions and failed dependencies. The driver SHA256 is AD2FE3BB5C1E5598AF8C9D25A6A6CF41D398E1A539211E6020D424361D8C3DD3. The ordinary negative log is target/scenarios/20261021-054553-1275690/formatting-negative.log. It confirms inspection of the second panic_message and then stops at formatter callback binding. It uses 12356502 transfers and 37 frames. It does not establish forbidden_format. The annotation results are target/contract-checks/20261021-054432-1192516/results.json.

The trace shows that required callback reads request failed control conditions again. One dependency chain reads the earlier formatting Result and Buffer.used. Stage104 records failed optional condition proofs for the rest of the query. It retains their branch constraints, but does not repeat those proofs. Read and control cache keys include the fallback epoch. Unfinished reads are removed before target work continues. Required pointer reads and memory effects do not become scalar unknown values.

```rust
if self.controls.coarse_conditions.contains(&condition) {
    for entry in &mut value.guards {
        if *entry == (frame, guard) { entry.1.proof = GuardProof::Unresolved; }
    }
    pending.push_back((value, checked, relations));
    continue;
}
```

Validate the existing focused and annotation cases at the same limits. Inspect any remaining ordinary-formatting error before a full audit. No new panic-format or kernel result is established by stage103.

Stage104 still fails ordinary formatting negative. Its log is target/scenarios/20261021-055834-9191607/formatting-negative.log. It uses 29601292 transfers and 99 frames, and stops at formatter callback queries with alternative and transfer limits. It does not establish forbidden_format. The broad condition fallback is removed in stage105.

Stage105 retains the continuation and diagnostic changes. After optional control refinement stops, it uses a conservative summary for non-pointer return data from loop-containing functions reached by that refinement. It does not change their memory effects or pointer-return data. The summary carries the cause of the stop. Its cache epoch differs from the precise stage. A new query clears these summaries and invalidates their cached reads. The driver SHA256 is 81D3D0DA50359AF8A5DB4640783B839A0583C3E10F737884E3529A963FFAD0C7. Compiler checking and release compilation pass. Focused validation is in progress.

Stage105 completes focused validation with 20 of 21 checks and all 18 annotation checks passing. Ordinary formatting negative still fails at callback binding. Its log is target/scenarios/20261021-060853-5118418/formatting-negative.log. It uses 17956159 transfers and 135 frames. The callback dependency still reaches Buffer.used through call_return and exit_values. The latter requires the callee return paths even when the return data has a conservative summary. Stage106 applies that summary at the call boundary, before the callee exit-path proof. Memory effects remain separate and are not assumed unchanged.

Keep RUPTA, the existing diagnostic rules, and all resource limits. Do not add tests or source comments. Use existing scenarios. Keep the 1200-second time limit and the 10 GiB memory limit.

Stage106 still fails ordinary formatting negative. Its log is target/scenarios/20261021-061720-9450944/formatting-negative.log. It uses 20996428 transfers and 172 frames. The scalar return summary does not prevent repeated memory-effect work. Stage107 also makes caller entry-path checks conservative when the outer path is unresolved. This change increases work to 122303533 transfers and 3373 frames and still fails. Its completed log is target/scenarios/20261021-063037-7312102/formatting-negative.log. The stage107 entry-path change is removed.

Stage108 retains precise caller entry checks. After a loop calculation stops, a call-boundary effect summary returns unknown for storage that existing complete models cannot prove unchanged. It does not return unchanged storage for an unknown write. Ordinary formatting negative still fails, with 24238833 transfers and 172 frames. Its log is target/scenarios/20261021-063936-6575271/formatting-negative.log. Required target work reaches further loop functions after optional control refinement ends. Those new loops do not receive the fallback summary in stage108.

Stage109 records loop functions reached by required target work as well. If that request stops at a local resource limit, it can retry with conservative summaries for those loop functions. The retry retains spent steps, changes the cache epoch, and removes invalid reads. It does not widen pointer return data or assume unknown writes are unchanged. It stops if no new summary is available or the main allowance is exhausted.

```rust
if changed {
    self.controls.epoch += 1;
    self.retain_completed();
}
if self.budget.steps >= self.budget.limit {
    return ReadOutcome::Stopped(QueryStop::TransferLimit);
}
```

Stage109 compiler checking and release compilation pass. Focused and annotation checks are in progress. No full-audit, panic-format, or kernel result is established by this candidate.

Stage109 completes all 18 annotation checks. Its ordinary negative trace confirms a binding fallback after 4725073 steps, with 12 loop functions selected for conservative summaries. A further fallback occurs at the 9000000-step main limit, too late to retry that callback query. The full negative run is still in progress. The log is target/scenarios/20261021-065008-1199018/formatting-negative.log.

Stage110 gives each precise binding attempt a local allowance before fallback. This allowance does not increase the total query allowance. If no new loop summary is available, required work can continue under the remaining main allowance. Nested requests inherit the current stage allowance. The request restores the previous allowance on success and on failure.

```rust
self.budget.refinement_limit = Some(
    self.budget.steps.saturating_add(stage_allowance).min(self.budget.limit),
);
let answer = operation(self);
self.budget.refinement_limit = saved_refinement_limit;
```

Stage110 compiler checking and release compilation pass. All 18 annotation checks pass in target/contract-checks/20261021-065628-2348371/results.json. No focused scenario run is made with this version.

The stage109 ordinary negative process is stopped after two callback queries reach the 9000000-step main limit. The other 20 focused checks complete and pass. The runner records the interrupted negative case as rejected. Its partial log is evidence of the two query stops, not a completed analysis result. No files are deleted.

The stage109 trace also shows control reads inside loop functions after the conservative call summary is active. These reads still call control_step with precise refinement enabled. Stage111 applies the same fallback stage to these internal control reads and to memory-path checks. It retains selected branch constraints, marks unresolved conditions, and records the original stop location. It does not change pointer values. Explicit stopped blocks remain excluded by the structural control check.

```rust
if let Some(&cause) = self.controls.loop_fallbacks.get(&point.frame) {
    self.controls.causes.insert(point, cause);
    for value in &mut coarse {
        for (_, guard) in &mut value.guards {
            if guard.proof == GuardProof::Pending && !guard.input && !guard.phi
                && guard.index.is_none() && guard.choice.is_none()
            {
                guard.proof = GuardProof::Unresolved;
            }
        }
        if !value.guards.contains(&marker) { value.guards.push(marker); }
    }
    return self.bounded(coarse);
}
```

Stage111 compiler checking and release compilation pass. All 18 annotation checks and 20 of 21 focused checks pass. Its driver SHA256 is 2574D66D69ABABDFA899DB124BE92FCF4B2BB258D79B6D7FFDB3D4526D58DD56. The annotation results are target/contract-checks/20261021-070045-7666612/results.json. Ordinary formatting negative still fails. Its log is target/scenarios/20261021-070214-3791777/formatting-negative.log, with 12502088 transfers and 43 frames. The remaining stop is AlternativeLimit on the byte-code cursor in core::fmt::write. The trace shows 104 alternatives on local _11 at bb77[0] before that read stops. No forbidden_format binding is established.

Stage112 restricts the internal-control fallback to functions in the complete IRQ-safe call closure. The formatting loop that selects callbacks must retain its precise branch checks. Safe loop functions can still use the conservative control result. Return values and possible writes remain separate from this safety classification. This change does not assume an IRQ-safe function preserves memory.

```rust
if self.facts.safe.contains(&frame.instance)
    && let Some(&cause) = self.controls.loop_fallbacks.get(&point.frame)
{
    self.controls.causes.insert(point, cause);
    for value in &mut coarse {
        for (_, guard) in &mut value.guards {
            if guard.proof == GuardProof::Pending && !guard.input && !guard.phi
                && guard.index.is_none() && guard.choice.is_none()
            {
                guard.proof = GuardProof::Unresolved;
            }
        }
        if !value.guards.contains(&marker) { value.guards.push(marker); }
    }
    return self.bounded(coarse);
}
```

Stage112 compiler checking and release compilation pass. All 18 annotation checks and 20 of 21 focused checks pass. Its driver SHA256 is 3066B4994C472B9C1BF57620D2CF4DEC9EEC86C83BBFC146002E3E48699344A2. The annotation results are target/contract-checks/20261021-070803-6540609/results.json. Ordinary formatting negative still fails, with 34132455 transfers and 292 frames. The log is target/scenarios/20261021-070930-4883975/formatting-negative.log. It reports ScratchLimit and TransferLimit at formatter binding. The storage trace identifies 16 MiB per-function stops in pad_integral, Buffer::write_str, PadAdapter::write_str, and core::fmt::write. It does not establish forbidden_format.

Stage113 moves existing call-boundary loop summaries ahead of caller-path refinement when the callee is already known. The earlier order requires the caller path before the conservative result can be used. The new shared method has two callers: call_return and call_effect. It accepts a direct MIR target or an existing singleton binding. It does not accept an arbitrary backend candidate. It preserves storage when the complete write model proves preservation. Otherwise its effect remains unknown. Pointer return data, unwind effects, explicit return exclusions, and the call-depth limit retain their existing rules.

```rust
if let Some(values) = self.loop_call_summary(point, requested, None)? {
    return Ok(values);
}
if self.flow.current.is_some() && self.paths(point, true)?.is_empty() {
    return Ok(Vec::new());
}
```

Stage113 compiler checking and release compilation pass for the final source, including its call-depth guard. All nine saved source files match the working files. Its driver SHA256 is A0589E166799CECD59D4A1F59165E6E630A05BA02A0381BACE807071A716EB17. All 18 annotation checks pass in target/contract-checks/20261021-071938-6783689/results.json. The six recursive focused configurations pass safe, negative, and cache checks. Ordinary formatting validation is in progress. No full-audit, panic-format, or kernel result is established by this candidate.

Stage113 completes focused validation with 20 of 21 checks passing. Ordinary formatting negative still reports ScratchLimit and TransferLimit, with 34130895 transfers and 383 frames. Its log is target/scenarios/20261021-072103-3148485/formatting-negative.log. The early summary lookup creates frames even when it cannot use a summary. Stage114 changes that lookup to a read-only frame-key lookup.

Stage114 also permits an unknown return or effect summary for a completely covered indirect call when a loop summary applies to a candidate. This summary contains no Function value and establishes no call binding. It does not report Forbidden from a candidate. The graph must still check the call separately. A candidate set alone does not prove unchanged storage; the existing complete write-coverage check remains necessary for that result.

```rust
let Some(targets) = self.facts.safe_candidates.get(&site)
    .filter(|targets| !targets.is_empty() && self.facts.covered_calls.contains(&site))
else { return Ok(None); };
let Some((_, &cause)) = self.controls.loop_fallbacks.iter()
    .filter(|(frame, _)| targets.contains(&self.frames[**frame].instance))
    .min_by_key(|(frame, _)| **frame)
else { return Ok(None); };
```

Stage114 compiler checking and release compilation pass. Its driver SHA256 is DFFF6027F2A6C9E3E90DBABE6B66BAAD241CD7800088931A26E6A95C0F5871F1. The focused result is 20 of 21 checks. All 18 annotation checks pass in target/contract-checks/20261021-073003-4655923/results.json. Ordinary formatting negative still fails with ScratchLimit and TransferLimit. Its log is target/scenarios/20261021-073118-9727153/formatting-negative.log, with 34153232 transfers and 374 frames. It reaches the second message but does not establish forbidden_format. This is not an all-pass result or a complete repair.

The stage114 extra and standard non-panic audits run in that order. They use the same limits as focused validation, with trace flags disabled: query 1000000, scalar 1000000, alternatives 128, memory 10240 MiB, timeout 1200 seconds, call depth 256, function scratch 16 MiB, and query scratch 256 MiB. The existing one-time transfer retry can extend a query to 9000000 steps. These settings do not change the defaults. Do not apply an earlier full-audit result to stage114.

The stage114 extra group completes with 19 of 19 cases passing in 323.812 seconds, including build work. Its report is target/control-summary-20261020/stage114/extra-results.json. The standard group completes with 93 of 94 cases and all 45 cache checks passing in 657.732 seconds, including build work. Its report is target/control-summary-20261020/stage114/standard-results.json. The combined non-panic count is 112 of 113. Cache and annotation checks are separate counts.

The full-audit ordinary negative log is target/scenarios/20261021-074508-4389226/formatting-negative.log. It reports two unknown-call errors at core::fmt::rt::Argument::fmt: ScratchLimit and TransferLimit. Its counters are 34130073 transfers, 374 frames, 17 queries, and 177 sites. These counters are separate from the focused run. Both runs reach the second message. Neither establishes forbidden_format. The new diagnostics identify source conditions and stopped dependencies, including the earlier question-mark expression, the Buffer byte loop, and formatting code.

The remaining failure is not a demonstrated mismatch between an established forbidden call on an unresolved path and the scenario expectation. The callback binding itself is unfinished. Required binding work still reaches loop control and memory-effect dependencies. No result proves that this remaining limit is unavoidable. Do not report the repair as complete, change the expected result to accept this error, or promote a backend candidate to an established forbidden call.

The source changes in this investigation are in src/analysis.rs, src/driver.rs, src/analysis/sparse.rs, src/analysis/sparse/control.rs, and src/analysis/sparse/flow.rs. The saved stage114 snapshot matches all nine recorded source files. The implementation preserves RUPTA, the refinement-owner cache key, the Unknown/Forbidden policy, and all resource limits. No whole-project format command was run.

1. Stop requiring a callee return proof to follow its normal MIR continuation. Keep branch conditions, return values, memory effects, and explicit unreachable exclusions separate. Retain bounded callee expansion when the entry path is unresolved. Such a stop must report Unknown; it must not mark the callee safe or remove the caller's successor.

```rust
if let Some(successor) = normal_successor {
    propagate_control(successor);
}
```

2. Add bounded loop-header states to the memory worklist. Join incoming values. Preserve stable values and memory proved unchanged. When changing loop states exceed the bounded precise domain, widen the requested value to unknown. Do not turn an unknown write into unchanged memory or a stopped request into a completed answer. Do not retain an unbounded sequence of iteration guards.

```rust
let next = join(entry, backedge);
let next = if growing && precision_exhausted {
    widen(previous, next)
} else {
    next
};
if next != previous {
    enqueue_users();
}
```

3. Validate normal continuation, explicit exclusions, callback updates, loops, and annotation evidence with the existing focused and contract checks. Diagnose regressions before accepting the change. Keep any unsupported relation explicit.

4. Run the standard and extra audits. Run panic-format last. Record source and executable identity, limits, results, and remaining gaps. Run kernel checks only after the scenario and annotation results permit them.

This is an implementation sequence, not a claim that all loop relations can be proved. Bounded widening must preserve possible effects and may reduce precision.

## Execution record

Stage75 removes ordinary return-proof dependencies and the unresolved-control callee shortcut. It exposes two false forbidden reports in the existing cut scenarios. A valid unreachable exclusion must still propagate through calls. This candidate is not accepted. Ordinary formatting also fails, with 23482162 transfers.

Stage76 propagates explicit exclusion dependencies through the call graph. It adds bounded memory loop-header updates and restores accepted-assembly evidence. The six recursive focused configurations pass safe, negative, and cache checks. All 18 annotation checks pass. Ordinary formatting still fails with one scratch-limit unknown diagnostic, 31736200 transfers, and 521 frames. The focused result is 20 of 21. Its trace confirms loop widening, but dependent reads still retain many old guarded alternatives. The full non-panic audit passes 112 of 113 cases and all 45 cache checks. Its driver hash is 45416054A45051777C4C7C86629E3A02FD92F1FB22071A08EC34722DD20BFAB0.

Stage77 also bounds loop relations and removes alternatives covered by an unknown value under the same conditions. It preserves established function alternatives. Only explicit unreachable marks require interprocedural exclusion checks; compiler assumptions do not require a separate termination proof. A safe-call classification does not override an explicit exclusion. The six recursive focused configurations and all 18 annotation checks pass. The full non-panic audit passes 112 of 113 cases and all 45 cache checks. Ordinary formatting remains the failure. Its driver hash is 2D03481FC710A7BB62BA77C285F2F0B0355ACF0AC8B7FE6520397135807291EE.

Stages78 and 79 bound the joined loop state, not only the latest incoming values. They incorrectly use the existing Widened guard state for a completed unknown condition. Code review shows that Widened bypasses branch-contradiction checks. An unknown condition must retain its selected branch. These candidates are not the retained implementation.

Stage80 adds GuardProof::Unresolved. It retains branch constraints but does not repeatedly evaluate a completed unknown condition. GuardProof::Widened remains the state for a discarded relation. Pending dependencies and resource stops are not changed into completed values. An unconstrained completed unknown value releases its refinement-only dependencies. All 18 annotation checks and 20 of 21 focused checks pass. Ordinary formatting fails with two transfer stops and one scratch stop, 25888848 transfers, and 578 frames. This is not a passing formatting result. Its driver hash is C665244CB0F9F95A48D3062EBED10BA79BF99CE4C8E36E62C6B9544BBE5AA699.

The separate stage80 trace is target/scenarios/20261021-023610-0795361/formatting-negative.log. It confirms loop-header widening. One required query stops after 7999993 transfers with 10304 required demands. Its root reads the first formatting result used by the question-mark operator in panic_common. Another required query reaches the 256 MiB query-scratch check while combining Formatter::write_str return conditions. The reported scratch size is 270234560 bytes. This is not the earlier 16 MiB per-function stop. Return conditions still carry many alternatives after loop-header widening. Assuming normal continuation cannot establish that a Result value is Ok.

Stage81 also preserves previously established function alternatives when a loop value widens. Compiler checking and release compilation pass. The driver hash is A252F9B656998718FC4485A1BD2750C44D7BC9326CE44BBAFBB6FCA9E703E6D4. Focused checks, annotation checks, and the full non-panic audit are running. Panic-format follows the full audit. No new kernel run is accepted or claimed.

Stage76 panic-format negative reaches the 1200-second analysis limit. It uses 799484304 transfers in the main query group. The log is target/scenarios/20261021-022057-2665759/recursive-bindings-negative.log. This fails relative to the recorded stage65 pass. The unchanged pre-edit stage74 needs a matched control run before the failure can be attributed to this loop change. The later stage77 and stage79 panic-negative processes are stopped at 2026-10-21 02:43 UTC. Their partial logs remain available but do not establish an analysis result. Their completed non-panic audits remain valid.

The next comparison restores the prior bounded callee-expansion rule while retaining all loop changes. This separates unknown termination from unresolved callee-entry paths. It does not restore the old requirement to prove normal return. Compiler checking passes; release compilation and validation are pending. The removal of this expansion bound is not accepted as part of the loop correction.

That comparison is stage82. Compiler checking and release compilation pass. All 18 annotation checks pass. The focused result is 20 of 21: the six recursive configurations and their cache checks pass; ordinary formatting safe and cache pass; ordinary formatting negative fails. The ordinary negative log is target/scenarios/20261021-024458-4895593/formatting-negative.log. It reports one pointer transfer stop and two bounded-control scratch stops, with 15346334 transfers and 578 frames. Its driver hash is DE687C1C7ADDA98860D377C67699322A240C3DA6E313467924DF7F0311544E06. All eight analysis source files match its frozen snapshot. The full audit is running. These results do not establish a general performance improvement or a passing panic-format result.

Stage80 and stage81 both complete the non-panic audit with 112 of 113 scenarios and all 45 cache checks passing. Their later panic-negative comparisons are stopped at 02:54 UTC and 02:58 UTC, respectively. Those stopped runs are not accepted analysis results. Stage82 completes the standard group with 93 of 94 cases and all 45 cache checks passing in 766.93 seconds, including build work. Its extra group is running. A separate unchanged stage74 panic control starts in target/scenarios/20261021-025643-1143647/. It uses the same limits as stage82.

Stage82 completes the extra group with 19 of 19 cases passing in 162.42 seconds. The combined non-panic result is 112 of 113, with all 45 cache checks passing. The full-audit formatting negative log is target/scenarios/20261021-024734-9629781/formatting-negative.log. It records 15346141 transfers and 578 frames. Keep these counters separate from the focused run. Stage82 panic-format safe and cache pass in target/scenarios/20261021-025940-6583395/. Its negative case and the unchanged stage74 negative control remain in progress.

## Final validation

Stage82 is the retained implementation. The release driver hash is DE687C1C7ADDA98860D377C67699322A240C3DA6E313467924DF7F0311544E06. Compiler checking and release compilation pass. The non-panic result is 112 of 113 scenarios and 45 of 45 cache checks. All 18 annotation checks pass. Panic-format safe, negative, and cache checks also pass. No tests, source comments, or scenario changes were added for this loop correction. No whole-project format command was run. No new kernel check or kernel edit was made.

The stage82 panic negative log is target/scenarios/20261021-025940-6583395/recursive-bindings-negative.log. It reports one Forbidden diagnostic and no Unknown diagnostic. The established route includes panic_bounds_check, panic_handler, and PanicInfo::fmt. Main queries use 208847280 transfers and 704 frames. Two separate root-input attempts each use 1000000 transfers. The checker stops after the established forbidden route. The evidence has incomplete=false and diagnostics_complete=false; this is an accepted negative witness, not complete coverage of every later route.

The unchanged stage74 control also passes safe, negative, and cache checks in target/scenarios/20261021-025643-1143647/. Its driver hash is 678D6F3FB506718407FD09081267D6A4FDA28040B31EABC0433F89FEA846AD0F. Main queries use 198599738 transfers and 698 frames, with two separate 1000000-transfer root-input attempts. Thus the retained loop correction preserves the panic result. These counts do not show a general reduction in analysis cost. The earlier stage76 timeout is not retained.

Ordinary formatting negative remains the only failed scenario. Normal continuation no longer requires a general callee-termination proof. Bounded loop states and completed unknown conditions no longer require unlimited precise iteration. However, formatting return-value relations still reach transfer and scratch limits. This implementation does not derive an exact iterator-copy or formatting-length relation. It does not complete the broader memory-summary migration. Do not replace the unresolved first-write Result with Ok, change the expected diagnostic, or add an annotation to hide this failure.

These candidates do not add a closed-form iterator-copy summary. The parameter-effect builder still rejects general loop bodies. Loop-header widening provides a bounded conservative fallback, not a proof of the exact number of written bytes. Do not describe this work as a complete replacement of all detailed memory analysis.

## Formatting limit investigation

This investigation starts from stage82 at the same limits. Stage83 omits caller-path refinement for fixed local addresses. It still fails ordinary formatting negative, with 16343742 transfers and 593 frames. Its focused result is 20 of 21. This address change is removed in stage86. It is not an established fix.

Stage84 adds a scratch lifetime around call-return calculations. Temporary reservations must end when the calculation returns; only the returned alternatives and retained flow data remain charged. Its log at target/scenarios/20261021-033654-4230611/formatting-negative.log records one completed call with charged=122987376, outer=63296160, flow-growth=0, and result=3063808. The old accounting retained the excess charge until the outer worklist step ended. The correction does not change transfer limits or values. Formatting still fails with 16511359 transfers and 593 frames. Its focused result is 20 of 21.

Stage85 separates value growth from condition growth at loop headers. The old rule replaced a value with Unknown when its condition count exceeded the alternatives allowance, even if the value did not change. The candidate keeps each distinct value with the conditions common to its alternatives. It marks discarded conditions as Widened. It counts changes to values separately and retains established function alternatives. Its focused result is 20 of 21 and all 18 annotation checks pass. Formatting still fails with 15236987 transfers and 981 frames, in target/scenarios/20261021-034355-3945080/formatting-negative.log. This is not a complete formatting fix.

Stage86 removes the stage83 address experiment. Stage87 also corrects redundant-state removal: a Widened marker records lost precision, not a branch constraint. It must not prevent a broader alternative from covering an older alternative. Established function alternatives remain protected. These candidates require validation. Do not apply the stage82 audit result to them.

Stage87 and stage88 introduce a local-callback regression. Equivalent alternatives replace each other when their lost-precision markers differ. The control template then reports repeated changes without a new constraint. Stage89 retains the earlier representative when both alternatives cover each other. The local-callback case passes again. Stage89 passes 112 of 113 non-panic cases, all 45 cache checks, and all 18 annotation checks. Ordinary formatting negative still fails. Its driver hash is 71A8AA0DF40E6B022262ABCEC8982F469FBF1E1E61498C822D9BC534722D3EBB. Panic-format has not been checked for this version.

The stage89 focused formatting trace is target/scenarios/20261021-035953-4657968/formatting-negative.log. It reaches the per-function alternative-storage limit in PadAdapter::write_str at 16812736 charged bytes. One demand retains five alternatives and 1455 guards, including 1144 choice guards. The complete run uses 14261959 transfers and 981 frames. This is evidence of retained condition growth, not a claim that the byte loop has an unbounded runtime count.

Stage90 raises the internal value-growth bound from eight to 128. Ordinary formatting negative still fails. Its trace is target/scenarios/20261021-040728-8990729/formatting-negative.log. It reaches the per-function alternative-storage limit in Buffer::write_str at 16798688 charged bytes. It uses 6682737 transfers and 502 frames. This bound change is not retained.

## Next comparison

Keep the stage89 result as the comparison base. Check field-sensitive memory dependencies at equal limits before changing the numeric domain. A definition can be omitted only when its known byte range cannot overlap the requested range. Unknown sizes, indirect writes, and unsupported projections must retain their existing dependencies. The current candidate uses per-range memory templates. It builds, but has no accepted scenario result yet.

```rust
if read_end <= write_start || write_end <= read_start {
    continue;
}
definitions.push(definition);
```

LLVM MemorySSA instead uses a shared memory graph with location-aware alias queries. Compare the storage cost before retaining per-range templates. IKOS uses widening and narrowing over numeric domains, and can keep separate states for return codes. Crab uses the loop structure to select widening points. Eva separates local loop unrolling from a shared state budget. These designs guide the next changes; they do not establish a passing result for this Rust case.

References: https://llvm.org/docs/MemorySSA.html ; https://github.com/NASA-SW-VnV/ikos/blob/master/analyzer/README.md ; https://github.com/seahorn/crab ; https://www.frama-c.com/2018/12/14/New-loop-unroll-annotation-in-Frama-C-18.html .

Stage91 adds the field-sensitive templates with sized-layout checks. Ordinary formatting safe and cache pass; negative fails. The negative log is target/scenarios/20261021-041739-2699441/formatting-negative.log. It uses 14090526 transfers and 981 frames. PadAdapter::write_str still reaches its 16 MiB alternative-storage limit. The driver hash is 1BE5B699F9E219754403D4AE916BBA9710EB65E532C62E7A5BCF6759BDE2877C.

Stage92 uses complete RUPTA write coverage before constructing the local-field memory graph. It omits a call effect only when every covered candidate preserves the requested field. It retains the call destination, unknown effects, and the existing recursion-cut behavior. The graph key includes recursion-cut mode. The six recursive focused configurations and all 18 annotation checks pass. Ordinary formatting validation is pending.

Stage92 completes with 20 of 21 focused results accepted. Ordinary formatting negative still fails, with 15209351 transfers and 981 frames. Its first per-function stop has 22024 demands and 132971168 estimated retained bytes. The log is target/scenarios/20261021-042225-2563268/formatting-negative.log. The driver hash is 975032C57AF4231E8BF3A2B89E0C658A01520334958B49ED677C28E26124D411. This result does not show an improvement over stage89.

The next scheduling comparison puts a newly required dependency before waiting callers. Existing users still enter the ordinary queue. This is not a complete weak-topological-order solver. It tests whether early evaluation of prerequisites reduces provisional state and retained conditions, with no change to value or diagnostic rules.

```rust
if self.flow.queued.contains(&id) {
    active.pending.push_front(id);
}
```

Stage93 runs this scheduling comparison. The six recursive configurations pass. Ordinary formatting is pending. Stage94 extends the same memory-preservation check to caller-owned local fields and statics. The template key includes the backend object identity and byte range. Heap and unknown-object reads retain the old graph. This uses the existing context-independent write proof; it does not infer a write footprint from a missing summary.

```rust
let preserved = covered && targets.iter().all(|target|
    self.preserves_storage(*target, address, requested));
if !preserved {
    entries.push(definition.location.statement_index);
}
```

Stage93 and stage94 each pass 20 of 21 focused checks. Their ordinary negative cases still stop in PadAdapter::write_str. Stage93 uses 15323619 transfers and 981 frames in target/scenarios/20261021-042650-8268756/formatting-negative.log. Stage94 uses 17191354 transfers and 1031 frames in target/scenarios/20261021-043025-2709357/formatting-negative.log. Stage94 also passes all 18 annotation checks. Neither is an accepted performance improvement.

Stage95 also checks completed parameter-effect summaries before constructing memory dependencies. It applies only to direct calls with no unwind edge. Every store must resolve to a fixed local address with a disjoint byte range. Unknown destinations and unsupported summaries retain the call. This avoids treating a cursor-only store as a write to the iterator end field. Compiler checking and release compilation pass; formatting validation is pending.

Stage96 extends this check to the recursion-cut pass only when the existing normal-return proof succeeds. A failed or stopped proof must not establish unchanged memory on a cut route.

```rust
if cut_recursive_returns && !self.returns_normally(callee, irq)? {
    preserve_call_dependency();
}
```

Stage96 builds but is not run separately. The MIR trace shows that the iterator argument uses a reborrow: `_12 = &mut _7; _11 = &mut (*_12); next(_11)`. The first footprint check rejects the second reference because it has a dereference projection. Stage97 adds bounded traversal through pointer copies and this reborrow form. Each local must have one assignment, no address exposure, and a definition that dominates its use. Cycles and unsupported projections retain the call.

Stage97 also omits two kinds of false memory dependencies: an assertion with no unwind edge, and a write through a pointer whose complete backend target set excludes the requested object. Both checks already exist later in value transfer. Moving them before memory-phi construction can keep unchanged fields out of loop merge states. This does not remove control conditions or assertions from call checking. Compiler checking and release compilation are pending for the combined candidate.

Stage95 still fails ordinary formatting negative. It uses 17193461 transfers and 1031 frames in target/scenarios/20261021-043634-9961178/formatting-negative.log. Its driver hash is 6AD121500483129AF20E2F97A352B8B5CE5A3CDE8957CACED97BA8B6009EB21C. The trace confirms that rejecting the reborrow leaves the iterator end field in a loop merge.

Stage97 passes compiler checking and release compilation. Its driver hash is 7BD806DD737CDE822A668DBCFF6CBA5BB6E8E6B4F229F0FE56DDCF81693166F1. The six recursive focused configurations pass. Ordinary formatting, annotation checks, and the standard audit are running. No new kernel run has started. No new test or source comment has been added.

Stage97 completes with 112 of 113 non-panic cases, all 45 cache checks, and all 18 annotation checks passing. Ordinary formatting negative remains the only failed case. Its focused log is target/scenarios/20261021-044422-2732259/formatting-negative.log. It uses 19352307 transfers and 1046 frames. The first storage stop has 41024 demands and 199807936 estimated retained bytes. The main-query graph no longer puts the unchanged iterator end field in a loop merge. This correction does not reduce the complete formatting cost. Panic-format has not been checked for stage97.

Stage98 raises the loop value-growth bound from eight to the configured alternatives allowance, which is 128 in this audit. The other limits do not change. Its driver hash is 11B36FEF51021ECB6373BCF2AF6E8A3F7859CA0C01FB38990ED2F61808C4766D. Ordinary formatting safe and cache pass; negative fails. The negative log is target/scenarios/20261021-044742-7422072/formatting-negative.log. It uses 6080038 transfers and 521 frames. Buffer::write_str reaches its 16 MiB alternative-storage limit. The trace retains concrete used-byte counts through 39. This is bounded iteration, not a numeric loop invariant.

The stage98-scratch64 probe uses the same executable with a 64 MiB function-scratch allowance. Query scratch remains 256 MiB. Safe and cache pass; negative fails. Its log is target/scenarios/20261021-044923-0912944/formatting-negative.log. It reaches the query-scratch limit at 268853984 charged bytes and uses 11308224 transfers and 595 frames. Increasing the local storage allowance does not remove the growth. No default limit is changed.

Stage99 checks completion of an idle dependency closure before reusing an earlier read under new index choices. Previously, a cyclic dependency could remain Evaluated until the complete active queue became empty. The existing index-independent cache check accepted only Complete dependencies. The new check rejects pending, queued, or currently evaluated dependencies. It also rejects any dependency that reads a new index choice. It can mark a closed idle dependency set Complete and notify its external users. It does not treat an unfinished read as complete. Temporary traversal storage remains charged until the check ends.

```rust
if demand.state == DemandState::Pending
    || self.flow.queued.contains(&id)
    || self.flow.current == Some(id)
    || key.choices.iter().any(|(index, _)| demand.indexes.contains(index))
{
    return Ok(None);
}
pending.extend(demand.inputs.iter().copied());
```

Stage99 passes compiler checking and release compilation. Focused validation is running. This is an experiment, not an accepted formatting fix. Stage97 is the last candidate with a complete non-panic audit.

Stage99 completes with 20 of 21 focused checks and all 18 annotation checks passing. Ordinary formatting negative still fails. Its log is target/scenarios/20261021-050033-6407251/formatting-negative.log. Four closure completions each cover 11 states. The first main-query storage stop has 11130 demands, compared with 11174 in stage98. However, total transfers increase from 6080038 to 19684824. The repeated dependency-closure searches cost more than the work they remove. This experiment is removed. Its source, executable, settings, and logs remain saved.

The stage98 loop value-bound experiment is also removed from the working source. The working analysis source returns to stage97. This does not remove the early memory-dependency checks from stages91 through97. A separate stage98-large-probe uses the frozen stage98 driver with query steps 5000000, scalar steps 1000000, function scratch 128 MiB, query scratch 1024 MiB, timeout 1200 seconds, and process memory 10240 MiB. It measures the remaining storage and transfer cost. It does not change the source defaults. Its result is pending.

The rebuilt stage97 source passes compiler checking and release compilation. All eight saved analysis source files match stage97. The rebuilt release driver hash is A1103FF350FC3F9F045F3EF0A24242907285E7D7049D7A73E709B88CB46CF027; the audit used the earlier frozen driver, not this rebuild. A separate stage97-unknown-fmt run enables the existing IRQ_CHECK_TRACE_UNKNOWN trace at the matched audit limits. Early unknown values include reads of uninitialized return-residual locals and unsupported operations on the unused string representation in Arguments::as_str. Those observations alone do not prove a bad reachable binding.

The large probe reaches character counting and Formatter::padding. Formatter::pad has an early return when the width and precision flag bits are zero. The unknown-value trace has not established that these flag bits became unknown before the extra work. Do not infer an incorrect formatter-options value from the presence of padding work alone.

Stage100 evaluates the path to a call before it evaluates the returned value. It applies inside the value-flow worklist. An empty provisional path retains its recorded dependencies, so a later dependency update can repeat the read. A complete empty path needs no callee-return work. This tests work ordering without changing unknown termination into a known return value.

```rust
if self.flow.current.is_some() && self.paths(point, true)?.is_empty() {
    return Ok(Vec::new());
}
```

Stage100 passes compiler checking. Release compilation and focused validation are pending. No result is claimed for this change.

Stage100 passes release compilation, the six recursive focused configurations, and all 18 annotation checks. Its driver hash is C3F6C91DBC93568E1488F164C2FC44264579FC84C15A8DB6BFDBA5694CACE63F. Ordinary formatting is pending. Stage101 combines the same call-path ordering with the configured loop value-growth allowance, as in stage98. This permits a matched comparison with stage98 at the same external limits. Stage101 passes compiler checking; release compilation is running.

LLVM SCCP is a related reference for this ordering question. It tracks executable control-flow edges and revisits phi nodes when new edges become executable. It also distinguishes unresolved values from overdefined values. This checker must still report incomplete required work. It must not copy an optimizer's treatment of undefined behavior as permission to accept an unproved IRQ path. Reference: https://llvm.org/doxygen/SCCPSolver_8cpp_source.html .

The stage97 unknown-value trace completes with safe and cache accepted and negative rejected. The negative log is target/scenarios/20261021-050858-1644013/formatting-negative.log. It records 19351050 transfers and 1046 frames. Keep these counters separate from the earlier stage97 focused run. The trace does not establish that formatter option bits are the first lost relation.

Stage100 completes with 20 of 21 focused checks accepted. Ordinary formatting negative fails in target/scenarios/20261021-051433-7230167/formatting-negative.log, with 15362255 transfers and 255 frames. Its first storage stop has 13075 demands and 172994368 estimated retained bytes. Stage97's corresponding stop has 41024 demands and 199807936 bytes. Checking the call path first removes substantial work, but does not resolve formatting.

Stage101 passes release compilation and the six recursive focused configurations. Ordinary formatting and annotation checks are pending. Stage102 extends the same ordering check to call memory effects, after the existing complete storage-preservation shortcut. A call with a proven unchanged effect does not need this extra path calculation. Pending path dependencies remain dependencies of the active read. Stage102 passes compiler checking; release compilation is pending. These candidates are not full replacements for the solver's loop or memory domains.

Stage101 completes with 20 of 21 focused results accepted and all 18 annotation checks passing. Ordinary formatting negative still fails in target/scenarios/20261021-051921-7636878/formatting-negative.log. It uses 4686067 transfers and 86 frames. Its first main-query storage stop has 2707 demands and 47248224 estimated retained bytes. This is less work than stage98 at the same loop bound and external limits, but both runs stop before a complete answer.

The stage101-scratch64 probe also fails ordinary formatting negative. Safe and cache pass. Its log is target/scenarios/20261021-052111-8908603/formatting-negative.log. It uses 8836880 transfers and 166 frames. Buffer::write_str reaches 67121952 charged function bytes, with 8795 demands and 248631872 estimated retained bytes. The combined ordering and allowance changes are not sufficient.

The stage98-large-probe reaches the 1200-second analysis time limit. Its negative log is target/scenarios/20261021-050420-0356759/formatting-negative.log. It uses 40121892 transfers and 1131 frames. Safe and cache pass; negative fails. A live process sample near the end shows 760143872 private bytes; this is not a peak measurement. The log does not show process-memory exhaustion. The larger query and scratch allowances do not establish a completed result.

Stage102 completes with 20 of 21 focused results accepted and all 18 annotation checks passing. The negative formatting log is target/scenarios/20261021-052330-5075736/formatting-negative.log. It uses 4863102 transfers and 81 frames. Its first reported function-storage stop has 2544 demands and 48358624 estimated retained bytes. Buffer::write_str reaches 16796096 charged function bytes. Source and release-driver identity are checked after the run. All investigation processes have finished.

Compared with stage98 at the same loop bound and external limits, stage102 reduces the formatting frame count from 521 to 81 and transfers from 6080038 to 4863102. Both fail. This is evidence that call work was started too early; it is not evidence that the remaining cost is necessary or that no solver bug remains. The first lost formatter relation is still not isolated. In particular, the presence of padding or character-counting work does not prove a bad formatter option value. A full audit of stage102, panic-format validation, and the remaining formatting correction are still required.
