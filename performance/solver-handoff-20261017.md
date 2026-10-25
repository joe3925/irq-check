# IRQ solver investigation handoff

Date: 2026-10-17. Repository: `C:/Users/Boden/Documents/irq-check`.

The current source is the saved **flow75** version. It passes **112 of 113 non-panic cases** and **45 of 45 cache checks**. Only ordinary `formatting/negative` fails. The cache-owner defect is fixed. The complete cause of the remaining formatting failure is not proved.

The main remaining implementation problem is the treatment of stopped refinement. A local scalar limit becomes an ordinary cached `Unknown` value. The cache does not keep the reason for that loss of precision. The same value can survive a query restart. This behavior is confirmed in the source. Its contribution to the remaining failure is not yet isolated.

There is also a confirmed accounting boundary: the scalar owner does not follow queued pointer dependencies. This can put prerequisite work outside the scalar allowance. It is not evidence that all work is unbounded. The main query and memory limits still apply.

The results below come from saved runs. Source comparisons, file hashes, and log checks were repeated for this handoff. No solver code, tests, or comments were added for this report. No new scenario run was made.

## Scope and required rules

- Finish the previously passing scenarios first. Keep `recursive-bindings/panic-format` last.
- Do not run the actual kernel at this stage. The small `kernel-memory` fixtures are part of the scenario audit, not the actual kernel.
- Keep all limits enabled. The user permits limit changes within 10 GiB. The flow75 audit used 2 GiB.
- Do not add tests or source comments unless the user asks for them.
- Do not run whole-project `cargo fmt`.
- Do not add a small helper that has only one caller. The user's rule permits a helper over about 150 lines or one with multiple callers.
- Use plain technical English. Include actual code examples in any complex implementation plan.
- Preserve the dirty worktree. Many changes predate this investigation.
- Do not change a scenario's expected result to hide an `Unknown` error.
- A backend candidate is not an established IRQ call binding. Do not report `Forbidden` from a candidate alone.

The original panic and kernel observations concern an earlier solver version:

| Original failure | Entry capture that stopped |
| --- | --- |
| Panic-format negative | `Formatter::write_str` from `PanicInfo::fmt`, at MIR `bb9[15]` |
| Kernel panic path | `Backtrace::from_state` from `panic_common` |
| Both kernel scheduling paths | `drop<Option<TaskUpdate>>` from `Scheduler::unpark` |

Those runs shared a transfer-limit stop. They did not prove one common cause. Neither panic-format nor the actual kernel was rerun in flow65 through flow75. Do not apply the current pass count to those cases.

The initial panic-format report also recorded repeated stops after 26 summary states. A run with a tenfold query allowance still failed. These are historical observations, not current flow75 measurements.

## Current source and executable identity

The five files below match `target/value-flow-20261016/flow75/source/` byte for byte:

- `src/analysis.rs`
- `src/analysis/global.rs`
- `src/analysis/query.rs`
- `src/analysis/sparse.rs`
- `src/analysis/sparse/flow.rs`

The first three also match flow65. The retained change from flow65 is confined to `sparse.rs` and `sparse/flow.rs`. It adds the owner to the read key, removes duplicate state, and adds an optional cost trace. It keeps the old scalar-only budget rule.

The release driver and saved flow75 driver have this SHA256:

```text
19DCE06DE6B68966FB3C405C5C66C511DBDDBF792EAC1BFAD067BD03AB6D50F3
```

Saved executable pair:

```text
target/value-flow-20261016/flow75/bin/irq-check.exe
target/value-flow-20261016/flow75/bin/irq-check-driver.exe
```

Compiler commit in the logs:

```text
923c95cdf5ba65cea505aa2ea829f578e1506ed8
```

The recorded compiler version was nightly 1.100.0. Scenario checks use `x86_64-unknown-none` and build the required standard-library crates.

`cargo +nightly check --locked --bins` passed before the final audit. The executable hash and source snapshots identify that result. The handoff did not rebuild it.

### Audit settings

```powershell
$env:CARGO_BUILD_JOBS = '1'
$env:IRQ_CHECK_MEMORY_MIB = '2048'
$env:IRQ_CHECK_QUERY_STEPS = '1000000'
$env:IRQ_CHECK_SCALAR_STEPS = '100000'
$env:IRQ_CHECK_ALTERNATIVES = '128'
$env:IRQ_CHECK_TIMEOUT_SECONDS = '300'
$env:IRQ_CHECK_STATS = '1'
```

These are audit settings, not changed defaults. `src/limits.rs:17` still sets these defaults:

| Setting | Default | Flow75 audit |
| --- | ---: | ---: |
| Query steps | 100,000 | 1,000,000 |
| Scalar steps | 1,000 | 100,000 |
| Alternatives | 32 | 128 |
| Memory in MiB | 10,240 | 2,048 |
| Timeout in seconds | 7,200 | 300 |
| Workers | 4, subject to available parallelism | 4 in the logs |

### Full audit result

| Group | Accepted | Total |
| --- | ---: | ---: |
| Standard cases, without panic-format | 93 | 94 |
| Extra cases | 19 | 19 |
| Combined non-panic cases | 112 | 113 |
| Applicable cache checks | 45 | 45 |

