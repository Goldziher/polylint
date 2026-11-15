//! One tree-sitter parse per file per grammar per lint pass (ADR 0057).
//!
//! The runner opens a scope ([`SharedParses`]) around the engines of one
//! lint pass over one file. Inside it, [`shared_tree`] hands every engine that asks
//! for the same content under the same grammar the tree the first one parsed, so
//! `quality` and `astgrep` stop parsing the file once each. Outside a scope it parses
//! and keeps nothing, which is what an engine running in a component sees: the scope
//! lives in this crate's thread-local, and a component carries its own copy of it.
//!
//! An entry holds a clone of the content's `Arc`, so the allocation a pointer
//! comparison relies on cannot be freed and reused while the entry exists. The tree is
//! stored type-erased and handed back only to a caller asking for the same type, so a
//! tree built by a different `tree-sitter` instance is never mistaken for one.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::sync::Arc;

use tree_sitter_language_pack::Language;

/// One parsed tree, keyed by the content it was parsed from and the grammar.
struct Entry {
    content: Arc<str>,
    language: Language,
    tree: Box<dyn Any>,
}

thread_local! {
    /// The open scope's trees; `None` outside a scope.
    static SCOPE: RefCell<Option<Vec<Entry>>> = const { RefCell::new(None) };
    /// Parses [`shared_tree`] ran on this thread, hits excluded.
    static PARSES: Cell<u64> = const { Cell::new(0) };
}

/// An open parse-sharing scope on this thread; dropping it (also on unwind) drops
/// the trees parsed inside it.
///
/// Opened while another scope is open on the thread, it joins that one and closes
/// nothing. Not `Send`: the scope belongs to the thread that opened it.
#[must_use = "the scope closes when this guard is dropped"]
pub struct SharedParses {
    opened: bool,
    _thread_bound: PhantomData<*const ()>,
}

impl SharedParses {
    /// Open a scope on this thread, or join the one already open.
    pub fn open() -> Self {
        let opened = SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            if scope.is_some() {
                return false;
            }
            *scope = Some(Vec::new());
            true
        });
        Self {
            opened,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for SharedParses {
    fn drop(&mut self) {
        if self.opened {
            // During thread teardown the slot may already be gone; there is nothing to clear.
            let _ = SCOPE.try_with(|scope| *scope.borrow_mut() = None);
        }
    }
}

/// Run `body` with a parse-sharing scope open on this thread (see [`SharedParses`]).
pub fn with_shared_parses<R>(body: impl FnOnce() -> R) -> R {
    let _scope = SharedParses::open();
    body()
}

/// The tree for `content` under `language`: the one already parsed in the open scope,
/// or the result of `parse`, which the scope then keeps.
///
/// `parse` must parse exactly `content` with `language` from scratch, because the tree
/// it returns is handed to every later caller in the scope as if they had parsed it.
/// A `None` from `parse` is returned as is and not remembered.
pub fn shared_tree<T: Clone + 'static>(
    content: &Arc<str>,
    language: &Language,
    parse: impl FnOnce() -> Option<T>,
) -> Option<T> {
    let hit = SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()?
            .iter()
            .filter(|entry| Arc::ptr_eq(&entry.content, content) && entry.language == *language)
            .find_map(|entry| entry.tree.downcast_ref::<T>().cloned())
    });
    if hit.is_some() {
        return hit;
    }
    PARSES.with(|count| count.set(count.get().saturating_add(1)));
    let tree = parse()?;
    SCOPE.with(|scope| {
        if let Some(entries) = scope.borrow_mut().as_mut() {
            entries.push(Entry {
                content: Arc::clone(content),
                language: language.clone(),
                tree: Box::new(tree.clone()),
            });
        }
    });
    Some(tree)
}

