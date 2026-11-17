//! The order the programs of an analysis start in: the early plan's, then the judged files' plan's.

use std::collections::BTreeSet;

/// The programs not started yet: those of the early plan until the judged files are planned, then those of the
/// judged files' plan that did not start early.
#[derive(Debug, Default)]
pub(super) struct Queue {
    /// The next position in [`Early::configs`].
    early: usize,
    /// The next position in [`Judged::order`].
    judged: usize,
    /// The configs of the programs started.
    started: BTreeSet<String>,
}

/// Where the plan of the judged files stands, as [`Queue::next`] reads it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Gate<'a> {
    /// Not handed over yet.
    Closed,
    /// Handed over: the configs of its programs in the order they start.
    Open(&'a [String]),
    /// The host gave the analysis up.
    Abandoned,
}

/// What a program thread does next.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Next {
    /// Build the program at this position of [`Early::configs`].
    Early(usize),
    /// Build the program at this position of [`Judged::order`].
    Judged(usize),
    /// Wait for the judged files' plan: every early program has started, or the next one has no cores.
    Wait,
    /// End: every program has started.
    Done,
}

impl Queue {
    /// The next program to start, of the `early` configs or, once the gate is open, of the judged files' plan. An
    /// early program starts only when `admit` lets the one at its position have its cores; otherwise it stays queued
    /// and the thread waits for the judged files' plan.
    pub(super) fn next(&mut self, early: &[String], gate: Gate<'_>, admit: impl FnOnce(usize) -> bool) -> Next {
        match gate {
            Gate::Abandoned => Next::Done,
            Gate::Open(order) => {
                while let Some(config) = order.get(self.judged) {
                    let position = self.judged;
                    self.judged += 1;
                    if self.started.insert(config.clone()) {
                        return Next::Judged(position);
                    }
                }
                Next::Done
            }
            Gate::Closed => {
                let Some(config) = early.get(self.early) else {
                    return Next::Wait;
                };
                if !admit(self.early) {
                    return Next::Wait;
                }
                let position = self.early;
                self.early += 1;
                self.started.insert(config.clone());
                Next::Early(position)
            }
        }
    }
}