The standard and extra counts are separate. Do not add cache checks to the scenario count. The original full set has 115 cases. The two excluded panic-format cases explain the current denominator of 113.

Audit records:

```text
target/value-flow-20261016/flow75/audit-results.json
target/value-flow-20261016/flow75/standard-results.json
target/value-flow-20261016/flow75/extra-results.json
target/value-flow-20261016/flow75/standard-records.json
target/value-flow-20261016/flow75/extra-records.json
```

## Remaining formatting failure

This is `scenarios/formatting`, with feature `negative`. It is not `recursive-bindings` with feature `formatting`, and it is not `panic-format`.

`scenarios/formatting/src/main.rs` builds two formatted messages through a `dyn Write`. The negative feature calls `forbidden_format` from `Label::fmt`. The checker must establish that call without an extra `Unknown` error.

The current full-audit log is:

```text
target/scenarios/20261017-233725-5587448/formatting-negative.log
```

It reports three unknown-call errors at `core::fmt::rt::Argument::fmt`, MIR `bb3[10]`:

| Analysis site | Context | Reason |
| --- | ---: | --- |
| 6 | 13 | `TransferLimit` |
| 9 | 74 | `ScratchLimit` |
| 77 | 809 | The value-flow query has no established value |

The first path is:

```text
irq_entry
  -> panic_common
  -> panic_message
  -> Buffer::write_fmt
  -> SpecWriteFmt::spec_write_fmt
  -> core::fmt::write
  -> core::fmt::rt::Argument::fmt
```

Full-audit counters:

```text
value-flow evaluations=294800 changes=113897 revisions=4
IRQ queries=34 transfers=71699557 frames=1880 sites=153
unknown_coverage=4 diagnostic_cutoff=false
```

A separate focused flow75 run is saved at:

```text
target/scenarios/20261017-233309-6764550/formatting-negative.log
```

That run reports two errors, `TransferLimit` and `ScratchLimit`. Its counters are 278,786 evaluations, 110,785 changes, 71,616,715 transfers, and 1,894 frames. Do not combine its counters with the full-audit log. The diagnostic count and work counts differ between these two failed runs. The reason for that variation was not isolated.

The transfer count is a total across queries and retries. It is not a claim that one query bypassed its allowance. `src/analysis.rs:458` permits one transfer-limit retry and sets its limit to `query_steps * 9`.

`ScratchLimit` does not mean the checker used 2 GiB or 10 GiB. The worklist also has local storage checks. These include a 16 MiB per-function threshold and a 256 MiB query scratch threshold. See `src/analysis/sparse/flow.rs:1534` and `:1552`. The normal audit log does not identify which local check caused this `ScratchLimit`.

### Proof that ordinary formatting passed before this rewrite

`performance/solver-investigation.txt:20` records the old attempt51 audit: 113 of 115 cases passed. Only the two panic-format cases failed. Lines 36 to 41 record the same 94 of 96 standard-case result for attempt63.

The old attempt38 entry, at `performance/solver-investigation.txt:752`, gives a specific cause and fix. The pointer-only preflight returned `NeedsRefinement`. Its caller ignored that result and started a larger snapshot. That snapshot stopped before the exact IRQ query could refine the arguments.

The old change let this result reach the exact-query path:

```rust
Ok(SummaryResult::Safe | SummaryResult::NeedsRefinement)
```

Formatting then passed safe, negative, and cache checks. This did not promote a candidate to `Forbidden`. The exact IRQ query still had to establish the call and its guards.

That result proves the case passed in an older implementation. It is not a drop-in fix for the current worklist solver.

## Confirmed remaining implementation behavior

### Stopped refinement loses its completion status

Current locations:

- `src/analysis/sparse/flow.rs:15`: `Demand` fields.
- `src/analysis/sparse/flow.rs:1453`: an exhausted scalar allowance produces `Ok(Unknown)`.
- `src/analysis/sparse/flow.rs:1473`: a local `TransferLimit` also becomes `Ok(Unknown)`.
- `src/analysis/sparse/flow.rs:1495`: new values join old values.
- `src/analysis/sparse/flow.rs:1499`: an unguarded scalar `Unknown` absorbs the other values.
- `src/analysis/sparse/flow.rs:1507`: a scalar alternative-limit stop also becomes `Unknown`.
- `src/analysis/sparse/flow.rs:311`: `begin_query` resets the scalar counters after cache retention.
- `src/analysis/sparse/flow.rs:319`: `retain_completed` chooses which demands to keep.

The current transfer-limit conversion is:

```rust
let step = match step {
    Err(QueryStop::TransferLimit) if scalar_limit < limit => Ok(vec![Alternative {
        value: Value::Unknown("the scalar value needs more refinement"), guards: Vec::new(),
    }]),
    result => result,
};
```

`Demand` has `stop: Option<QueryStop>`, but this conversion does not set it. The conversion uses the successful result path. There is no separate status for a completed read and a read widened because its allowance ended.

`retain_completed` invalidates pending demands and demands with incompatible index choices. It then invalidates their users. It does not invalidate a demand solely because a local allowance produced its `Unknown` value.

The consequence follows from the code:

