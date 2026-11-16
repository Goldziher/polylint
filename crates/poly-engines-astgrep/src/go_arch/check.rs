//! Running the compiled Go architecture rules over one parsed file.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ast_grep_core::Node;
use ast_grep_core::tree_sitter::StrDoc;
use poly_engine_api::engine::{Diagnostic, Severity, Span};
use poly_engine_api::host_hooks::go_mod;
use poly_engine_api::rule_config::RuleSelection;

use super::config::{GoArch, Layer};
use super::{FORBIDDEN_CALL, FORBIDDEN_IMPORT, LAYER_VIOLATION};
use crate::language::TslpLanguage;

type GoNode<'r> = Node<'r, StrDoc<TslpLanguage>>;

/// Where a file sits: its path relative to the module root, and the module's import path.
struct Location {
    rel: String,
    module: Option<String>,
}

impl Location {
    fn of(path: &Path) -> Self {
        let Some((root, module)) = go_mod::nearest_root(path) else {
            return Self {
                rel: slashed(poly_engine_api::run_root::relative(path))
                    .trim_start_matches("./")
                    .to_owned(),
                module: None,
            };
        };
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let rel = absolute.strip_prefix(&root).map_or_else(|_| slashed(path), slashed);
        Self {
            rel,
            module: module.module.clone(),
        }
    }

    fn dir(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// One `import_spec` of the file.
struct Import<'r> {
    path: String,
    /// The name the file refers to the package by; `None` for `_` and `.` imports.
    local: Option<String>,
    dot: bool,
    path_node: GoNode<'r>,
    spec: GoNode<'r>,
}

fn collect_imports<'r>(root: &GoNode<'r>) -> Vec<Import<'r>> {
    let mut imports = Vec::new();
    for declaration in root.children().filter(|node| node.kind() == "import_declaration") {
        for spec in declaration.dfs().filter(|node| node.kind() == "import_spec") {
            let Some(path_node) = spec.field("path") else {
                continue;
            };
            let path = path_node.text().trim_matches(['"', '`']).to_owned();
            let alias = spec.field("name");
            let alias_kind = alias.as_ref().map(|node| node.kind().into_owned());
            let dot = alias_kind.as_deref() == Some("dot");
            let local = match (alias_kind.as_deref(), &alias) {
                (Some("dot" | "blank_identifier"), _) => None,
                (_, Some(name)) => Some(name.text().into_owned()),
                (_, None) => Some(default_name(&path)),
            };
            imports.push(Import {
                path,
                local,
                dot,
                path_node,
                spec,
            });
        }
    }
    imports
}

/// The package name an unaliased import is referred to by: the last path element, minus a major
/// version suffix (`.../yaml/v3` is `yaml`, `gopkg.in/yaml.v3` is `yaml`). Real package names can
/// differ from the path; an alias is the way to say so.
fn default_name(path: &str) -> String {
    let mut elements = path.rsplit('/');
    let last = elements.next().unwrap_or(path);
    let is_major = |element: &str| {
        element
            .strip_prefix('v')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    if is_major(last)
        && let Some(previous) = elements.next()
    {
        return previous.to_owned();
    }
    match last.rsplit_once(".v") {
        Some((name, version)) if is_major(&format!("v{version}")) => name.to_owned(),
        _ => last.to_owned(),
    }
}

/// `path` relative to the module root, when it is inside `module`.
fn module_relative<'p>(path: &'p str, module: &str) -> Option<&'p str> {
    let rest = path.strip_prefix(module)?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix('/')
}

fn span_of(node: &GoNode<'_>) -> Span {
    let (start, end) = (node.start_pos(), node.end_pos());
    Span {
        start_line: (start.line() + 1) as u32,
        start_col: (start.column(node) + 1) as u32,
        end_line: (end.line() + 1) as u32,
        end_col: (end.column(node) + 1) as u32,
    }
}

