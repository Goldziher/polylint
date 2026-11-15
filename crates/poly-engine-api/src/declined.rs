//! [`Declined`]: how a format call reports a file its engine declines when the decline is only
//! known once the call is made.

/// The error a `format` call returns for a file its engine declines ([`Engine::skip_reason`]),
/// carrying the reason.
///
/// A component engine learns the decline from its `Format` reply rather than asking
/// [`Engine::skip_reason`] up front, which would cost a round trip carrying the file per file. The
/// runner downcasts to this type and treats the engine as having declined the file: it left the
/// content unchanged, and when every engine routed to the file declined it the file is reported as
/// skipped with this reason, exactly as an up-front `skip_reason` answer would have been.
///
/// [`Engine::skip_reason`]: crate::engine::Engine::skip_reason
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined(pub String);

impl std::fmt::Display for Declined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "declined: {}", self.0)
    }
}

impl std::error::Error for Declined {}

impl Declined {
    /// The reason, when `error` is a [`Declined`] (at any depth of its context chain).
    #[must_use]
    pub fn reason(error: &anyhow::Error) -> Option<&str> {
        error.downcast_ref::<Declined>().map(|declined| declined.0.as_str())
    }
}
