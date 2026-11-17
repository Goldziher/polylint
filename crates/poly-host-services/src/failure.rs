//! Classifying a run that failed before it could report on any file.
//!
//! The CLI answers every such failure with exit code 2 and a message on stderr. A frontend has
//! no exit code, and a host-service error reaches an MCP client as a protocol error that most
//! clients surface as a transport failure, so the model never reads it. The services instead
//! answer with a [`FailureDocument`], which the frontend returns as an `isError` result.

use poly_frontend_api::dto::{Failure, FailureDocument, FailureKind};

/// An error tagged with its [`FailureKind`], so the service table can classify it after it has
/// travelled up through `anyhow`.
#[derive(Debug)]
pub struct RunFailure {
    kind: FailureKind,
    message: String,
}

impl RunFailure {
    /// `error`, classified as `kind`.
    pub fn new(kind: FailureKind, error: impl std::fmt::Display) -> Self {
        Self {
            kind,
            message: error.to_string(),
        }
    }

    /// An `anyhow` error classified as `kind`, keeping its whole context chain in the message.
    pub fn from_anyhow(kind: FailureKind, error: &anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(Self::new(kind, format!("{error:#}")))
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunFailure {}

/// The failure document for `error`: its own class when it carries one, else
/// [`FailureKind::Pipeline`].
#[must_use]
pub fn failure_document(error: &anyhow::Error) -> FailureDocument {
    let error = match error.downcast_ref::<RunFailure>() {
        Some(failure) => Failure {
            kind: failure.kind,
            message: failure.message.trim_end().to_owned(),
        },
        None => Failure {
            kind: FailureKind::Pipeline,
            message: format!("{error:#}"),
        },
    };
    FailureDocument { error, summary: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tagged_error_keeps_its_class_and_anything_else_is_a_pipeline_failure() {
        let tagged = RunFailure::from_anyhow(FailureKind::Config, &anyhow::anyhow!("bad toml"));
        let document = failure_document(&tagged.context("loading"));
        assert_eq!(document.error.kind, FailureKind::Config);
        assert_eq!(document.error.message, "bad toml");
        assert_eq!(
            failure_document(&anyhow::anyhow!("boom")).error.kind,
            FailureKind::Pipeline
        );
    }

    #[test]
    fn the_document_has_a_null_summary() {
        let value = serde_json::to_value(failure_document(&anyhow::anyhow!("boom"))).unwrap();
        assert_eq!(value["summary"], serde_json::Value::Null);
        assert_eq!(value["error"]["kind"], "pipeline");
    }
}
