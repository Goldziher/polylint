//! Why a type-aware analysis produced no facts.

use std::path::PathBuf;

/// A failure of the whole analysis. A package or file that has no usable types is no error: it is reported in
/// [`crate::TypeFacts::coverage`] and analysed without types.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TypeError {
    /// A `tsconfig` given explicitly (`[js] tsconfig`) cannot be read or parsed.
    #[error("cannot read the tsconfig {path:?}: {message}")]
    Config {
        /// The config file.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
    /// The checker failed in a way the analysis does not expect (a panic inside tsrs).
    #[error("the type checker failed: {message}")]
    Internal {
        /// What went wrong.
        message: String,
    },
    /// The host cancelled the analysis.
    #[error("the type-aware analysis was cancelled")]
    Cancelled,
}
