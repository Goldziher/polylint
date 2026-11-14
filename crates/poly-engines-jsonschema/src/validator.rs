//! Compiles a local schema and runs it over a document, with a retriever that never leaves the disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use jsonschema::{Retrieve, Uri, Validator};
use poly_engine_api::host_hooks::jsonschema_source::{Scope, file_uri};
use serde_json::Value;

use crate::doc::Document;

/// One schema violation, before it is turned into a diagnostic.
pub(crate) struct Finding {
    pub(crate) pointer: String,
    pub(crate) keyword: String,
    pub(crate) message: String,
    pub(crate) schema_path: String,
}

/// Serves `$ref`s from files under the project root or the schema cache and from nowhere else:
/// no network, and no read outside the directories [`Scope`] allows.
struct LocalRetriever {
    scope: Scope,
}

impl Retrieve for LocalRetriever {
    fn retrieve(&self, uri: &Uri<String>) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let path = self.scope.locate(uri.as_str(), &self.scope.root)?;
        let text = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&text)?)
    }
}

type Compiled = Rc<Result<Validator, String>>;

thread_local! {
    /// Compiled validators by schema path, bytes and cache directory. Compiling is the costly step
    /// and a run validates many files against one schema; a validator is per thread, so nothing
    /// here has to be shared across the rayon pool.
    static COMPILED: RefCell<HashMap<String, Compiled>> = RefCell::new(HashMap::new());
}

/// Runs `check` with the validator compiled from the schema at `schema`, or with the reason it
/// could not be compiled.
pub(crate) fn with_validator<T>(
    scope: &Scope,
    schema: &std::path::Path,
    check: impl FnOnce(Result<&Validator, &str>) -> T,
) -> T {
    let bytes = match std::fs::read(schema) {
        Ok(bytes) => bytes,
        Err(error) => return check(Err(&format!("cannot be read: {error}"))),
    };
    let key = format!(
        "{}\0{}\0{}",
        schema.display(),
        blake3::hash(&bytes).to_hex(),
        scope.cache_dir.display()
    );
    let cached = COMPILED.with(|compiled| compiled.borrow().get(&key).cloned());
    let compiled = cached.unwrap_or_else(|| {
        let built = Rc::new(compile(scope, schema, &bytes));
        COMPILED.with(|compiled| compiled.borrow_mut().insert(key, Rc::clone(&built)));
        built
    });
    match compiled.as_ref() {
        Ok(validator) => check(Ok(validator)),
        Err(reason) => check(Err(reason.as_str())),
    }
}

fn compile(scope: &Scope, schema: &std::path::Path, bytes: &[u8]) -> Result<Validator, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| format!("is not valid JSON: {error}"))?;
    jsonschema::options()
        .with_retriever(LocalRetriever { scope: scope.clone() })
        .with_base_uri(file_uri(schema))
        .build(&value)
        .map_err(|error| format!("is not a usable JSON Schema: {error}"))
}

/// Every violation of the compiled schema in `document`.
pub(crate) fn findings(validator: &Validator, document: &Document) -> Vec<Finding> {
    validator
        .iter_errors(&document.value)
        .map(|error| Finding {
            pointer: error.instance_path().to_string(),
            keyword: error.kind().keyword().to_owned(),
            message: error.to_string(),
            schema_path: error.schema_path().to_string(),
        })
        .collect()
}
