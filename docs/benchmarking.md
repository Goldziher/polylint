# Benchmarking

Two layers: criterion benches for isolated stages, and hyperfine scenarios for the released binary
over a real corpus. Judge a change by **instructions retired plus wall time, the median of 3
invocations**; a single wall-time number on a shared machine is not evidence.

## Criterion benches

All inputs are generated at start-up into a temp dir, deterministic and offline. Engines that the
default build does not link are skipped, not downloaded.

| Command | Measures |
|---------|----------|
| `cargo bench -p poly-core --bench runner_modes` | lint and fmt with the cache off over 5k tiny files, one 2 MB file and a 700-file mixed tree (Rust, Python, JSON, YAML, Markdown, TOML, shell); lint with `jobs = 1` versus all cores; lint and fmt with the result cache cold versus warm (the stat fast path replay) |
| `cargo bench -p poly-core --bench discover_walk` | `discover` over 50k files in nested directories, without and with `.gitignore` files at every level |
| `cargo bench -p poly-core --bench runner_e2e` | the original 64-file Rust fmt dry run |
| `cargo bench -p poly-core --bench cache_key` | cache digest, key and lookup costs |
| `cargo bench -p poly-core --bench generic_formatter` | tree-sitter parse and reindent |
| `cargo bench -p poly-component-host --bench component_call` | `Connection::call` over an in-memory echo transport with 1 KiB, 64 KiB and 1 MiB bodies, single and 64-call batches on 1 and 8 rayon threads; `EngineConfig::fingerprint` with 0, 10 and 100 options |
| `cargo bench -p poly-component-protocol --bench protocol_codec` | request encode and decode; response encode and decode with 0, 10 and 1000 diagnostics |

Save a baseline and compare: `cargo bench -p poly-core --bench runner_modes -- --save-baseline before`,
then `-- --baseline before` after the change.

## Binary scenarios (hyperfine)

### Build a binary that runs every engine

The root `poly-cli` build links only the core engines. The web (TOML, Markdown, JSON, YAML, JS/TS,
CSS), biome and ruby engines are components, and a build without an embedded component lock prints
`this build embeds no component lock` and skips them: timings of such a binary measure almost
nothing. Benchmark the **bundled** binary, which links every engine statically and needs no lock:

```sh
cargo build --release --locked --manifest-path components/Cargo.toml -p poly-bundled
# binary: components/target/release/poly
```

To compare two revisions, build each the same way from its own `mise run worktree:add` checkout and
keep the binaries (`cp components/target/release/poly /tmp/poly-old`). A released core binary
(`poly-<version>-<triple>`) also works: it embeds a signed lock and downloads the components on first
use, which is network-dependent, so warm its component cache with one `poly lint <corpus>` first.
`bench:hyperfine` refuses any binary that prints `embeds no component lock` for the corpus unless
`--allow-missing-components` is passed.

### Run

```sh
mise run bench:corpus /tmp/poly-corpora            # rust-lang/book and django/django at pinned commits
mise run bench:corpus /tmp/poly-corpora --large    # adds kubernetes, vscode, home-assistant
mise run bench:corpus /tmp/poly-corpora --latest   # default-branch tips instead (not comparable across days)
mise run bench:corpus /tmp/poly-corpora --local ~/work/repo-a ~/work/repo-b   # git archive HEAD copies

mise run bench:hyperfine /tmp/poly-corpora/django /tmp/poly-bench-out \
  --old /tmp/poly-old --new components/target/release/poly
```

The corpus is scratch (the touched scenario rewrites mtimes); never point it at a working tree.
`--old` is optional; `--new` alone prints a plain table. Runs use `-N -i` with `--warmup 3 --runs 30`
(`--warmup`/`--runs` or `POLY_BENCH_WARMUP`/`POLY_BENCH_RUNS`) and a throwaway `POLY_CACHE_HOME`.
Per binary, `OUT_DIR/{old,new}/` gets one JSON per scenario plus `rss.txt`, and `OUT_DIR/summary.md`
the side-by-side table (mean ± σ, wall delta, instructions delta where hyperfine reports them; a
delta smaller than the combined σ is labelled `within noise`).

| Scenario | What it exercises |
|----------|-------------------|
| `startup` | `--version`, `config show` |
| `cold-lint`, `cold-fmt` | `--no-cache`, every file through its engine |
| `warm-lint`, `warm-fmt` | cache primed, stat fast path replay |
| `touched-lint` | cache primed, then every mtime changed with identical bytes: the content-digest path |
| `hooks` | `poly hooks run pre-commit --all-files`, only when the corpus `poly.toml` has `[hooks]` |
| `threads` | cold lint at `-j 1,2,4,8,<ncpu>` |

Corpora are pinned to commits in `mise-tasks/bench/corpus.sh`; repin with `git ls-remote <repo> <branch>`.

## Profiling

```sh
mise run profile:samply /tmp/poly-corpora/django               # CPU profile, bundled binary
mise run profile:samply /tmp/poly-corpora/django --cli -- fmt  # poly-cli alone, custom args
samply load target/profile/poly-samply.json.gz
mise run profile:dhat /tmp/poly-corpora/django                 # heap profile: target/profile/dhat-heap.json
```

Both build `[profile.profiling]` (release plus line tables, unstripped). `samply` is not pinned by
mise (like `hyperfine`): `cargo binstall samply` or `brew install samply`. `profile:dhat` builds
`poly-cli` with the `dhat-heap` feature (core engines only), which swaps the global allocator for
dhat's and excludes the musl mimalloc one; open the JSON in
[dh_view](https://nnethercote.github.io/dh_view/dh_view.html) (it loads the file locally). dhat writes on a
normal return from `main`, not on `process::exit` paths.

## Reporting a result

Report instructions retired and wall time for before and after, each the median of 3 runs, on the
same machine and binary profile, with the corpus commit. On Linux read instructions from
`perf stat -e instructions`; on macOS use `xctrace` or the criterion wall time alone and say so.
