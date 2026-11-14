//! The secrets engine family: secret scanning with kingfisher, and the `secrets` component serving it.

mod engine;

pub use engine::{ENGINE_NAME, SecretsEngine};

use poly_engines::{ComponentCallError, PolyComponent, Producer};

/// The `secrets` component: the kingfisher scanner behind the component protocol.
pub struct SecretsComponent(Producer);

impl Default for SecretsComponent {
    fn default() -> Self {
        Self(Producer::new(env!("CARGO_PKG_VERSION"), vec![Box::new(SecretsEngine)]))
    }
}

impl PolyComponent for SecretsComponent {
    fn describe(&self) -> Result<Vec<u8>, ComponentCallError> {
        self.0.describe()
    }

    fn call(&self, header: &[u8], body: &[u8]) -> Result<Vec<u8>, ComponentCallError> {
        self.0.call(header, body)
    }
}