1. A scalar demand reaches its local allowance.
2. The solver stores an unguarded `Unknown` as an ordinary value.
3. The demand can leave the pending queue.
4. A later `begin_query` can keep that value while resetting `scalar_steps`.
5. Even if the demand is evaluated again, its old unguarded `Unknown` can absorb newly found values until invalidation removes it.

This is a completion and retry problem. It loses the distinction between an unknown value and an unfinished computation. An `Unknown` can still be conservative; this finding alone does not prove an unsound `Safe` result.

The current key separates different owners. It does not separate query epochs, budgets, or completion states for the same owner.

Experiment flow70 marked limited demands and invalidated them and their users between queries. It did not fix global-update or the larger recursive-formatting run. Thus this remaining problem is not a proved complete explanation of the failures. Flow70 also did not remove the within-query loss of precision when the solver first inserted `Unknown`.

### The scalar allowance does not cover all prerequisites

Current code at `src/analysis/sparse/flow.rs:1288`:

```rust
let origin = (scalar && !control).then(|| self.refinement_origin.unwrap_or(point));
```

A queued non-scalar read has no refinement owner, even if a scalar refinement requested it. When that read is evaluated, its pointer work does not spend the parent's separate scalar allowance. Scalar dependencies reached from it can acquire a different origin.

This is a confirmed accounting boundary. It is a defect if the scalar allowance is intended to bound the whole refinement request. It is not necessarily a defect if the intended contract only counts scalar reads. That contract needs to be explicit.

The experimental full-dependency rule in flow66 was:

```rust
let origin = (!control)
    .then(|| self.refinement_origin.or_else(|| scalar.then_some(point)))
    .flatten();
```

That rule made some formatting cases pass. However, the existing stop handler then widened pointer and aggregate facts at the scalar limit. It introduced other failures. The full-dependency rule is not retained in flow75.

Do not fix this boundary by changing only the owner propagation line again. The experiments show that ownership and stop semantics interact.

## Confirmed defect that is already fixed

### The read cache omitted the refinement owner

In flow65 through flow67, `ReadKey` stored only `refining_guard: bool`. `Demand` stored `refinement_origin: Option<Point>` outside the key. The first request that created the demand assigned its owner. A later request could use the same key with a different owner. It then reused the first owner's value and allowance state.

The retained key now contains:

```rust
struct ReadKey<'tcx> {
    address: Address,
    ty: Ty<'tcx>,
    point: Point,
    effects_only: bool,
    successor: Option<CallEdge>,
    choices: Box<[((Point, mir::Local), u128)]>,
    inputs: Box<[(usize, usize)]>,
    alias_object: Option<Object>,
    refinement_origin: Option<Point>,
    control: bool,
}
```

The worklist reads the owner from the key:

```rust
let origin = key.refinement_origin;
self.refinement_origin = origin;
```

See `src/analysis/sparse.rs:88` and `src/analysis/sparse/flow.rs:1434`. The separate owner on `Demand` and unused `Solver.refining_guard` were removed. `flow_values` now checks `demand.key.refinement_origin.is_some()` for scalar choices.

### Trace evidence

Flow67 added tracing to flow66. It did not add a semantic fix. Its boot-registry negative log is:

```text
target/scenarios/memory-20261017-230225-3079155/stderr.log
```

Settings: query 1,000,000; scalar 100,000; alternatives 128; memory 2,048 MiB; timeout 300 seconds.

The trace contains:

- 95 demands stopped by the scalar allowance.
- 75 stopped demands with `scalar=false`, including pointers and aggregates.
- 3,227 `refinement reuse` events for `Unknown` values under a different owner.
- Five distinct mismatched demand/request pairs.

The old owner was frame2, `bb0[1]`, in `Option::unwrap`:

| Demand | Read | Request owner |
| --- | --- | --- |
| 18 | `Option::map` discriminant | frame3, `bb0[3]` |
| 264 | `Iterator::find` predicate | frame4, `bb4[0]` |
| 28 | `Iterator::find` option discriminant | frame4, `bb2[2]` |
| 36 | `Iterator::next` end predicate | frame6, `bb8[0]` |
| 156 | `NonNull::add` offset | frame21, `bb1[3]` |

The 75 non-scalar stops belong to the experimental full-dependency budget rule. Do not describe them as current flow75 behavior.

Flow68 removed cross-owner reuse by adding the owner to the key. Its 100,000-scalar boot trace still had 581 refinement-stop lines and still failed. Fixing cache identity did not fix the whole refinement algorithm.

## Experiment results

In these tables, `PASS` means the scenario runner accepted the expected diagnostics. A negative run that reports both its expected `Forbidden` and an extra `Unknown` is a failure. Exit code 101 alone is not a pass criterion.

Unless stated otherwise, the settings were query 1,000,000, alternatives 128, memory 2,048 MiB, and timeout 300 seconds. The scalar allowance is shown separately.

### Global-update and ordinary formatting changed in opposite directions

Flow65 passed global-update but failed ordinary formatting negative. After the dependency-accounting and cache-key changes, flow69 passed ordinary formatting but failed both global-update cases. Flow75 restored scalar-only accounting and returned to the flow65 result.

The full audits record this sequence. Ordinary formatting here means `scenarios/formatting`, with feature `negative`.

