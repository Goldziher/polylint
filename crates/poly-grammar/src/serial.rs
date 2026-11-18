//! Grammars that cannot be parsed on two threads at once.
//!
//! tree-sitter keeps an external scanner's state in a per-parser payload, but a
//! scanner may ignore it and use a process-global variable instead. Two workers
//! parsing with such a grammar then flip each other's state mid-parse, and the
//! scanner can emit a zero-width token at end of input forever: the parse never
//! finishes and its error-recovery stacks grow until the process is killed.
//! Every parse with one of these grammars holds [`serial_parse`]'s guard.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Grammars whose external scanner keeps its state in a process-global variable.
///
/// `properties`: `static bool reached_eof` in tree-sitter-properties' `scanner.c`
/// (tree-sitter-language-pack 1.25.2). Under parallel lint runs it took
/// openapi-generator's `gradle-wrapper.properties` files past 30 GB of memory.
const GLOBAL_SCANNER_STATE: &[&str] = &["properties"];

static SERIAL: Mutex<()> = Mutex::new(());

/// A guard to hold across one parse with the grammar `name`, or `None` when the
/// grammar is safe to parse concurrently.
#[must_use = "the parse is serialized only while the guard is held"]
pub fn serial_parse(name: &str) -> Option<MutexGuard<'static, ()>> {
    GLOBAL_SCANNER_STATE
        .contains(&name)
        .then(|| SERIAL.lock().unwrap_or_else(PoisonError::into_inner))
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;

    use tree_sitter::{ParseOptions, ParseState, Parser};

    use super::*;

    #[test]
    fn only_grammars_with_global_scanner_state_are_serialized() {
        assert!(serial_parse("properties").is_some());
        assert!(serial_parse("rust").is_none());
    }

    /// Eight threads parsing `.properties` content at once. Unserialized, the
    /// scanners corrupt each other and a parse runs away; the progress callback
    /// cancels one that does, so a regression fails here instead of exhausting
    /// memory.
    #[test]
    fn concurrent_properties_parses_finish() {
        const SOURCE: &str = "distributionBase=GRADLE_USER_HOME\ndistributionPath=wrapper/dists\n\
            distributionUrl=https\\://services.gradle.org/distributions/gradle-8.14.5-bin.zip\n\
            networkTimeout=10000\nzipStoreBase=GRADLE_USER_HOME\nzipStorePath=wrapper/dists\n";
        let language = crate::language("properties").expect("properties grammar");
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let language = language.clone();
                std::thread::spawn(move || {
                    let mut parser = Parser::new();
                    parser.set_language(&language).expect("set language");
                    for _ in 0..200 {
                        let _serial = serial_parse("properties");
                        let mut checks = 0_u32;
                        let mut progress = |_: &ParseState| {
                            checks += 1;
                            if checks > 10_000 {
                                ControlFlow::Break(())
                            } else {
                                ControlFlow::Continue(())
                            }
                        };
                        let tree = parser.parse_with_options(
                            &mut |offset, _| SOURCE.as_bytes().get(offset..).unwrap_or_default(),
                            None,
                            Some(ParseOptions::new().progress_callback(&mut progress)),
                        );
                        let tree = tree.expect("the parse ran away and was cancelled");
                        assert!(!tree.root_node().has_error(), "{}", tree.root_node().to_sexp());
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker panicked");
        }
    }
}
