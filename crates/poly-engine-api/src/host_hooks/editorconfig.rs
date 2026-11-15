//! The indentation shfmt takes from `.editorconfig`, resolved by poly.
//!
//! shfmt reads `.editorconfig` only when it is given a filename, and poly pipes stdin and always
//! passes `-i`, so the file would be ignored. poly therefore resolves it for the file being
//! formatted and passes the same `-i` shfmt would have chosen, unless `[fmt.shell.shfmt]` sets
//! `indent_width` or `use_tabs` itself. Checked against shfmt 3.14: `indent_style = tab` indents
//! with tabs; `indent_style = space` uses `indent_size` (8 when it is missing, `tab` or `0`); a
//! file that does not set `indent_style` leaves poly's default alone. Later sections of a file and
//! nearer files win, and `root = true` ends the search. Sections are matched as editorconfig
//! globs relative to their file; `{n..m}` ranges are not supported.
//!
//! It is a host hook ([`HostHook::Editorconfig`](super::HostHook::Editorconfig)): the host-tool engine
//! and the `sh` sidecar both get it, so the file is resolved and the cache salted the same way whichever
//! serves shfmt, and a sidecar (which sees only the bytes) never has to read `.editorconfig` itself.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use globset::{GlobBuilder, GlobMatcher};

use crate::config::EngineConfig;

/// The indentation `.editorconfig` asks shfmt for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// `indent_style = tab`: shfmt's `-i 0`.
    Tabs,
    /// `indent_style = space` with this many columns per level.
    Spaces(usize),
}

struct Section {
    matcher: GlobMatcher,
    properties: Vec<(String, String)>,
}

struct EditorConfig {
    root: bool,
    sections: Vec<Section>,
}

fn section_matcher(header: &str) -> Option<GlobMatcher> {
    let anchored = header.strip_prefix('/');
    let pattern = match anchored {
        Some(rest) => rest.to_owned(),
        None if header.contains('/') => header.to_owned(),
        None => format!("**/{header}"),
    };
    GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher())
}

fn parse(text: &str) -> EditorConfig {
    let mut root = false;
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            sections.extend(current.take());
            current = section_matcher(header).map(|matcher| Section {
                matcher,
                properties: Vec::new(),
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        match current.as_mut() {
            Some(section) => section.properties.push((key, value)),
            None if key == "root" => root = value == "true",
            None => {}
        }
    }
    sections.extend(current);
    EditorConfig { root, sections }
}

type Chain = Arc<Vec<(PathBuf, Arc<EditorConfig>)>>;

#[derive(Default)]
struct Memo {
    parsed: HashMap<PathBuf, Option<Arc<EditorConfig>>>,
    chains: HashMap<PathBuf, Chain>,
}

fn memo() -> &'static RwLock<Memo> {
    static MEMO: OnceLock<RwLock<Memo>> = OnceLock::new();
    MEMO.get_or_init(RwLock::default)
}

/// Drops every memoised `.editorconfig` so the next run rereads them from disk.
pub fn forget() {
    *memo().write().unwrap_or_else(std::sync::PoisonError::into_inner) = Memo::default();
}

/// The `.editorconfig` files that apply to files in `dir`, nearest first, resolved once per directory
/// (sibling files and the salt/apply pair share it).
fn chain_for(dir: &Path) -> Chain {
    if let Some(hit) = memo()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .chains
        .get(dir)
    {
        return Arc::clone(hit);
    }
    let mut chain: Vec<(PathBuf, Arc<EditorConfig>)> = Vec::new();
    let mut guard = memo().write().unwrap_or_else(std::sync::PoisonError::into_inner);
    for ancestor in dir.ancestors() {
        let parsed = match guard.parsed.get(ancestor) {
            Some(known) => known.clone(),
            None => {
                let parsed = std::fs::read_to_string(ancestor.join(".editorconfig"))
                    .ok()
                    .map(|text| Arc::new(parse(&text)));
                guard.parsed.insert(ancestor.to_path_buf(), parsed.clone());
                parsed
            }
        };
        if let Some(config) = parsed {
            let root = config.root;
            chain.push((ancestor.to_path_buf(), config));
            if root {
                break;
            }
        }
    }
    let chain = Arc::new(chain);
    guard.chains.insert(dir.to_path_buf(), Arc::clone(&chain));
    chain
}

/// The indentation `.editorconfig` asks for `path`, when it asks for one.
pub fn indent_for(path: &Path) -> Option<Indent> {
    let absolute: Cow<'_, Path> = if path.is_absolute() && !path.components().any(|c| matches!(c, Component::CurDir)) {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(std::path::absolute(path).ok()?)
    };
    let chain = chain_for(absolute.parent()?);
    let mut style: Option<String> = None;
    let mut size: Option<String> = None;
    for (dir, config) in chain.iter().rev() {
        let Ok(relative) = absolute.strip_prefix(dir) else {
            continue;
        };
        for section in config
            .sections
            .iter()
            .filter(|section| section.matcher.is_match(relative))
        {
            for (key, value) in &section.properties {
                let slot = match key.as_str() {
                    "indent_style" => &mut style,
                    "indent_size" => &mut size,
                    _ => continue,
                };
                *slot = (value != "unset").then(|| value.clone());
            }
        }
    }
    match style.as_deref() {
        Some("tab") => Some(Indent::Tabs),
        Some("space") => Some(Indent::Spaces(
            size.and_then(|size| size.parse().ok())
                .filter(|&size| size > 0)
                .unwrap_or(8),
        )),
        _ => None,
    }
}