| Version | Scalar steps | Memory in MiB | Global-update safe and negative | Ordinary formatting negative |
| --- | ---: | ---: | --- | --- |
| Flow65 | 100,000 | 4,096 | PASS | FAIL |
| Flow69 | 250,000 | 2,048 | FAIL | PASS |
| Flow75 | 100,000 | 2,048 | PASS | FAIL |

All three audits used query 1,000,000, alternatives 128, and timeout 300 seconds. Their records are `target/value-flow-20261016/flow65/audit-results.json`, `flow69/audit-results.json`, and `flow75/audit-results.json` under the same parent directory.

Flow66 made queued pointer dependencies inherit the scalar refinement owner and use its allowance. Flow68 also added that owner to the read-cache key. Flow69 kept both changes. The existing stop handler could then replace pointer and aggregate facts with `Unknown` at the scalar limit. Flow75 kept the cache-key fix but restored scalar-only owner propagation.

Global-update was not checked in the first flow66 matrix. Its failure was confirmed in the flow69 full audit. Thus, the records do not identify the first intermediate version that broke global-update.

A separate flow65 control passed global-update safe, negative, and cache checks at query 1,000,000 and scalar 250,000. Flow75 also passed all three at those limits. Flow69 failed both scenario cases at those limits. The scalar allowance increase alone therefore does not explain the global-update failure. The exact transition that caused the failure remains unproved. The control logs are listed in the flow69 and flow75 sections below.

### Flow65 baseline

The full audit passed 112 of 113 non-panic cases and 45 of 45 cache checks. Only ordinary formatting negative failed. This audit used 4,096 MiB and scalar 100,000.

```text
target/value-flow-20261016/flow65/audit-results.json
target/scenarios/20261017-224659-4111704/formatting-negative.log
```

At query 5,000,000 and scalar 100,000, ordinary formatting negative reached the 300-second analysis timeout. Total measured time was 348.961 seconds, including build work. The checker peak was 148,152,320 private bytes. This was not process-memory exhaustion.

```text
target/scenarios/memory-20261017-224140-0019091/stderr.log
target/value-flow-20261016/flow65/formatting-5m/formatting.json
```

The trace included work on `core::fmt::write` template pointer `_11`, the byte loop in `Buffer::write_str`, and scalar control conditions. It did not establish one root cause.

### Flow66 full dependency accounting

This version changed the owner propagation line. It did not put the owner in the cache key.

| Scalar allowance | Boot-registry negative | Recursive formatting negative | Ordinary formatting negative |
| ---: | --- | --- | --- |
| 100,000 | FAIL | PASS | PASS |
| 250,000 | PASS | PASS | FAIL |
| 1,000,000 | PASS | FAIL | PASS |

All corresponding safe and cache checks passed. Focused local-callback, cut, and coverage-unknown checks also passed at 100,000 and 1,000,000. Global-update was not part of this first matrix.

At 250,000, ordinary formatting reported an uninitialized formatting argument and the real `forbidden_format` call. At 1,000,000, recursive formatting reported two `Unknown` errors and the real forbidden service call. No tested scalar setting passed all three negative cases.

Machine-readable matrix:

```text
target/value-flow-20261016/flow66/experiment-results.json
```

Each cell's negative log is in the directory below, under `target/scenarios/`:

| Scalar allowance | Boot-registry directory | Recursive formatting directory | Ordinary formatting directory |
| ---: | --- | --- | --- |
| 100,000 | `20261017-224812-3627344` | `20261017-224816-9129925` | `20261017-224825-5663292` |
| 250,000 | `20261017-225147-5990250` | `20261017-225220-2154596` | `20261017-225224-3031171` |
| 1,000,000 | `20261017-224915-7553683` | `20261017-224952-3236915` | `20261017-225011-1350299` |

Recursive cases use `recursive-bindings-negative.log`. Ordinary formatting uses `formatting-negative.log`.

### Flow67 trace only

This version supplied the 95, 75, and 3,227 counts above. Boot-registry negative still failed. It reported three `Unknown` errors plus the real forbidden target. Counters: 2,253 evaluations, 1,505 changes, 307,141 transfers, and 35 frames.

Do not treat this instrumented run as a passing result or as current source behavior.

### Flow68 owner in the key with full dependency accounting

This version kept the flow66 budget rule and added the owner to the key.

| Scalar allowance | Boot-registry negative | Recursive formatting negative | Ordinary formatting negative |
| ---: | --- | --- | --- |
| 100,000 | FAIL | PASS | PASS |
| 250,000 | PASS | PASS | PASS |
| 1,000,000 | PASS | FAIL | PASS |

The 250,000 setting was the first common setting to pass those three negative cases, their safe cases, and their cache checks. It was not a full-suite result.

At 100,000, boot-registry still reported three `Unknown` errors plus the forbidden target. Its trace had no cross-owner reuse. At 1,000,000, recursive formatting reported a `TransferLimit` at `Argument::fmt`, plus the forbidden call later.

Directories under `target/scenarios/`:

