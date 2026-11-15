#!/usr/bin/env python3
# MISE description="Release-gating pass: run a poly binary over copies of every real local repository (never the trees themselves)"
"""Run one poly binary over pristine copies of every local git repository.

Usage (flags go after ``--`` so mise does not swallow them)::

    mise run harden:repos -- --poly target/release/poly
    mise run harden:repos -- --poly target/release/poly --only xberg,alef --timeout 600

Every repository is copied from ``HEAD`` (``git archive``), re-initialised as a
git repository, and only the copy is ever linted or rewritten. Uncommitted work
in the source trees is deliberately not measured.

Exit status: 1 when a hard invariant failed (poly error, panic, signal, timeout,
non-convergence, cache or thread-count mismatch, component-lock problem), else 0.
Reports are written either way.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve()
ROOT = HERE.parents[2]
DEFAULT_TIMEOUT = 1200
SKIP_NAMES = {"polylint", "reports", ".worktrees"}
SKIP_RATIO = 0.20
SLOW_MS = 2000
VOLATILE = re.compile(r"elapsed|duration|timing|_ms$|^ms$|cpu", re.I)
ANSI = re.compile(r"\x1b\[[0-9;]*m")
PANIC_PATTERNS = {
    "panic": re.compile(r"panicked at"),
    "stack-overflow": re.compile(r"stack overflow", re.I),
    "signal": re.compile(r"SIGABRT|SIGSEGV|SIGBUS|SIGILL|terminated by signal", re.I),
    "component-lock": re.compile(r"embeds no component lock|incompatible component", re.I),
}
HARD_KINDS = {
    "poly-error-exit",
    "poly-errors",
    "panic",
    "stack-overflow",
    "signal",
    "timeout",
    "non-convergence",
    "cache-mismatch",
    "jobs-mismatch",
    "component-lock",
    "bad-json",
}


def polyroot() -> Path:
    for parent in [ROOT, *ROOT.parents]:
        if (parent / "repos.txt").is_file():
            return parent
    return ROOT.parent


@dataclass
class Run:
    name: str
    argv: list[str]
    rc: int | None = None
    wall: float = 0.0
    rss_mb: float | None = None
    timed_out: bool = False
    stdout: str = ""
    stderr: str = ""


@dataclass
class RepoResult:
    name: str
    source: str
    head: str = ""
    runs: dict[str, Run] = field(default_factory=dict)
    flags: list[tuple[str, str]] = field(default_factory=list)
    checked: int | None = None
    skipped: int | None = None
    converged: bool | None = None
    cache_ok: bool | None = None
    jobs_ok: bool | None = None
    speedup: float | None = None
    changed_by_fmt: list[str] = field(default_factory=list)

    def flag(self, kind: str, evidence: str) -> None:
        self.flags.append((kind, evidence))


def git(*args: str, cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=check)


def enumerate_repos(roots: list[Path], only: set[str], skip: set[str]) -> list[tuple[str, Path]]:
    own = git("rev-parse", "--git-common-dir", cwd=ROOT, check=False).stdout.strip()
    own_common = (ROOT / own).resolve() if own else None
    seen: set[Path] = set()
    found: list[tuple[str, Path]] = []
    for root in roots:
        if not root.is_dir():
            print(f"harden:repos: root {root} is not a directory, skipping", file=sys.stderr)
            continue
        for child in sorted(root.iterdir()):
            name = child.name
            if name.startswith(".") or name in SKIP_NAMES or not child.is_dir():
                continue
            if not (child / ".git").exists():
                continue
            probe = git("rev-parse", "--is-inside-work-tree", "--git-common-dir", cwd=child, check=False)
            lines = probe.stdout.split()
            if probe.returncode != 0 or not lines or lines[0] != "true":
                continue
            common = (child / lines[1]).resolve()
            if common in seen or common == own_common:
                continue
            seen.add(common)
            if (only and name not in only) or name in skip:
                continue
            label = name if all(name != n for n, _ in found) else f"{root.name}-{name}"
            found.append((label, child))
    return found


def make_copy(source: Path, dest: Path) -> str:
    dest.mkdir(parents=True)
    archive = subprocess.Popen(["git", "-C", str(source), "archive", "HEAD"], stdout=subprocess.PIPE)
    untar = subprocess.run(["tar", "-x", "-C", str(dest)], stdin=archive.stdout, capture_output=True)
    archive.stdout.close()  # type: ignore[union-attr]
    if archive.wait() != 0 or untar.returncode != 0:
        raise RuntimeError(f"archive/extract failed for {source}: {untar.stderr.decode(errors='replace')}")
    head = git("rev-parse", "HEAD", cwd=source).stdout.strip()
    ident = ["-c", "user.name=harden", "-c", "user.email=harden@localhost", "-c", "commit.gpgsign=false"]
    git("init", "-q", cwd=dest)
    git("add", "-A", cwd=dest)
    git(*ident, "commit", "-q", "--no-verify", "--allow-empty", "-m", f"snapshot of {head}", cwd=dest)
    return head


def time_wrapper() -> list[str]:
    if not Path("/usr/bin/time").exists():
        return []
    return ["/usr/bin/time", "-l" if sys.platform == "darwin" else "-v"]


def parse_rss(text: str) -> float | None:
    mac = re.search(r"^\s*(\d+)\s+maximum resident set size", text, re.M)
    if mac:
        return int(mac.group(1)) / 1048576
    gnu = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", text)
    return int(gnu.group(1)) / 1024 if gnu else None


def execute(name: str, argv: list[str], cwd: Path, env: dict[str, str], timeout: int, scratch: Path) -> Run:
    run = Run(name=name, argv=argv)
    wrapper = time_wrapper()
    time_file = scratch / f"{name}.time"
    cmd = [*wrapper, "-o", str(time_file), *argv] if wrapper else argv
    out_file, err_file = scratch / f"{name}.out", scratch / f"{name}.err"
    start = time.perf_counter()
    with out_file.open("wb") as out, err_file.open("wb") as err:
        proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=out, stderr=err, start_new_session=True)
        try:
            run.rc = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            run.timed_out = True
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
    run.wall = time.perf_counter() - start
    run.stdout = out_file.read_text(errors="replace")
    run.stderr = ANSI.sub("", err_file.read_text(errors="replace"))
    if time_file.exists():
        run.rss_mb = parse_rss(time_file.read_text(errors="replace"))
    return run


def parse_json(text: str) -> dict | None:
    decoder = json.JSONDecoder()
    start = text.find("{")
    while start != -1:
        try:
            value, _ = decoder.raw_decode(text[start:])
        except ValueError:
            start = text.find("{", start + 1)
            continue
        if isinstance(value, dict) and ("results" in value or "summary" in value):
            return value
        start = text.find("{", start + 1)
    return None


def normalize(value):
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in sorted(value.items()) if not VOLATILE.search(k)}
    if isinstance(value, list):
        return sorted((normalize(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
    return value


def comparable(doc: dict) -> str:
    keep = {k: doc.get(k) for k in ("results", "skipped", "errors")}
    return json.dumps(normalize(keep), sort_keys=True)


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:12]


def supports_timings(poly: str) -> bool:
    probe = subprocess.run([poly, "lint", "--help"], capture_output=True, text=True, check=False)
    return "--timings" in probe.stdout + probe.stderr


def scan_logs(res: RepoResult, run: Run) -> None:
    text = run.stdout + "\n" + run.stderr
    for kind, pattern in PANIC_PATTERNS.items():
        hit = pattern.search(text)
        if hit:
            line = next((ln for ln in text.splitlines() if pattern.search(ln)), hit.group(0))
            res.flag(kind, f"{run.name}: {line.strip()[:240]}")
    if run.timed_out:
        res.flag("timeout", f"{run.name}: no exit within the timeout ({run.wall:.0f}s)")
    elif run.rc is not None and (run.rc >= 128 or run.rc < 0):
        res.flag("signal", f"{run.name}: exit status {run.rc}")
    for line in run.stderr.splitlines():
        if "backend run" not in line:
            continue
        ms = re.search(r"elapsed_ms=(\d+)", line)
        if ms and int(ms.group(1)) > SLOW_MS:
            res.flag("slow-file", f"{run.name}: {line.strip()[:240]}")


def check_lint_doc(res: RepoResult, run: Run) -> dict | None:
    doc = parse_json(run.stdout)
    if run.rc == 2:
        res.flag("poly-error-exit", f"{run.name}: exit code 2 (poly verified less than it claims)")
    if doc is None:
        if not run.timed_out:
            res.flag("bad-json", f"{run.name}: stdout holds no results/summary document (rc={run.rc})")
        return None
    errors = doc.get("errors") or []
    if errors:
        res.flag("poly-errors", f"{run.name}: {len(errors)} errors, first: {json.dumps(errors[0])[:240]}")
    return doc


def check_skips(res: RepoResult, doc: dict) -> None:
    summary = doc.get("summary") or {}
    res.checked = summary.get("checked")
    res.skipped = summary.get("skipped")
    skipped = doc.get("skipped") or []
    generated = sum(1 for s in skipped if "generated" in json.dumps(s).lower())
    total = (res.checked or 0) + (res.skipped or len(skipped))
    if total and generated / total > SKIP_RATIO:
        res.flag("skip-outlier", f"{generated}/{total} files skipped as generated ({generated / total:.0%})")


def lint_phase(res: RepoResult, ctx: dict) -> None:
    base = [ctx["poly"], "lint", "--workspace", "never", "--format", "json"]
    timings = ["--timings"] if ctx["timings"] else []
    runs = {
        "lint-cold": [*base, "--no-cache", *timings, "."],
        "lint-prime": [*base, "."],
        "lint-warm": [*base, "."],
        "lint-j1": [*base, "--no-cache", "-j", "1", "."],
    }
    docs: dict[str, dict | None] = {}
    for name, argv in runs.items():
        run = execute(name, argv, ctx["copy"], ctx["env"], ctx["timeout"], ctx["scratch"])
        res.runs[name] = run
        scan_logs(res, run)
        docs[name] = check_lint_doc(res, run)
        if run.timed_out:
            break
    cold = docs.get("lint-cold")
    if cold is not None:
        check_skips(res, cold)
    reference = comparable(cold) if cold is not None else None
    for name, label in (("lint-prime", "cache"), ("lint-warm", "cache"), ("lint-j1", "jobs")):
        doc = docs.get(name)
        if reference is None or doc is None:
            continue
        same = comparable(doc) == reference
        if label == "cache":
            res.cache_ok = same and res.cache_ok is not False
            if not same:
                res.flag(
                    "cache-mismatch",
                    f"{name} differs from lint-cold ({digest(comparable(doc))} vs {digest(reference)})",
                )
        else:
            res.jobs_ok = same
            if not same:
                res.flag(
                    "jobs-mismatch",
                    f"-j 1 differs from default jobs ({digest(comparable(doc))} vs {digest(reference)})",
                )
    cold_run, j1 = res.runs.get("lint-cold"), res.runs.get("lint-j1")
    if cold_run and j1 and cold_run.wall > 0 and not (cold_run.timed_out or j1.timed_out):
        res.speedup = j1.wall / cold_run.wall


def tree_hash(copy: Path) -> str:
    git("add", "-A", cwd=copy)
    return git("write-tree", cwd=copy).stdout.strip()


def fmt_phase(res: RepoResult, ctx: dict) -> None:
    poly, copy = ctx["poly"], ctx["copy"]

    def go(name: str, *args: str) -> Run:
        run = execute(name, [poly, "fmt", *args, "."], copy, ctx["env"], ctx["timeout"], ctx["scratch"])
        res.runs[name] = run
        scan_logs(res, run)
        return run

    dry = go("fmt", "--format", "json", "--no-cache")
    doc = parse_json(dry.stdout)
    if dry.rc == 2:
        res.flag("poly-error-exit", "fmt: exit code 2")
    if doc is None:
        res.flag("bad-json", f"fmt: stdout holds no results/summary document (rc={dry.rc})")
    elif doc.get("errors"):
        res.flag("poly-errors", f"fmt: {len(doc['errors'])} errors, first: {json.dumps(doc['errors'][0])[:240]}")
    if dry.timed_out:
        return
    base = tree_hash(copy)
    first = go("fmt-fix-1", "--fix", "--no-cache")
    tree1 = tree_hash(copy)
    res.changed_by_fmt = git("diff", "--name-only", base, tree1, cwd=copy).stdout.split()
    second = go("fmt-fix-2", "--fix", "--no-cache", "--exit-nonzero-on-fix")
    tree2 = tree_hash(copy)
    after = go("fmt-after", "--no-cache")
    if first.timed_out or second.timed_out or after.timed_out:
        return
    res.converged = True
    if tree1 != tree2 or second.rc == 1:
        res.converged = False
        drift = git("diff", "--name-only", tree1, tree2, cwd=copy).stdout.split()
        res.flag("non-convergence", f"second --fix pass rewrote {len(drift)} files: {', '.join(drift[:5])}")
    if after.rc not in (0, None):
        res.converged = False
        res.flag("non-convergence", f"dry run after --fix still reports drift (exit {after.rc})")
    if first.rc == 2 or second.rc == 2:
        res.flag("poly-error-exit", "fmt --fix: exit code 2")


def process_repo(
    label: str, source: Path, args: argparse.Namespace, out: Path, work: Path, timings: bool
) -> RepoResult:
    res = RepoResult(name=label, source=str(source))
    copy = work / label / "tree"
    scratch = work / label / "scratch"
    scratch.mkdir(parents=True)
    print(f"[{label}] copying {source}", flush=True)
    try:
        res.head = make_copy(source, copy)
    except (RuntimeError, subprocess.CalledProcessError) as err:
        res.flag("copy-failed", str(err)[:240])
        return res
    env = dict(os.environ)
    env["POLY_CACHE_HOME"] = str(work / label / "cache")
    env.setdefault("RUST_LOG", "warn,poly_core::runner::engine_clock=info")
    env["NO_COLOR"] = "1"
    ctx = {"poly": args.poly, "copy": copy, "env": env, "timeout": args.timeout, "scratch": scratch, "timings": timings}
    print(f"[{label}] lint", flush=True)
    lint_phase(res, ctx)
    print(f"[{label}] fmt", flush=True)
    fmt_phase(res, ctx)
    write_repo(res, out / label, timings)
    if not args.keep_copies:
        shutil.rmtree(work / label, ignore_errors=True)
    print(f"[{label}] done, {len(res.flags)} flags", flush=True)
    return res


def write_repo(res: RepoResult, dest: Path, timings: bool) -> None:
    dest.mkdir(parents=True, exist_ok=True)

    def save(file: str, text: str) -> None:
        (dest / file).write_text(text)

    runs = res.runs
    for name, file in (("lint-cold", "lint-cold.json"), ("lint-warm", "lint-warm.json"), ("fmt", "fmt.json")):
        if name in runs:
            save(file, runs[name].stdout)
    for name in ("fmt-fix-1", "fmt-fix-2"):
        if name in runs:
            save(f"{name}.log", runs[name].stdout + runs[name].stderr)
    cold = runs.get("lint-cold")
    note = "" if timings else "binary has no --timings flag\n"
    save("timings.txt", note + (cold.stderr + cold.stdout if cold and timings else ""))
    save("stderr.log", "".join(f"=== {n} (rc={r.rc})\n{r.stderr}\n" for n, r in runs.items()))
    meta = {
        "repo": res.name,
        "source": res.source,
        "head": res.head,
        "checked": res.checked,
        "skipped": res.skipped,
        "converged": res.converged,
        "cache_consistent": res.cache_ok,
        "jobs_consistent": res.jobs_ok,
        "j1_speedup": res.speedup,
        "fmt_changed_files": res.changed_by_fmt,
        "flags": [{"kind": k, "evidence": e} for k, e in res.flags],
        "runs": {
            n: {
                "argv": r.argv,
                "rc": r.rc,
                "wall_s": round(r.wall, 2),
                "peak_rss_mb": r.rss_mb,
                "timed_out": r.timed_out,
            }
            for n, r in runs.items()
        },
    }
    save("meta.json", json.dumps(meta, indent=2) + "\n")


def yn(value: bool | None) -> str:
    return "-" if value is None else ("y" if value else "n")


def secs(res: RepoResult, name: str) -> str:
    run = res.runs.get(name)
    return "-" if run is None else (f"{run.wall:.1f}" + ("!" if run.timed_out else ""))


def write_summary(results: list[RepoResult], out: Path, poly: str, version: str) -> None:
    rows = [
        "| repo | checked | poly errors | fmt converged | cache ok | cold lint s | warm lint s | cold fmt s "
        "| peak RSS MB | j1/jN | flags |",
        "|" + " --- |" * 11,
    ]
    for r in results:
        errors = sum(1 for k, _ in r.flags if k in {"poly-errors", "poly-error-exit"})
        rss = max((x.rss_mb for x in r.runs.values() if x.rss_mb), default=None)
        kinds = ", ".join(sorted({k for k, _ in r.flags})) or "-"
        rows.append(
            f"| {r.name} | {r.checked if r.checked is not None else '-'} | {errors} | {yn(r.converged)} "
            f"| {yn(r.cache_ok)} | {secs(r, 'lint-cold')} | {secs(r, 'lint-warm')} | {secs(r, 'fmt')} "
            f"| {f'{rss:.0f}' if rss else '-'} | {f'{r.speedup:.2f}' if r.speedup else '-'} | {kinds} |"
        )
    head = f"# poly local-repo hardening\n\n- binary: `{poly}`\n- version: `{version}`\n- repos: {len(results)}\n\n"
    (out / "summary.md").write_text(head + "\n".join(rows) + "\n")
    groups: dict[str, list[str]] = {}
    for r in results:
        for kind, evidence in r.flags:
            groups.setdefault(kind, []).append(f"- `{r.name}`: {evidence}")
    body = ["# Failures\n"]
    for kind in sorted(groups, key=lambda k: (k not in HARD_KINDS, k)):
        tier = "hard" if kind in HARD_KINDS else "advisory"
        body.append(f"## {kind} ({tier}, {len(groups[kind])})\n\n" + "\n".join(groups[kind]) + "\n")
    if not groups:
        body.append("No problems flagged.\n")
    (out / "failures.md").write_text("\n".join(body))


def main() -> int:
    top = polyroot()
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--poly", default=os.environ.get("POLY_BIN") or str(ROOT / "target" / "release" / "poly"))
    parser.add_argument("--root", action="append", dest="roots", help="directory whose child git repos are measured")
    parser.add_argument("--out", help="output directory (default <polyrepo root>/reports/polylint-harden/<timestamp>)")
    parser.add_argument("--only", default="", help="comma-separated repo names")
    parser.add_argument("--skip", default="", help="comma-separated repo names")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT, help="seconds per poly invocation")
    parser.add_argument("--parallel", type=int, default=1, help="repos in flight (1 keeps timings meaningful)")
    parser.add_argument("--keep-copies", action="store_true", help="leave the working copies behind")
    args = parser.parse_args()

    poly = shutil.which(args.poly) or args.poly
    if not os.access(poly, os.X_OK):
        print(f"harden:repos: poly binary {args.poly} is not executable", file=sys.stderr)
        return 2
    args.poly = str(Path(poly).resolve())
    roots = [Path(r) for r in args.roots] if args.roots else [top, top.parent / "xberg-io"]
    out = Path(args.out) if args.out else top / "reports" / "polylint-harden" / datetime.now().strftime("%Y%m%d-%H%M%S")
    out.mkdir(parents=True, exist_ok=True)
    split = lambda text: {x for x in text.split(",") if x}  # noqa: E731
    repos = enumerate_repos(roots, split(args.only), split(args.skip))
    if not repos:
        print("harden:repos: no repositories matched; refusing to report a pass over nothing", file=sys.stderr)
        return 2
    (out / "manifest.tsv").write_text("".join(f"{n}\t{p}\n" for n, p in repos))
    probe = subprocess.run([args.poly, "--version"], capture_output=True, text=True, check=False)
    version = (probe.stdout.strip().splitlines() or ["unknown"])[0]
    timings = supports_timings(args.poly)
    print(f"harden:repos: {len(repos)} repos, {version}, --timings={'yes' if timings else 'no'}, out={out}", flush=True)
    work = Path(tempfile.mkdtemp(prefix="poly-harden-repos-"))
    results: list[RepoResult] = []
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.parallel)) as pool:
            futures = [pool.submit(process_repo, n, p, args, out, work, timings) for n, p in repos]
            results = [f.result() for f in futures]
    finally:
        if not args.keep_copies:
            shutil.rmtree(work, ignore_errors=True)
        else:
            print(f"harden:repos: copies kept under {work}")
    write_summary(results, out, args.poly, version)
    hard = [(r.name, k) for r in results for k, _ in r.flags if k in HARD_KINDS or k == "copy-failed"]
    print(f"harden:repos: reports in {out}; {len(hard)} hard failures")
    return 1 if hard else 0


if __name__ == "__main__":
    sys.exit(main())
