# 0073 — Root an In-Process lint or fmt Run at the Staged Snapshot

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0071
- Related: ADR 0019, ADR 0067, ADR 0072

## Context

ADR 0071 ran the `lint` and `fmt` hooks in the `poly hooks` process only when the hook's directory was the process
working directory. The default pre-commit run validates the staged snapshot (ADR 0019), so it kept starting a `poly`
child per hook: below the CLI, the per-file tier read the process working directory, and an in-process run over the
snapshot would have judged the snapshot's files with the worktree's configs. ADR 0071 deferred threading a run root
through the per-file tier.

The process working directory cannot be changed for one hook: hooks run concurrently. Every read of it on the lint and
fmt path had to take a root instead. They are:

- in the runner: discovery's frame for a named directory, the per-file-ignore match bases, the project-check root,
  the relative config-frame walk and the config canonicalizer, and the result cache's location;
- in the engines poly-core links: the shellcheck batch's `.shellcheckrc` anchor, the native tools' config
  fingerprint (memoised once per process behind `Engine::version`), the recipe tools' run directory and its display
  form, the directory every native, catalog and recipe tool is started in (a `Command` without `current_dir`, or a
  catalog tool's relative `root`), and the host hooks that key the cache (ruff's package-root base and module-tree
  roots, ruff's discovered target, the JSON Schema project root);
- in ruff: isort's default source roots, which ruff takes from a process-wide copy of the working directory.

Every file path a hook names is relative to the hook's directory, and several components resolve a relative path
against the working directory themselves (`std::path::absolute`, `fs::canonicalize`, ruff's package detection, the
config-file walks of oxc, editorconfig, Go modules and JSON Schema). Some engines also match globs against the path
as given: the format scope, the catalog tier's `path_glob`, the quality tier's test-file rule, typos' exclusions,
ruff's format `exclude` and the Go architecture rules.

## Decision

- **`RunOptions::root`** names the directory a run is rooted at. `None`, what the CLI and the MCP server pass, is the
  process working directory and leaves every behaviour as it was. With a root, `lint_run` / `format_run` are what a
  `poly` child started in the root does: a pure function of the root, the config and the paths.
- **Paths.** The run joins each named path onto the root, so every file read, every upward config walk and every
  engine sees an absolute path under the root, and nothing resolves a relative path against the process directory.
  Before returning, the run spells every path it reports relative to the root again, and removes the root prefix
  from the messages it carries (diagnostic titles and descriptions, error messages, skip reasons), so the report is
  rendered from the same paths and the same message text the child has. The glob sites above match against
  `run_root::relative(path)`, the path as the child spelled it.
- **The root reaches the engines as a thread-local** (`poly_engine_api::run_root`). The `Engine` methods carry no run
  context, and adding one would change every engine, every test that builds a `SourceFile` or an `EngineConfig`,
  and the component wire protocol. A rooted run executes on a rayon pool of its own, sized by `-j`, whose every
  worker has the root pinned for its whole life, so no task of the run, stolen or not, sees another root and no
  other run's task sees this one. Each working-directory read listed above asks `run_root::current_dir()`, which is
  `std::env::current_dir()` outside a rooted run. `tool_run` starts a tool in the root when the caller set no
  directory, and resolves a relative one under it.
- **Process-wide caches are keyed by root.** A native tool's version key, which carries the config fingerprint, is
  kept per (root, tool) for rooted runs and stays in its `OnceLock` otherwise. Ruff's package-root base answers the
  root instead of the remembered process directory, and ruff's settings with isort's source roots anchored at the root
  are memoised per (base settings, root). Every other process-wide memo on the path is keyed by the absolute paths
  the rooted run now uses, so a snapshot run and a worktree run cannot share an entry.
- **The hook handler** roots a `lint` / `fmt` call at the hook's directory (canonical, as the child's `getcwd` is)
  whenever it is not the process directory, and loads the config and the `extends` resolver's repository from there.
  It declines a rooted call that names no file (the child would walk `.`), a file the directory does not hold (the
  child reports it in its own spelling), or a config it cannot load there.
- **A rooted run stays in process only when no component is needed.** A downloaded component engine runs inside its
  own copy of `poly-engine-api` and never sees the root: ruff's default isort source roots and the `oxfmt` fallback
  directory would follow this process's working directory, not the snapshot. Before running, the handler asks
  `poly_core::needs_components` (the run's own component selection, `components::needed`, over the hook's files and
  the snapshot's configs, the configs above them included; nothing installed) and declines when any component would
  serve an engine, so the hook re-execs. A build that links every engine the files need (the bundled musl build, the
  dev `all-engines` build) keeps the in-process path; the core build keeps it for files whose engines it links. An
  unrooted in-process run is unaffected: there a component sees the same working directory as the child. ADR 0071's
  other conditions stand.

## Consequences

- Good, because the default `git commit` run no longer starts a `poly` process per `lint` / `fmt` hook whose engines
  are all linked, which is the case #215 exists for, and its output and exit code are the child's.
- Bad, because in the core build, which serves ruff, the web engines and the rest as components, a staged hook over
  any file one of them handles still re-execs, and the in-process run pays one extra config resolution and
  discovery to find that out.
- Good, because concurrent in-process `lint` and `fmt` hooks over a snapshot each get a pool sized by their own `-j`,
  not the global pool sized by the first one.
- Good, because a run without a root takes exactly the code path it took before: no extra pool, no path rewriting,
  and the thread-local is read only at the sites listed above, never per file except on a rooted run's native-tool
  version lookup and ruff settings lookup, which borrow and allocate nothing once warm.
- Bad, because staying in process with components would need the root on the component wire, a protocol change.
- Bad, because a few rules relate the file path to ruff's `project_root` / `src` spelled differently from the child's
  (opt-in `A005`, `INP001`), and a project check reports from an absolute project root; the run removes the root
  from what it reports, but a rule that compares the two spellings can still answer differently.
- Bad, because the hook runner's `GIT_DIR` / `GIT_INDEX_FILE` reach the child and not an in-process run. On the lint
  and fmt path only the directory walk's `git ls-files` pruning probe reads them, and a hook names its files.
- Bad, because a rooted run starts a pool's threads per call, which a child also did.

## Alternatives considered

- **Pass relative paths and resolve each file read against the root.** Rejected: ruff, oxc and the config walks
  resolve a relative path against the process directory themselves, inside crates poly does not own.
- **Add the root to `SourceFile` or `EngineConfig`.** Rejected: several hundred struct literals change, and the
  component wire protocol with them.
- **Run in process with components and accept the difference.** Rejected: ruff's isort roots and the `oxfmt`
  fallback directory would read the worktree, which is the false pass ADR 0019 exists to prevent, in the build most
  users run.
- **Keep the global pool and enter the root around each task.** Rejected: a worker blocked inside one run's task
  steals tasks of another run, and every task entry point would need a guard.