| Scalar allowance | Boot-registry directory | Recursive formatting directory | Ordinary formatting directory |
| ---: | --- | --- | --- |
| 100,000 | `20261017-230359-5326111` | `20261017-230433-1174551` | `20261017-230437-7146951` |
| 250,000 | `20261017-230457-6240432` | `20261017-230533-4401302` | `20261017-230539-0566204` |
| 1,000,000 | `20261017-230700-1304191` | `20261017-230732-7591661` | `20261017-230755-9074923` |

The separate 100,000 boot trace is `target/scenarios/memory-20261017-230534-5054793/stderr.log`.

### Flow69 full audit rejected the broader change

This version removed obsolete fields from flow68 without changing its intended behavior.

At scalar 250,000, the full audit passed **111 of 113** non-panic cases. Both `recursive-bindings/global-update` cases failed. The extra group passed 19 of 19. There were 44 applicable cache checks, and all 44 passed. The failed safe case did not produce its usual cache check.

Ordinary formatting, recursive formatting, and boot-registry passed. Global-update safe reported two `Unknown` errors and a false forbidden-path report. Global-update negative reported unknown paths and its actual forbidden path.

```text
target/value-flow-20261016/flow69/audit-results.json
target/scenarios/20261017-231619-8843363/recursive-bindings-safe.log
target/scenarios/20261017-231619-8843363/recursive-bindings-negative.log
```

The safe run used 2,562,046 transfers, 17,584 evaluations, and 57 frames.

A control run of flow65 at the same query 1,000,000 and scalar 250,000 passed global-update safe, negative, and cache checks:

```text
target/scenarios/20261017-232644-9693589/recursive-bindings-safe.log
target/scenarios/20261017-232644-9693589/recursive-bindings-negative.log
```

The control safe run used 1,189,801 transfers, 2,800 evaluations, and 56 frames. Thus the global-update regression is not explained merely by changing scalar 100,000 to 250,000.

At query 5,000,000 and scalar 1,000,000, flow69 recursive formatting still failed. It finished without a process timeout. It reported an uninitialized object at `Argument::fmt`, unknown storage in `Task::fmt`, and the real forbidden call. Counters: 23,165,527 transfers, 199,813 evaluations, and 549 frames.

```text
target/scenarios/memory-20261017-230950-7415019/stderr.log
target/value-flow-20261016/flow69/recursive-formatting-5m/
```

### Flow70 invalidated results stopped by local limits

This version added `Demand.limited`. Local scalar stops and scalar alternative-limit fallback set it. Cache retention then invalidated those demands and their users between queries.

At query 1,000,000 and scalar 250,000, the three focused cases passed. Global-update safe and negative still failed:

```text
target/scenarios/20261017-231902-4451000/recursive-bindings-safe.log
target/scenarios/20261017-231902-4451000/recursive-bindings-negative.log
```

At query 5,000,000 and scalar 1,000,000, recursive formatting still reported the same two unknown paths and actual forbidden call. Counters: 23,208,759 transfers, 187,396 evaluations, and 572 frames.

```text
target/scenarios/memory-20261017-231325-3126531/stderr.log
target/value-flow-20261016/flow70/recursive-formatting-5m/
```

This version was removed. It shows that cache invalidation alone was insufficient. It does not prove that reuse of limited results is harmless.

### Flow71 traced a callee stack object at the IRQ root

This version returned to flow69 semantics and added a trace at the no-caller case in `flow_entry`. Flow70 invalidation was not present.

At query 5,000,000 and scalar 1,000,000, recursive formatting again failed. Its trace is described in the next section.

```text
target/scenarios/memory-20261017-231649-5081264/stderr.log
target/value-flow-20261016/flow71/recursive-formatting-5m/
```

The added trace is not in current source.

### Flow72 restricted the scalar owner to explicit branch refinement

This version no longer created an owner for every ordinary scalar read:

```rust
let origin = if control { None } else { self.refinement_origin };
```

It also kept scalar choice tracking for all integer types. At scalar 250,000:

- Boot-registry passed.
- Global-update safe and negative failed.
- Recursive formatting negative failed.
- Ordinary formatting negative failed.

Directories under `target/scenarios/`, in that case order: `20261017-232004-1261538`, `20261017-231930-2442392`, `20261017-232011-2730595`, and `20261017-232056-8296673`.

This version was removed.

### Flow73 stopped the optional refinement as one request

This version added an exhausted-origin set. It skipped queued work for an exhausted origin and notified users outside that origin. Branch and assertion checks returned uncertainty at the refinement boundary. Cache retention invalidated exhausted requests and their users. Alternative-limit fallback inside the optional request also stopped the request.

It did not fix the focused cases:

| Scalar allowance | Global-update safe and negative | Boot-registry negative | Recursive formatting negative | Ordinary formatting negative |
| ---: | --- | --- | --- | --- |
| 100,000 | FAIL | FAIL | FAIL | FAIL |
| 250,000 | FAIL | PASS | FAIL | FAIL |

Both first global-update runs started in parallel and used directory `target/scenarios/20261017-232442-0072519`. Their logs collided. Both runner outputs reported failure, but that directory cannot support a per-budget log comparison.

Separate directories for the other cases:

| Scalar allowance | Boot-registry | Recursive formatting | Ordinary formatting |
| ---: | --- | --- | --- |
| 100,000 | `20261017-232513-7987764` | `20261017-232539-1714937` | `20261017-232625-5477233` |
| 250,000 | `20261017-232514-7155150` | `20261017-232520-8357659` | `20261017-232603-8379190` |

