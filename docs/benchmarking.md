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

```sh
mise run bench:corpus /tmp/poly-corpora            # rust-lang/book and django/django
mise run bench:corpus /tmp/poly-corpora --large    # adds kubernetes, vscode, home-assistant
mise run bench:hyperfine target/release/poly /tmp/poly-corpora/django /tmp/poly-bench-out
```

`bench:hyperfine` runs `-N -i --warmup 2 --runs 10` with `POLY_CACHE_HOME` in a fresh temp dir and
writes one JSON file per scenario: `startup` (`--version`, `config show`), `cold-lint` and
`cold-fmt` (`--no-cache`), `warm-lint` (cache primed by a first run), and `threads`
(`-j 1,2,4,8,<ncpu>`). It prints the peak RSS of one run per scenario from `/usr/bin/time`.

Corpora without a pinned commit are cloned from the branch tip (`bench:corpus` prints the commit
it got). Pin the SHA before comparing numbers across days.

## Reporting a result

Report instructions retired and wall time for before and after, each the median of 3 runs, on the
same machine and binary profile, with the corpus commit. On Linux read instructions from
`perf stat -e instructions`; on macOS use `xctrace` or the criterion wall time alone and say so.
