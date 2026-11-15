# 0061 — ADR Log Maintenance Through `poly adr`

- Status: Accepted
- Date: 2026-11-15
- Related: ADR 0037, ADR 0038, ADR 0041, ADR 0043, ADR 0058, ADR 0059

## Context

poly lints decision records with adrlint as a library in the `web` component (ADR 0037) and enforces its lock rules
(`ADR50`, `ADR51`) as a project check (ADR 0041), but cannot write `adrlint.lock` or create a record. A repository that
lints its ADRs through poly still installs the standalone adrlint CLI for `lock`, `unlock` and `new` (#221). That puts
two adrlint versions in play and two configs: `[engines.adrlint]` in `poly.toml` and `.adrlint.toml`, with the rule
switches duplicated. The CLI config also cannot express the per-file exemption `poly.toml` can: a template sharing
number `0000` with the first record needs `MADR43` off for that pair alone, which `[per-file-ignores]` does and
adrlint's `[rules]` table cannot.

The adrlint library exposes what the CLI's commands are built from (`lock::entry`, `lock::render`,
`lock::parse_manifest`, `lock::unlock`, `discovery::parse_number`); the template and slug helpers sit behind its `cli`
feature, which poly does not enable. Since ADR 0043 every release triple but musl ships the core build, which does not
link the `web` family, so a CLI command needs the engine across the component protocol on those builds and in-process
on the bundled one.

## Decision

- **`poly adr lock [--all] [--check]`, `poly adr unlock <FILE> --reason <TEXT>` and `poly adr new <TITLE> [--dir
  <DIR>]`** in `poly-cli`, reading the adrlint settings from the config the run resolves (`[engines.adrlint]`, or the
  v1 `[lint.markdown.adrlint]`). The log's directories and the manifest come from the option keys and defaults the
  engine declares in `Engine::project_inputs` (`dir`, `paths`, `recursive`, `lock`), read from the component index, so
  `poly adr` acts on exactly the log `poly lint` judges. The project root is the directory of the nearest `poly.toml`.
- **One new component request, `Request::Adr { op, config, files }`, answered with `Payload::Adr(WireAdrPlan)`.** `op`
  is `Lock { all }`, `Unlock { path }` or `New { dir, title, date }`. The host hands over the files as body slices with
  root-relative paths, as for `ProjectCheck`; the engine reads and writes nothing and answers the files to write
  (`path`, `content`, `create`), the manifest changes and a summary. The planner is one function in the `web` crate,
  `adrlint::ops::plan`; the `web` producer serves it through `Producer::with_adr`, as the `astgrep` producer serves
  `Rules`, and every other component answers `Unsupported`.
- **The bundled build plans in-process.** `poly-bundled` registers the same planner with
  `poly_host_services::adr::register_linked_planner`; without a registration the host calls the component that hosts
  `adrlint`, installing it on demand under `--offline` and `[components] install`, through
  `call_engine_component_with_body`. Both builds run the same function on the same inputs.
- **The lint gate is the host's.** `lock` first runs `poly lint` over the log with `--only adrlint` and refuses (exit 1)
  on any error-severity adrlint finding, as `adrlint lock` refuses on its own errors. Running the real lint means
  `[per-file-ignores]` and inline `poly: allow[…]` directives decide what blocks a lock. A missing component fails the
  gate rather than skipping it, and a gate run that checked no file is an error.
- **Templates are found by name.** `new` fills `template.md` or `NNNN-template.md` in the target directory (front
  matter `status: proposed`, the host's date, the first `#` heading set to the title), else MADR's template as
  `adrlint new` writes it. A template is never locked and never counts toward the next number.
- **Writes stay in the repository.** The host writes each planned path only when it is relative, its directory resolves
  inside the root and the file is not a symlink; a new record is created exclusively and the manifest is replaced
  atomically. No command runs a program, so repository trust (ADR 0059) gates nothing here.
- **`PROTOCOL_VERSION` stays 3.** The variant is appended to `Request` and `Payload`, which are externally tagged by
  name, so no existing message changes shape. Only the component hosting adrlint is ever sent it; an older `web`
  component fails that one call with a decode error and every other request keeps working.

## Consequences

- A repository maintains its ADR log with the adrlint poly ships and one config; `.adrlint.toml` and a separately
  pinned adrlint are no longer needed for lock maintenance.
- The link-target and external-link checks (`MADR11`, `MADR12a`, `MADR12c`) still need the adrlint CLI (ADR 0037).
- `poly adr` on a core build downloads the `web` component the first time, like `poly rules` does `astgrep`.
- A `web` component built before this change answers `Adr` with a protocol error; the release's component lock pins a
  component built with it.
- The lock statuses are adrlint's defaults (`accepted`, `rejected`); poly exposes no `statuses` option.

## Alternatives considered

- **Document the CLI as required and read `.adrlint.toml` from poly.** One config, but still two adrlint versions, and
  the CLI still cannot express a per-pair exemption.
- **A new `Engine` trait method for project actions.** Every engine and the component proxy would carry a method only
  adrlint implements; a component-level handler, as `Rules` has, keeps the trait unchanged.
- **Linking adrlint into the core binary.** It pulls rumdl and the Markdown stack into the build ADR 0043 keeps small.
- **Bumping `PROTOCOL_VERSION` to 4**, as `Rules` once did. It would force every component, the Go sidecars and the
  three separate component workspaces included, to be rebuilt for a request only one of them answers.
- **Planning only the tree-wide rules as the lock gate.** Cheaper, but a record that fails a document rule could be
  locked, which `adrlint lock` refuses.