/// How many times [`shared_tree`] ran its `parse` closure on this thread.
///
/// A diagnostic for tests and profiling: the difference across a lint pass is the
/// number of parses the linked engines performed for it.
#[must_use]
pub fn parse_count() -> u64 {
    PARSES.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> Language {
        crate::language("rust").expect("rust grammar")
    }

    fn counting(calls: &Cell<u32>, value: u32) -> impl FnOnce() -> Option<u32> + '_ {
        move || {
            calls.set(calls.get() + 1);
            Some(value)
        }
    }

    #[test]
    fn a_scope_parses_each_content_and_grammar_once() {
        let content: Arc<str> = Arc::from("fn main() {}\n");
        let calls = Cell::new(0);
        let before = parse_count();
        with_shared_parses(|| {
            assert_eq!(shared_tree(&content, &rust(), counting(&calls, 7)), Some(7));
            assert_eq!(shared_tree(&content, &rust(), counting(&calls, 8)), Some(7));
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(parse_count() - before, 1);
    }

    #[test]
    fn outside_a_scope_every_call_parses() {
        let content: Arc<str> = Arc::from("fn main() {}\n");
        let calls = Cell::new(0);
        assert_eq!(shared_tree(&content, &rust(), counting(&calls, 1)), Some(1));
        assert_eq!(shared_tree(&content, &rust(), counting(&calls, 2)), Some(2));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn equal_text_in_another_allocation_is_parsed_again() {
        let first: Arc<str> = Arc::from("fn main() {}\n");
        let second: Arc<str> = Arc::from("fn main() {}\n");
        let calls = Cell::new(0);
        with_shared_parses(|| {
            assert_eq!(shared_tree(&first, &rust(), counting(&calls, 1)), Some(1));
            assert_eq!(shared_tree(&second, &rust(), counting(&calls, 2)), Some(2));
        });
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn another_grammar_or_type_is_parsed_again() {
        let content: Arc<str> = Arc::from("x = 1\n");
        let python = crate::language("python").expect("python grammar");
        let calls = Cell::new(0);
        with_shared_parses(|| {
            assert_eq!(shared_tree(&content, &rust(), counting(&calls, 1)), Some(1));
            assert_eq!(shared_tree(&content, &python, counting(&calls, 2)), Some(2));
            let other_type = shared_tree(&content, &rust(), || {
                calls.set(calls.get() + 1);
                Some(3u64)
            });
            assert_eq!(other_type, Some(3));
        });
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn a_failed_parse_is_not_remembered() {
        let content: Arc<str> = Arc::from("fn main() {}\n");
        let calls = Cell::new(0);
        with_shared_parses(|| {
            let failed: Option<u32> = shared_tree(&content, &rust(), || {
                calls.set(calls.get() + 1);
                None
            });
            assert_eq!(failed, None);
            assert_eq!(shared_tree(&content, &rust(), counting(&calls, 5)), Some(5));
        });
        assert_eq!(calls.get(), 2);
    }

    fn engine_failure() {
        panic!("engine failure");
    }

    #[test]
    fn the_scope_closes_on_return_and_on_panic() {
        let content: Arc<str> = Arc::from("fn main() {}\n");
        with_shared_parses(|| shared_tree(&content, &rust(), || Some(1u32)));
        let calls = Cell::new(0);
        assert_eq!(shared_tree(&content, &rust(), counting(&calls, 2)), Some(2));
        assert_eq!(calls.get(), 1, "a closed scope keeps nothing");

        let unwound = std::panic::catch_unwind(|| {
            with_shared_parses(|| {
                let _ = shared_tree(&content, &rust(), || Some(1u32));
                engine_failure();
            });
        });
        assert!(unwound.is_err());
        assert_eq!(shared_tree(&content, &rust(), counting(&calls, 3)), Some(3));
        assert_eq!(calls.get(), 2, "a panic inside the scope still closes it");
    }

    #[test]
    fn a_nested_scope_joins_the_enclosing_one() {
        let content: Arc<str> = Arc::from("fn main() {}\n");
        let calls = Cell::new(0);
        with_shared_parses(|| {
            with_shared_parses(|| shared_tree(&content, &rust(), counting(&calls, 1)));
            assert_eq!(shared_tree(&content, &rust(), counting(&calls, 2)), Some(1));
        });
        assert_eq!(calls.get(), 1);
    }
}
