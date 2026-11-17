//! The services poly's core exposes to a frontend component.
//!
//! [`Services`] answers the service names of [`poly_frontend_api::wire::service`]: it decodes the
//! JSON request, runs the same `poly-core` pipeline the CLI runs (see [`ops`]) and encodes the
//! reply. It links the engines the host binary links and nothing of the frontend: no `rmcp`, no
//! `tokio`. The component ABI's `HostServices` and the in-process (bundled) server both reach it
//! through [`ServiceCaller`].
//!
//! Config resolution is network-free here: a remote git `extends` base is read from the local
//! source cache when the host injects a resolver, and never fetched (see [`ops`]).

pub mod adr;
pub mod cache;
pub mod failure;
pub mod ops;
pub mod paths;
pub mod run;
mod services;
pub mod state_cache;
pub mod workspace_coverage;
mod workspace_guard;

pub use poly_frontend_api::ServiceCaller;
pub use services::{ComponentsProvider, Services};
pub use workspace_guard::WorkspaceGuard;

/// Loading a config installs the repository's `poly.lock` into process-global state, so the
/// tests that load a config run one at a time.
#[cfg(test)]
static CONFIG_LOADING: std::sync::Mutex<()> = std::sync::Mutex::new(());