/// Whether the table already fixes the indentation, which then wins over `.editorconfig`.
fn sets_indent(cfg: &EngineConfig) -> bool {
    cfg.options.contains_key("indent_width") || cfg.options.contains_key("use_tabs")
}

/// `cfg` with the indentation `.editorconfig` gives `path`.
pub fn apply<'a>(cfg: &'a EngineConfig, path: &Path) -> Cow<'a, EngineConfig> {
    if sets_indent(cfg) {
        return Cow::Borrowed(cfg);
    }
    match indent_for(path) {
        None => Cow::Borrowed(cfg),
        Some(Indent::Tabs) => {
            let mut owned = cfg.clone();
            owned.options.insert("use_tabs".to_owned(), toml::Value::Boolean(true));
            Cow::Owned(owned)
        }
        Some(Indent::Spaces(width)) => {
            let mut owned = cfg.clone();
            owned.indent_width = width;
            Cow::Owned(owned)
        }
    }
}

/// What the formatted output of `path` depends on beyond its bytes: the indentation `.editorconfig`
/// gives it.
pub fn salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    if sets_indent(cfg) {
        return None;
    }
    indent_for(path).map(|indent| format!("editorconfig:{indent:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalDefaults;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = repo.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        repo
    }

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::from_str(options).unwrap(),
        }
    }

    #[test]
    fn space_style_takes_the_indent_size() {
        let repo = repo(&[(
            ".editorconfig",
            "root = true\n[*.sh]\nindent_style = space\nindent_size = 4\n",
        )]);
        let path = repo.path().join("bin/run.sh");
        assert_eq!(indent_for(&path), Some(Indent::Spaces(4)));
        assert_eq!(apply(&cfg(""), &path).indent_width, 4);
    }

    #[test]
    fn tab_style_asks_shfmt_for_tabs_whatever_the_size() {
        let repo = repo(&[(".editorconfig", "[*]\nindent_style = tab\nindent_size = 4\n")]);
        let path = repo.path().join("run.sh");
        assert_eq!(indent_for(&path), Some(Indent::Tabs));
        let base = cfg("");
        let applied = apply(&base, &path);
        assert_eq!(
            applied.options.get("use_tabs").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_size_without_a_style_leaves_the_default_alone() {
        let repo = repo(&[(".editorconfig", "[*.sh]\nindent_size = 3\n")]);
        let path = repo.path().join("run.sh");
        assert_eq!(indent_for(&path), None);
        assert!(matches!(apply(&cfg(""), &path), Cow::Borrowed(_)));
    }

    #[test]
    fn space_style_without_a_usable_size_is_eight_like_shfmt() {
        let repo = repo(&[(".editorconfig", "[*.sh]\nindent_style = space\nindent_size = tab\n")]);
        assert_eq!(indent_for(&repo.path().join("run.sh")), Some(Indent::Spaces(8)));
    }

    #[test]
    fn later_sections_and_nearer_files_win() {
        let repo = repo(&[
            (
                ".editorconfig",
                "root = true\n[*]\nindent_style = space\nindent_size = 4\n[*.sh]\nindent_size = 2\n",
            ),
            ("deep/.editorconfig", "[*.sh]\nindent_size = 6\n"),
        ]);
        assert_eq!(indent_for(&repo.path().join("a.sh")), Some(Indent::Spaces(2)));
        assert_eq!(indent_for(&repo.path().join("deep/b.sh")), Some(Indent::Spaces(6)));
        assert_eq!(indent_for(&repo.path().join("deep/c.py")), Some(Indent::Spaces(4)));
    }

    #[test]
    fn root_true_stops_the_search_and_slash_sections_are_anchored() {
        let repo = repo(&[
            (".editorconfig", "[*]\nindent_style = tab\n"),
            (
                "pkg/.editorconfig",
                "root = true\n[scripts/*.sh]\nindent_style = space\nindent_size = 3\n",
            ),
        ]);
        assert_eq!(
            indent_for(&repo.path().join("pkg/scripts/a.sh")),
            Some(Indent::Spaces(3))
        );
        assert_eq!(indent_for(&repo.path().join("pkg/other/a.sh")), None);
    }

    #[test]
    fn an_explicit_table_wins_and_the_salt_follows_the_resolved_indent() {
        let repo = repo(&[(".editorconfig", "[*.sh]\nindent_style = space\nindent_size = 4\n")]);
        let path = repo.path().join("run.sh");
        assert!(matches!(apply(&cfg("indent_width = 2"), &path), Cow::Borrowed(_)));
        assert!(matches!(apply(&cfg("use_tabs = false"), &path), Cow::Borrowed(_)));
        assert_eq!(salt(&cfg("indent_width = 2"), &path), None);
        assert_eq!(salt(&cfg(""), &path).as_deref(), Some("editorconfig:Spaces(4)"));
    }

    #[test]
    fn the_chain_is_memoised_per_directory_until_forgotten() {
        let repo = repo(&[(".editorconfig", "[*.sh]\nindent_style = tab\n")]);
        let path = repo.path().join("run.sh");
        assert_eq!(indent_for(&path), Some(Indent::Tabs));
        std::fs::write(
            repo.path().join(".editorconfig"),
            "[*.sh]\nindent_style = space\nindent_size = 2\n",
        )
        .unwrap();
        assert_eq!(indent_for(&path), Some(Indent::Tabs));
        forget();
        assert_eq!(indent_for(&path), Some(Indent::Spaces(2)));
    }
}