This version was removed. It was a local cancellation experiment, not a complete SUPA stage implementation. Its failure does not disprove staged fallback.

### Flow74 separated value and guard allowances

This version used:

```rust
enum Refinement {
    Value(Point),
    Guard(Point),
}
```

Value requests used scalar-only accounting. Guard requests followed pointer dependencies and stopped as a whole. The key and allowance map included the enum. A guard proof started a guard request even inside a value request.

At scalar 250,000, global-update safe and negative still failed. Boot-registry passed. Both formatting negative cases failed.

Directories under `target/scenarios/`: global-update `20261017-232958-2023038`; boot-registry `20261017-233032-1474094`; recursive formatting `20261017-233039-3203708`; ordinary formatting `20261017-233119-0663190`.

This version was removed. The enum, exhausted set, and cancellation helper are not in current source.

### Flow75 retained only the cache identity fix

This version kept the owner in the cache key but restored flow65 scalar-only propagation.

At scalar 100,000, global-update, boot-registry, and recursive formatting passed safe, negative, and cache checks. Ordinary formatting safe and cache passed; its negative case failed.

Focused directories under `target/scenarios/`:

```text
20261017-233223-0201574  global-update
20261017-233259-2411671  boot-registry
20261017-233303-3215583  recursive formatting
20261017-233309-6764550  ordinary formatting
```

At scalar 250,000, global-update also passed all three checks in `20261017-233232-9971433`.

The full audit then confirmed 112 of 113 non-panic cases and 45 of 45 cache checks. There were no new failures relative to flow65.

## Unresolved stack object trace

Flow71 recorded this read at line 786 of its `stderr.log`:

```text
uninitialized-root irq_entry frame=0
address=Address { object: Local(61, _9), offset: 8 }
type=for<'a, 'b> unsafe fn(NonNull<()>, &'a mut Formatter<'b>) -> Result<(), Error>
origin=Some(Point { frame: 10, location: bb20[5] })
selected=None inputs={} recursive=None
```

The next line reads offset zero of the same object as `ArgumentType`. Later requests carry origins at frame10 `bb35[5]`, frame47 `bb1[3]`, and frame64 `bb7[9]`. Some later reads have no refinement origin.

At the IRQ root, the query has no caller through which it can find initialization for this other frame's stack object. Current `flow_entry`, at `src/analysis/sparse/flow.rs:1051`, returns `Unknown` in that situation.

The trace proves that this query reached the root with the callee's object identity. It does not prove why. Possible causes include a lost guard relation, an unmatched call/return path, activation mapping, or paths admitted after refinement stopped.

The order is important: local transfer stops appear by line 51, and scalar exhaustion appears at line 63. The first traced root read appears at line 786. Therefore, this trace does not show a bad activation mapping before budget-related precision loss.

Do not describe this as a proved RCEUS context-selection bug. It is also not a current flow75 reproduction: flow71 used the broader owner-propagation rule.

Useful current code locations:

- `src/analysis/sparse/flow.rs:155`: `boundary_addresses`.
- `src/analysis/sparse/flow.rs:903`: `flow_entry`.
- `src/analysis/sparse/flow.rs:1035`: propagation to callers.
- `src/analysis/sparse/flow.rs:1255`: reads of another frame's local.
- `src/analysis/sparse.rs:150`: frame construction and reuse.

## Time and memory measurements

These values come from the last record in each run's `memory.jsonl`. The checker column is `target_peak_private_bytes`. The whole-run peak also includes build processes. Total seconds include build work and tracing overhead.

| Build and case | Query steps | Scalar steps | Seconds | Checker peak private bytes | Whole-run peak private bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| flow65 ordinary formatting | 5,000,000 | 100,000 | 348.961 | 148,152,320 | 1,327,869,952 |
| flow67 boot trace | 1,000,000 | 100,000 | 26.909 | 123,203,584 | 1,316,868,096 |
| flow68 boot trace | 1,000,000 | 100,000 | 30.751 | 128,000,000 | 1,402,556,416 |
| flow69 recursive formatting | 5,000,000 | 1,000,000 | 60.544 | 215,982,080 | 1,312,788,480 |
| flow70 recursive formatting | 5,000,000 | 1,000,000 | 62.608 | 191,102,976 | 1,359,618,048 |
| flow71 recursive formatting trace | 5,000,000 | 1,000,000 | 52.892 | 217,026,560 | 1,325,572,096 |

In the same order, the measurement directories under `target/scenarios/` are:

```text
memory-20261017-224140-0019091
memory-20261017-230225-3079155
memory-20261017-230534-5054793
memory-20261017-230950-7415019
memory-20261017-231325-3126531
memory-20261017-231649-5081264
```

The measurement tool says `reason=completed` when the process exits. That does not mean the analysis passed. In particular, flow65 exited after its internal analysis timeout, which its log and evidence JSON report.

These measurements do not show a process-memory leak or exhaustion near 10 GiB. They also do not rule out excessive retained state or wasted work. A solver can hit its local scratch threshold well below the process limit.

## Relation to the papers

