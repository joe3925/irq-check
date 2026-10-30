import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid


parser = argparse.ArgumentParser(description="Run the kernel Unknown cases and their controls.")
parser.add_argument("--checker", type=Path)
parser.add_argument("--toolchain", default="nightly")
parser.add_argument("--target", action="append", choices=["x86_64-unknown-none", "aarch64-unknown-none"])
parser.add_argument("--jobs", type=int, default=14)
args = parser.parse_args()
if args.jobs < 1:
    parser.error("--jobs must be at least 1")

root = Path(__file__).resolve().parent.parent
checker = (args.checker or root / "target/release/irq-check.exe").resolve()
targets = list(dict.fromkeys(args.target or ["x86_64-unknown-none", "aarch64-unknown-none"]))
environment = os.environ.copy()
for name in list(environment):
    if name.startswith("IRQ_CHECK_") or name in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}:
        environment.pop(name)
settings = {
    "query_steps": 1000000,
    "scalar_steps": 1000000,
    "alternatives": 128,
    "workers": 4,
    "memory_mib": 10240,
    "timeout_seconds": 1200,
    "call_depth": 256,
    "function_scratch_mib": 16,
    "query_scratch_mib": 256,
}
environment.update({"IRQ_CHECK_" + key.upper(): str(value) for key, value in settings.items()})
environment.update(RUSTUP_TOOLCHAIN=args.toolchain, CARGO_BUILD_JOBS="1", IRQ_CHECK_STATS="1", IRQ_CHECK_EXPLAIN="1")
subprocess.run([str(checker), "--self-check"], cwd=root, env=environment, check=True)
identity = subprocess.check_output([str(checker), "--version"], cwd=root, env=environment, text=True).strip()
compiler = re.search(r"\b[0-9a-f]{40}\b", identity)
if compiler is None:
    raise SystemExit("Cannot read the compiler identity.")

matrix = {
    "kernel-scheduler": ["", "native-alloc", "trait-context", "direct-root", "native-alloc,trait-context", "native-alloc,direct-root", "task-scan", "native-alloc,task-scan"],
    "kernel-metadata": ["", "opaque-init", "try-lock", "opaque-init,try-lock"],
    "kernel-caller-inputs": ["", "few-callers", "opaque-init", "opaque-init,few-callers"],
}
jobs = [(name, features, target) for name, variants in matrix.items() for features in variants for target in targets]
run = root / "target/kernel-repros" / (datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:8])
run.mkdir(parents=True)
methods = {"enqueue", "contains_cpu", "pick_next", "on_switch_out", "should_preempt", "maybe_balance"}
method_roots = {"<Domain<FifoClass> as DomainOps>::" + method for method in methods}
source_paths = [Path(__file__).resolve(), root / "scenarios/Cargo.toml", root / "scenarios/Cargo.lock"]
for name in matrix:
    source_paths.extend([root / "scenarios" / name / "Cargo.toml", root / "scenarios" / name / "src/main.rs"])
state = {
    "started_utc": datetime.now(timezone.utc).isoformat(),
    "checker": str(checker),
    "identity": identity,
    "driver_sha256": hashlib.sha256(checker.with_name("irq-check-driver.exe").read_bytes()).hexdigest(),
    "settings": settings,
    "sources": {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in source_paths},
    "kernel_mapping": {
        "kernel-scheduler": "scheduling/domain.rs: the six DomainOps calls through Once and boxed domain storage",
        "kernel-metadata": "debug_metadata.rs: sink(bytes) through spin::Mutex<Option<fn(&[u8])>>",
        "kernel-caller-inputs": "debug_metadata.rs: formatting loses retained caller inputs",
    },
    "not_reproduced": [
        "The kernel's 9,000,000-transfer query stops",
        "The kernel's 144 alternatives at a limit of 128",
        "The exact local _5 status-discriminant loss in spin::Once::get",
    ],
    "active": {},
    "results": [],
}
lock = threading.Lock()


def save():
    temporary = run / "results.tmp"
    temporary.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    temporary.replace(run / "results.json")


