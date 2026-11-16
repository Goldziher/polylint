//! `poly-hooks` — sync execution building blocks for the poly native hook runner.
//!
//! The low-level execution primitives ported from the vendored `polyhooks` (prek)
//! fork in synchronous, tokio-free form, driven by a rayon-based runner.
//!
//! # Modules
//!
//! - [`identify`] — file-type tagging by filename, shebang, and interpreter.
//! - [`stage`] — [`Stage`] enum + [`HookType`] and their mapping.
//! - [`git`] — synchronous git helpers (staged files, diff, worktree state).
//! - [`install`] — install/uninstall the git-hook shims that invoke `poly hooks hook-impl`.
//! - [`hook_impl`] — parse a fired git hook's args/stdin into [`hook_impl::RunInputs`].
//! - [`filter`] — filename + tag-based file filtering primitives.
//! - [`model`] — the in-memory hook model ([`Hook`], [`StageSpec`], request/outcome types).
//! - [`snapshot`] — staged-content snapshots for whole-workspace hook isolation.
//! - [`timeout`] — per-hook time budgets and the still-running announce cadence.
//! - [`runner`] — the native rayon hook runner ([`run`]).
//!
//! - [`process`] — synchronous [`Cmd`] wrapper over [`std::process::Command`].
//! - [`pty`] (Unix-only) — blocking PTY primitives for colored subprocess output.
//!
//! Output rendering, concurrency sizing, cargo-lock probing and child
//! supervision are crate-internal; [`HookRunReporter`] and [`CaptureSink`] are
//! their public face.
//!
//! # Entry point
//!
//! [`run`] executes a [`HookRunRequest`] (a set of [`StageSpec`]s) on a
//! dedicated rayon pool and returns a [`HookRunOutcome`]. Per stage the order is
//! precondition → before → hooks (rayon) → after.
//!
//! Every configured hook appears in the outcome even when it did not run —
//! [`HookStatus::Skipped`] when a `precondition` declared it inapplicable,
//! [`HookStatus::Unknown`] when its setup failed and its verdict is therefore
//! undetermined, [`HookStatus::TimedOut`] when poly killed it for overrunning
//! its [`timeout`] budget. [`HookRunOutcome::validated_nothing`] tells a machine
//! consumer that the run completed without checking anything.

pub mod filter;
pub mod git;
pub mod hook_impl;
pub mod identify;
pub mod in_process;
pub mod install;
pub mod model;
pub mod process;
pub mod runner;
pub mod snapshot;
pub mod stage;
pub mod timeout;

pub(crate) mod cargo_lock;
pub(crate) mod concurrency;
pub(crate) mod consts;
pub(crate) mod fs;
pub(crate) mod reporter;
pub(crate) mod supervise;

#[cfg(unix)]
pub mod pty;

pub use hook_impl::{PushInfo, RunInputs};
pub use model::{
    Hook, HookCache, HookCommand, HookOutcome, HookRunOutcome, HookRunRequest, HookStatus, SccacheSettings, SetupScope,
    SkipReason, StageOutcome, StageSpec, StageStatus, StepOutcome, TimedOutPhase, TimeoutReason, UnknownReason,
};
pub use process::{Cmd, OutputSink};
pub use reporter::{CaptureSink, HookRunReporter, live};
pub use runner::run;
pub use stage::{HookType, Stage};