### SUPA gives a different completion and fallback contract

SUPA section 4.3 counts traversed value-flow edges against a stage budget. If a stage cannot answer a query, the next stage reissues it with less precision. The final fallback uses pre-analysis. Its cache stores completed points-to information. Reuse must meet its stage and context conditions. These are not the same rules as inserting `Unknown` into each unfinished dependency. [SUPA FSE 2016, section 4.3](https://cgi.cse.unsw.edu.au/~jingling/papers/fse16.pdf)

The longer treatment also distinguishes a previously attempted full query from a variable reached with an unknown remaining allowance. Its forward-reuse rule depends on prior failure under the full stage allowance. A nested read that used only another request's remaining allowance does not establish that condition. This is the relevant design difference for the cache-owner and completion findings. [SUPA longer treatment, section 4.3](https://arxiv.org/html/1701.05650#S4.SS3)

This comparison identifies a custom behavior to review. It does not prove that copying one SUPA rule will fix Rust formatting. SUPA's implementation treats arrays as single objects and collapses positive-weight field cycles. The current IRQ solver adds scalar guards, byte offsets, argument-index relations, and activation handling. Those custom requirements need their own invariants. [SUPA FSE 2016, section 5.1](https://cgi.cse.unsw.edu.au/~jingling/papers/fse16.pdf)

### No paper level root cause is established

RUPTA supplies Rust pointer-analysis and call-graph machinery. RCEUS selects contexts from pointer flows and precision-critical functions. These roles do not by themselves define the IRQ solver's budget, completion, or diagnostic policy. [RUPTA CC 2024](https://conf.researchr.org/details/CC-2024/CC-2024-papers/6/A-Context-Sensitive-Pointer-Analysis-Framework-for-Rust-and-its-Application-to-Call-G), [RCEUS ECOOP 2026](https://drops.dagstuhl.de/storage/00lipics/lipics-vol372-ecoop2026/html/LIPIcs.ECOOP.2026.1/LIPIcs.ECOOP.2026.1.html)

This experiment series did not isolate an incorrect RCEUS selection rule or a fundamental limitation of the papers. It also did not prove that coverage gaps are the common cause of all failures.

The current backend uses `context_depth: 0`, `rceus: true`, and `RCEUSCallSiteSensitive::new(0)`. See `src/analysis/global.rs:67`. The README also records disabled stack filtering and cast pruning. This series did not vary that configuration. It remains open to review; it is not a fixed requirement.

The local policy matters. `README.md:30`, `:52`, and `:56` require an established binding before a forbidden-target report. A coarse candidate set can support a safe result if complete coverage proves all candidates safe. It cannot turn an unproved candidate into an established forbidden call.

## Earlier fixes to preserve

- RTA `Drop` and `Assert` edges. The implementation is in `vendor/rupta/src/pre_analysis/rta/body_visitor.rs:195`. The earlier panic-format safe improvement depended on these edges.
- Complete singleton backend bindings from flow58 and flow59. Signature-only or multiple candidates must not be treated as proved bindings. See `src/analysis/sparse.rs:2381`.
- Loop incoming-edge relations from flow60. Control and memory SSA must agree on the same incoming edge. See `Guard.phi` in `src/analysis/query.rs:24` and the merge logic in `sparse/flow.rs`.
- Scoped loop-guard removal from flow63 through flow65. Keep input guards and guards outside the loop and its callees. Repeat affected work when call inputs change. See `src/analysis/sparse/flow.rs:54` and `:362`.
- Scalar merge-choice correlation and the typed choice-value key. Do not merge distinct typed values only because their bit representations match.
- The current lifetime-based scratch accounting, matched call/return handling, activation handling, and complete memory-preservation checks. Do not remove these as part of a budget experiment without a separate reason.

The saved flow65 audit lists the loop and singleton-binding fixes. Current source includes them. Earlier work also tried independent control solvers, whole-solver transactional copies, and a local visited-state cache for guard feasibility. Those changes did not provide a complete fix and were removed. Do not treat them as unused ideas without checking their saved versions.

## What remains unproved

- No single root cause has been established for ordinary formatting, old panic-format, and the actual kernel failures.
- No memory leak has been demonstrated.
- No proof shows that the worklist grows forever. Limits stop the observed runs.
- It is not true that nothing is discarded. `retain_completed` removes invalid demands and their users. `read_edge` can also clear the demand cache at a size threshold.
- The dependency sets grow as new dependencies are found. The top-level read drains the shared pending queue. Whether stale dependencies or work unrelated to the live query dominate the remaining failure is unmeasured.
- The callee stack read at the IRQ root is a symptom, not a proved call-context merge defect.
- More scalar allowance did not reliably improve acceptance. This observation alone is not proof of unsoundness or a violation of the papers.
- Ordinary formatting passed in discarded versions. No discarded version in this series passed all 113 non-panic cases.

## Next investigation targets

First, distinguish semantic uncertainty from incomplete work. A possible representation to evaluate is:

```rust
enum RefinementOutcome<T> {
    Complete(T),
    Stopped(QueryStop),
}
```

This is a design example, not an implemented fix. A complete result can still contain a conservative unknown value. A stopped computation must carry its reason and must not silently become a completed result. The owner, analysis stage, context, and full-versus-remaining allowance must be available when deciding reuse.

Flow70 already tried one part of this: invalidation between queries. The next investigation must also explain what happens within a query when a limited dependency affects its users. Do not assume that clearing caches is sufficient.

Second, trace the first lost formatter binding in current flow75. Record the demand key, object owner, call input, guards, parent dependency, and stop reason. Check whether the first bad relation appears before or after local widening. Do not use flow71's post-exhaustion root read as proof of an earlier activation defect.

Third, identify the exact current `ScratchLimit` branch. `IRQ_CHECK_TRACE=1` can distinguish per-function demand, dependency, alternative-storage, and worklist scratch stops. Record live demands, pending demands, dependency edges, and retained bytes at that point. This can separate storage growth from repeated evaluation.

Fourth, compare one semantic change at equal limits. Flow65 and flow75 global-update controls are useful because both pass at scalar 250,000. Flow68 is useful for an ordinary-formatting pass under the broader propagation rule. Include global-update, boot-registry, and recursive formatting in any focused comparison before a full audit.

Keep the main question narrow: which first transition loses a valid binding or admits an invalid relation? Large frame counts and transfer totals are effects until that transition is identified.

## Reproduction commands

Run from the repository root. Set the audit environment shown earlier first. These commands use existing scenarios; they do not add tests.

```powershell
./scenarios/run.ps1 -Scenario formatting -Fast -VerifyCache -Checker ./target/value-flow-20261016/flow75/bin/irq-check.exe
./scenarios/run.ps1 -Scenario recursive-bindings -Features global-update -Fast -VerifyCache -Checker ./target/value-flow-20261016/flow75/bin/irq-check.exe
./scenarios/run.ps1 -Scenario recursive-bindings -Features boot-registry -Fast -VerifyCache -Checker ./target/value-flow-20261016/flow75/bin/irq-check.exe
./scenarios/run.ps1 -Scenario recursive-bindings -Features formatting -Fast -VerifyCache -Checker ./target/value-flow-20261016/flow75/bin/irq-check.exe
```

For an implementation change, the previous build commands were:

```powershell
$env:CARGO_BUILD_JOBS = '1'
cargo +nightly check --locked --bins
cargo +nightly build --locked --release --bins
```

Use a fresh audit directory for a new full run. The saved `run.ps1` writes reports in its own directory. Do not overwrite the frozen flow75 reports. Copy only its runner scripts and the chosen executable pair into a new directory, then run `-Group standard` and `-Group extra`. Do not run `-Group kernel`.

The relevant scripts are:

```text
target/value-flow-20261016/flow75/run.ps1
target/value-flow-20261016/flow75/focused.ps1
target/value-flow-20261016/flow75/measure-memory-fast.ps1
```

The standard group excludes panic-format and has 94 cases. The extra group has 19 cases. The runner reads the existing matrix at `target/scenario-audit-20261014-024407-results.json`.

Run focused cases sequentially. `scenarios/run.ps1` uses `target/scenario-fast-cargo`. Flow75's saved focused runner uses `target/solver-refactor-extra-fast-cargo`. Flow66's saved focused runner uses `target/memory-scenario-cargo`. Do not run two jobs against the same Cargo target directory.

Do not start timestamp-based scenario runners at the same instant. Flow73 showed that separate jobs can choose the same log directory. Preserve each run's binary hash, environment, log path, and result.

### Optional traces

```powershell
$env:IRQ_CHECK_TRACE = '1'
$env:IRQ_CHECK_TRACE_FLOW_COST = '1'
```

`IRQ_CHECK_TRACE_FUNCTION` and `IRQ_CHECK_TRACE_TRANSFER` can produce very large logs. Narrow their function substring when using them.

Use the frozen flow67 binary for its cross-owner reuse trace. Use the frozen flow71 binary for its added `uninitialized-root` trace. The current flow75 binary does not contain those two experimental trace additions.

## Artifact index and cautions

All experiment snapshots are under `target/value-flow-20261016/flowNN/`. The main subdirectories are `bin/` and `source/`. Early snapshots can contain only the changed sparse files; compare unchanged files with flow65.

The most useful starting files are:

```text
performance/solver-investigation.txt
performance/solver-handoff-20261017.md
target/value-flow-20261016/flow65/audit-results.json
target/value-flow-20261016/flow66/experiment-results.json
target/value-flow-20261016/flow69/audit-results.json
target/value-flow-20261016/flow75/audit-results.json
target/scenarios/20261017-233725-5587448/formatting-negative.log
target/scenarios/memory-20261017-230225-3079155/stderr.log
target/scenarios/memory-20261017-231649-5081264/stderr.log
```

Old JSON fields such as `next_change` describe the state when that report was written. For example, flow69's proposed next change was later tried in flow73. Flow75 is the retained source state.

Do not delete the saved binaries, source snapshots, logs, or measurement files during cleanup. Many are under `target` and are not version-controlled. They are the evidence needed to compare the experiments.

The handoff conclusion is limited but concrete: the cache identity defect is fixed; stopped refinement still loses status; scalar ownership still ends at non-scalar queued reads; and the complete cause of the remaining formatting failure needs a trace of the first incorrect transition.