fn diagnostic(code: &str, severity: Severity, title: String, node: &GoNode<'_>) -> Diagnostic {
    Diagnostic {
        engine: "astgrep".to_owned(),
        code: Some(code.to_owned()),
        severity,
        title,
        description: None,
        span: Some(span_of(node)),
        url: None,
        fix: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

/// Whether `[engines.astgrep]` `select` / `extend_select` / `ignore` leave `id` on.
fn enabled(selection: &RuleSelection, id: &str) -> bool {
    let named = |ids: &[String]| ids.iter().any(|candidate| candidate == id);
    !named(&selection.ignore)
        && (selection.select.is_empty() || named(&selection.select) || named(&selection.extend_select))
}

/// `[engines.astgrep.rules.<id>] level`, then the entry's own `level`, then error.
fn severity(selection: &RuleSelection, id: &str, entry: Option<Severity>) -> Severity {
    selection
        .rules
        .get(id)
        .and_then(|options| options.level)
        .or(entry)
        .unwrap_or(Severity::Error)
}

impl GoArch {
    /// The findings of every active rule in `root`, the parse of the file at `path`.
    pub(crate) fn check(&self, root: &GoNode<'_>, path: &Path, selection: &RuleSelection) -> Vec<Diagnostic> {
        let location = Location::of(path);
        let rel = location.rel.as_str();
        let calls: Vec<_> = self
            .calls
            .iter()
            .filter(|rule| enabled(selection, FORBIDDEN_CALL) && rule.scope.applies(rel))
            .collect();
        let forbidden: Vec<_> = self
            .imports
            .iter()
            .filter(|rule| enabled(selection, FORBIDDEN_IMPORT) && rule.scope.applies(rel))
            .collect();
        let layer = self
            .layers
            .iter()
            .find(|layer| layer.dirs.is_match(location.dir()))
            .filter(|layer| enabled(selection, LAYER_VIOLATION) && (layer.include_tests || !rel.ends_with("_test.go")));
        if calls.is_empty() && forbidden.is_empty() && layer.is_none() {
            return Vec::new();
        }

        let imports = collect_imports(root);
        let mut found = Vec::new();

        for import in &imports {
            if let Some(rule) = forbidden.iter().find(|rule| rule.import.is_match(&import.path)) {
                let title = rule.message.clone().unwrap_or_else(|| {
                    format!(
                        "importing `{}` is forbidden here (matches `{}`)",
                        import.path, rule.pattern
                    )
                });
                let level = severity(selection, FORBIDDEN_IMPORT, rule.level);
                found.push(diagnostic(FORBIDDEN_IMPORT, level, title, &import.path_node));
            }
        }

        if !calls.is_empty() {
            for import in imports.iter().filter(|import| import.dot) {
                if let Some(rule) = calls.iter().find(|rule| rule.package.is_match(&import.path)) {
                    let title = format!(
                        "dot import of `{}` hides the selectors forbidden by `{}`; import it by name",
                        import.path, rule.call
                    );
                    found.push(diagnostic(
                        FORBIDDEN_CALL,
                        severity(selection, FORBIDDEN_CALL, rule.level),
                        title,
                        &import.spec,
                    ));
                }
            }
            let by_local: HashMap<&str, &Import<'_>> = imports
                .iter()
                .filter_map(|import| Some((import.local.as_deref()?, import)))
                .collect();
            found.extend(self.forbidden_calls(root, &calls, &by_local, selection));
        }

        if let (Some(layer), Some(module)) = (layer, location.module.as_deref()) {
            for import in &imports {
                if let Some(finding) = self.layer_finding(layer, module, import, selection) {
                    found.push(finding);
                }
            }
        }
        found
    }

    fn forbidden_calls(
        &self,
        root: &GoNode<'_>,
        calls: &[&super::config::CallRule],
        by_local: &HashMap<&str, &Import<'_>>,
        selection: &RuleSelection,
    ) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        for node in root.dfs().filter(|node| node.kind() == "selector_expression") {
            let (Some(operand), Some(field)) = (node.field("operand"), node.field("field")) else {
                continue;
            };
            if operand.kind() != "identifier" {
                continue;
            }
            let Some(import) = by_local.get(&*operand.text()) else {
                continue;
            };
            let name = field.text();
            if let Some(rule) = calls
                .iter()
                .find(|rule| rule.package.is_match(&import.path) && rule.name.is_match(&*name))
            {
                let title = rule.message.clone().unwrap_or_else(|| {
                    format!(
                        "`{}.{name}` is forbidden here (matches `{}`)",
                        operand.text(),
                        rule.call
                    )
                });
                found.push(diagnostic(
                    FORBIDDEN_CALL,
                    severity(selection, FORBIDDEN_CALL, rule.level),
                    title,
                    &node,
                ));
            }
        }
        found
    }

    fn layer_finding(
        &self,
        layer: &Layer,
        module: &str,
        import: &Import<'_>,
        selection: &RuleSelection,
    ) -> Option<Diagnostic> {
        let dir = module_relative(&import.path, module)?;
        let target = self.layers.iter().find(|candidate| candidate.dirs.is_match(dir))?;
        if target.name == layer.name || layer.may_import.contains(&target.name) {
            return None;
        }
        let title = layer.message.clone().unwrap_or_else(|| {
            format!(
                "layer `{}` must not import layer `{}` (`{}`)",
                layer.name, target.name, import.path
            )
        });
        Some(diagnostic(
            LAYER_VIOLATION,
            severity(selection, LAYER_VIOLATION, layer.level),
            title,
            &import.path_node,
        ))
    }
}