def execute(job):
    name, features, target = job
    selected = set(features.split(",")) - {""}
    identifier = name + "--" + (features.replace(",", "-") or "default") + "--" + target
    directory = run / identifier
    directory.mkdir()
    log = directory / "run.log"
    evidence_path = directory / "evidence" / (name.replace("-", "_") + ".json")
    local = environment.copy()
    local.update(IRQ_CHECK_REQUIRED_CRATE=name.replace("-", "_"), IRQ_CHECK_DUMP_DIR=str(evidence_path.parent), CARGO_TARGET_DIR=str(run / "cargo" / str(jobs.index(job))))
    command = [str(checker), "--cargo", "check", "--locked", "--manifest-path", str(root / "scenarios" / name / "Cargo.toml"), "-p", name, "--release", "--target", target, "-Zbuild-std=core,alloc,compiler_builtins", "-Zbuild-std-features=compiler-builtins-mem"]
    if features:
        command.extend(["--features", features])
    started = time.monotonic()
    with log.open("w", encoding="utf-8") as output:
        process = subprocess.Popen(command, cwd=root, env=local, stdout=output, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        with lock:
            state["active"][identifier] = process.pid
            save()
        process.wait()
    output = log.read_text(encoding="utf-8", errors="replace")
    evidence = json.loads(evidence_path.read_text(encoding="utf-8")) if evidence_path.exists() else {}
    unknown = len(re.findall(r"^error: interrupt call path cannot be fully checked", output, re.M))
    forbidden = len(re.findall(r"^error: forbidden operation is reachable", output, re.M))
    errors = re.findall(r"^error[^\n]*", output, re.M)
    other_errors = [error for error in errors if not re.match(r"error: (interrupt call path cannot be fully checked|aborting due to \d+ previous errors?|could not compile `" + re.escape(name) + r"`)", error)]
    roots = {entry["instance"] for entry in evidence.get("roots", [])}
    required_roots = {"irq_entry"}
    required_notes = []
    if name == "kernel-scheduler":
        expect_unknown = False
        required_roots = method_roots if "direct-root" in selected else {"irq_entry", "<TaskHandle as core::ops::Drop>::drop"}
        if "trait-context" in selected:
            required_roots = required_roots | method_roots
    elif name == "kernel-metadata":
        expect_unknown = "opaque-init" in selected
        if expect_unknown:
            required_notes = ["the summary pointer operation has no established input pointer", "the loop summary cannot exclude a write to the requested storage"]
    else:
        expect_unknown = "few-callers" not in selected or "opaque-init" in selected
        if "few-callers" not in selected:
            required_notes.append("the caller inputs are not retained in this function summary")
        if "opaque-init" in selected:
            required_notes.append("the loop summary cannot exclude a write to the requested storage")
    unknown_edges = [edge for edge in evidence.get("edges", []) if edge.get("evidence") == "unknown"]
    unknown_sites = {(edge["caller"], edge["caller_context"], edge["span"]) for edge in unknown_edges}
    unknown_candidates = {edge["callee"] for edge in evidence.get("candidates", []) if (edge["caller"], edge["caller_context"], edge["span"]) in unknown_sites}
    required_crates = {name.replace("-", "_"), "core"}
    if name != "kernel-caller-inputs":
        required_crates.add("spin")
    if name == "kernel-scheduler":
        required_crates.add("alloc")
    checks = {
        "exit": process.returncode != 0 if expect_unknown else process.returncode == 0,
        "unknown": unknown > 0 and bool(unknown_edges) if expect_unknown else unknown == 0 and not unknown_edges,
        "no_forbidden": forbidden == 0,
        "no_other_errors": not other_errors and not re.search(r"internal compiler error|panicked at|did not reach a complete result|metadata is missing", output),
        "complete": evidence.get("incomplete") is False and evidence.get("diagnostics_complete") is True,
        "roots": roots == required_roots,
        "target": evidence.get("target") == target,
        "compiler": evidence.get("compiler") == compiler.group(),
        "crates": required_crates.issubset(evidence.get("checked_crates", [])),
        "limits": evidence.get("limits") == settings,
        "loss": all(note in output for note in required_notes),
    }
    if name == "kernel-scheduler" and not selected.intersection({"trait-context", "direct-root"}):
        binding_path = evidence_path.with_name("kernel_scheduler-trait-bindings.json")
        bindings = json.loads(binding_path.read_text(encoding="utf-8")) if binding_path.exists() else []
        established = {target for binding in bindings if binding.get("complete_binding") for target in binding["targets"]}
        checks["six_dispatch_calls"] = method_roots.issubset(established)
    if expect_unknown and name == "kernel-metadata":
        checks["sink_call"] = "emit" in unknown_candidates and any(edge["caller"] == "sink_write" for edge in unknown_edges)
    matched = all(checks.values())
    record = {
        "id": identifier,
        "scenario": name,
        "features": features,
        "target": target,
        "expected": "unknown" if expect_unknown else "safe",
        "status": ("reproduced" if expect_unknown else "safe-passed") if matched else "mismatch",
        "matched": matched,
        "checks": checks,
        "command": command,
        "exit_code": process.returncode,
        "seconds": round(time.monotonic() - started, 3),
        "unknown": unknown,
        "forbidden": forbidden,
        "errors": errors,
        "roots": sorted(roots),
        "required_notes": required_notes,
        "losses": list(dict.fromkeys(item["message"] for site in evidence.get("sites", []) for item in site["diagnostics"] if "information was lost" in item["message"] or "unresolved storage read" in item["message"])),
        "incomplete": evidence.get("incomplete"),
        "diagnostics_complete": evidence.get("diagnostics_complete"),
        "log": str(log),
        "evidence": str(evidence_path),
    }
    with lock:
        state["active"].pop(identifier)
        state["results"].append(record)
        save()
        failed = ",".join(key for key, value in checks.items() if not value)
        print(f"{identifier}: {record['status']} unknown={unknown} seconds={record['seconds']} failed={failed}", flush=True)
    return record


save()
print(f"Run {len(jobs)} cases with {min(args.jobs, len(jobs))} parallel jobs. Results: {run / 'results.json'}", flush=True)
with ThreadPoolExecutor(max_workers=args.jobs) as pool:
    for future in as_completed([pool.submit(execute, job) for job in jobs]):
        future.result()
state["finished_utc"] = datetime.now(timezone.utc).isoformat()
state["matched"] = all(record["matched"] for record in state["results"])
state["results"].sort(key=lambda record: record["id"])
save()
print(f"Matched {sum(record['matched'] for record in state['results'])}/{len(jobs)} cases. Results: {run / 'results.json'}", flush=True)
raise SystemExit(0 if state["matched"] else 1)
