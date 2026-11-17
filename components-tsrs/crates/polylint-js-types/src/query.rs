//! The walk of one covered file on its checker: the member reads of skif ADR 0010, sent to the checker only for names some
//! asked declaration has as a member, the uses of a declaration as a whole, and the reads the checker cannot settle.

use std::collections::BTreeSet;

use tsrs_ast::{Kind, Node, SourceFile, Symbol, SymbolFlags, SymbolTable};
use tsrs_checker::{Checker, ObjectFlags, Type, TypeFlags};
use tsrs_compiler::Program;
use tsrs_core::{P, tspath};

use crate::members::{Candidates, DeclKey, Implementation, MemberRead};
use polylint_tsrs::BomFiles;

/// What the walks of one program share.
pub(crate) struct QueryContext<'a> {
    pub(crate) candidates: &'a Candidates,
    pub(crate) boms: &'a BomFiles,
    pub(crate) escapes: EscapeNames,
}

impl<'a> QueryContext<'a> {
    /// The context of the walks of `program`.
    pub(crate) fn new(candidates: &'a Candidates, boms: &'a BomFiles, program: &Program) -> Self {
        let escapes = if candidates.is_empty() {
            EscapeNames::default()
        } else {
            EscapeNames::of(program, candidates, boms)
        };
        Self {
            candidates,
            boms,
            escapes,
        }
    }
}

/// The names a property access `x.name` must have for its escape to be sent to the checker: resolving it checks the
/// receiver `x`, most of the time of a query.
///
/// The escape credits nothing unless the symbol the checker resolves `name` to, through its aliases, is a class, enum
/// or namespace with an asked declaration, or a variable [`aliases_a_value`] says may hold one ([`Walker::escape`]),
/// and credits only reach asked declarations ([`crate::members::credit`]). A property access resolves to a property of
/// the receiver's type under its own name, and the only such properties that are aliases, containers or variables are
/// symbols of the tables of the program: the exports of its modules and namespaces and its globals. So every name that
/// can matter is in this set:
///
/// - `default`, the name of a default export;
/// - the name of every symbol of those tables that has an asked declaration (`export default class C` declares `C`
///   and `default`);
/// - the name of every variable of those tables that may alias a value;
/// - the name of every alias whose declaration does not name its target (a namespace import, `import x = require()`,
///   `export * as x`, `export as namespace x`, a `CommonJS` export or `require` in `JavaScript`);
/// - and, closed over, the name of every alias whose declaration names a target in the set: an import or export
///   specifier (`export { C as D }`), an `import x = A.C` and a default import. An alias resolves through the symbol
///   of the name it targets, so a chain that ends at a credited symbol has every name in the set.
///
/// A name outside the set resolves to a symbol whose escape credits nothing, and skipping the checker for it changes no
/// fact. The tables of every file of the program are walked, not only the judged files', because any receiver's type
/// may be a module. A program with a file left unbound gets no filter.
#[derive(Debug, Default)]
pub(crate) struct EscapeNames(Option<crate::hash::HashSet<&'static str>>);

impl EscapeNames {
    fn of(program: &Program, candidates: &Candidates, boms: &BomFiles) -> Self {
        let files = program.get_source_files();
        if files.iter().any(|file| !file.is_bound()) {
            return Self(None);
        }
        let mut collector = EscapeCollector {
            candidates,
            boms,
            names: crate::hash::HashSet::default(),
            targets: crate::hash::HashMap::default(),
            seen: crate::hash::HashSet::default(),
            tables: Vec::new(),
        };
        collector.names.insert(tsrs_ast::INTERNAL_SYMBOL_NAME_DEFAULT);
        for &file in files {
            collector.tables.extend(file.locals());
            collector
                .tables
                .extend(file.symbol().and_then(|symbol| symbol.exports()));
            collector.tables.extend(file.global_exports());
        }
        collector.walk();
        Self(Some(collector.close()))
    }

    /// Whether the escape of a property access of `name` may credit an asked declaration.
    fn may_reach(&self, name: &str) -> bool {
        self.0.as_ref().is_none_or(|names| names.contains(name))
    }
}

/// The walk of the symbol tables behind [`EscapeNames`].
struct EscapeCollector<'a> {
    candidates: &'a Candidates,
    boms: &'a BomFiles,
    names: crate::hash::HashSet<&'static str>,
    /// The aliases by the name of the target their declaration names.
    targets: crate::hash::HashMap<&'static str, Vec<&'static str>>,
    seen: crate::hash::HashSet<P<Symbol>>,
    tables: Vec<P<SymbolTable>>,
}

impl EscapeCollector<'_> {
    fn walk(&mut self) {
        while let Some(table) = self.tables.pop() {
            for symbol in table.values() {
                if self.seen.insert(symbol) {
                    self.symbol(symbol);
                }
            }
        }
    }

    fn symbol(&mut self, symbol: P<Symbol>) {
        let name = symbol.name();
        let flags = symbol.flags();
        if flags.intersects(SymbolFlags::Alias) {
            let declarations = symbol.declarations();
            if declarations.is_empty() {
                self.names.insert(name);
            }
            for &declaration in declarations {
                match alias_target(declaration) {
                    Some(target) => self.targets.entry(target).or_default().push(name),
                    None => {
                        self.names.insert(name);
                    }
                }
            }
        }
        if (flags.intersects(SymbolFlags::Variable) && aliases_a_value(symbol)) || self.declares_asked(symbol) {
            self.names.insert(name);
        }
        // Namespaces and their merges, ambient modules and global augmentations. Function bodies hold no table walked:
        // no receiver's type has their declarations as properties.
        if flags.intersects(SymbolFlags::Module) {
            self.tables.extend(symbol.exports());
            for &declaration in symbol.declarations() {
                if declaration.kind() == Kind::ModuleDeclaration {
                    self.tables.extend(declaration.locals());
                }
            }
        }
    }

    fn declares_asked(&self, symbol: P<Symbol>) -> bool {
        symbol.declarations().iter().any(|&declaration| {
            tsrs_ast::get_source_file_of_node(declaration)
                .is_some_and(|file| self.candidates.has_file(file.file_name()))
                && key_of(declaration, self.boms).is_some_and(|key| self.candidates.is_asked(&key))
        })
    }

    /// The names in the set and the aliases that reach them, transitively.
    fn close(mut self) -> crate::hash::HashSet<&'static str> {
        let mut queue: Vec<&'static str> = self.names.iter().copied().collect();
        while let Some(name) = queue.pop() {
            for alias in self.targets.remove(name).unwrap_or_default() {
                if self.names.insert(alias) {
                    queue.push(alias);
                }
            }
        }
        self.names
    }
}

/// The name an alias declaration gives its target by, where it gives one.
fn alias_target(declaration: P<Node>) -> Option<&'static str> {
    match declaration.kind() {
        Kind::ImportSpecifier | Kind::ExportSpecifier => declaration.property_name_or_name().map(|name| name.text()),
        Kind::ImportClause => Some(tsrs_ast::INTERNAL_SYMBOL_NAME_DEFAULT),
        Kind::ImportEqualsDeclaration => {
            let reference = declaration.as_import_equals_declaration().module_reference();
            match reference.kind() {
                Kind::Identifier => Some(reference.text()),
                Kind::QualifiedName => Some(reference.as_qualified_name().right.text()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// What the walk of one file found, as owned data.
#[derive(Debug, Default)]
pub(crate) struct FileFacts {
    pub(crate) reads: Vec<MemberRead>,
    pub(crate) whole: Vec<DeclKey>,
    /// The names of reads the checker could not settle.
    pub(crate) unresolved: BTreeSet<String>,
    /// The members of base classes and interfaces that the classes declared in the file have as members of asked
    /// declarations.
    pub(crate) implementations: BTreeSet<Implementation>,
}

/// Walks `file` with `checker`.
pub(crate) fn query_file(checker: &mut Checker, file: P<SourceFile>, context: &QueryContext<'_>) -> FileFacts {
    if context.candidates.is_empty() {
        return FileFacts::default();
    }
    let mut walker = Walker {
        checker,
        context,
        facts: FileFacts::default(),
    };
    file.as_node().for_each_child(&mut |child| {
        walker.visit(child);
        false
    });
    walker.facts
}

/// The package imports of `file` that resolve to no module and that no ambient `declare module` provides: mostly
/// dependencies that are not installed. A relative import that does not resolve is a broken path or an asset, not
/// missing type information, and is not counted.
pub(crate) fn unresolved_modules(checker: &mut Checker, program: &Program, file: P<SourceFile>) -> u32 {
    let Some(resolutions) = program.get_resolved_modules().get(file.path()) else {
        return 0;
    };
    let mut unresolved = 0_u32;
    for (key, module) in resolutions {
        if module.is_resolved() || tspath::is_external_module_name_relative(key.name) {
            continue;
        }
        if checker.try_find_ambient_module_exported(key.name).is_none() {
            unresolved = unresolved.saturating_add(1);
        }
    }
    unresolved
}

/// The UTF-8 byte offset in the host's text of the token at tsrs position `pos` of `text` (tsrs's text of the file):
/// past the trivia before it, and past the byte order mark tsrs dropped and the host keeps.
pub(crate) fn host_offset(text: &str, pos: u32, bom: u32) -> u32 {
    tsrs_scanner::skip_trivia(text, pos).saturating_add(bom)
}

/// The value declarations whose members a read may reach.
const CONTAINER: SymbolFlags = SymbolFlags::Class
    .union(SymbolFlags::Enum)
    .union(SymbolFlags::ValueModule);

struct Walker<'c, 'a> {
    checker: &'c mut Checker,
    context: &'a QueryContext<'a>,
    facts: FileFacts,
}

impl Walker<'_, '_> {
    fn visit(&mut self, node: P<Node>) {
        match node.kind() {
            Kind::PropertyAccessExpression => self.property_access(node),
            Kind::ElementAccessExpression => self.element_access(node),
            Kind::QualifiedName => self.qualified_name(node),
            Kind::BindingElement => self.binding_element(node),
            Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment => self.object_literal_property(node),
            Kind::IndexedAccessType => self.indexed_access_type(node),
            Kind::BinaryExpression => self.binary(node),
            Kind::TypeOperator => self.type_operator(node),
            Kind::CallExpression | Kind::NewExpression => {
                for &argument in node.arguments() {
                    self.escape(argument);
                }
            }
            Kind::SpreadElement | Kind::SpreadAssignment | Kind::ReturnStatement | Kind::ForInStatement => {
                if let Some(expression) = node.expression() {
                    self.escape(expression);
                }
            }
            Kind::ArrayLiteralExpression => {
                for &element in node.elements() {
                    self.escape(element);
                }
            }
            Kind::ClassDeclaration | Kind::ClassExpression => self.class(node),
            _ => {}
        }
        node.for_each_child(&mut |child| {
            self.visit(child);
            false
        });
    }

    // Reads.

    fn property_access(&mut self, node: P<Node>) {
        let name = node.as_property_access_expression().name();
        if !matches!(name.kind(), Kind::Identifier | Kind::PrivateIdentifier) {
            return;
        }
        self.read_at(name, name.text(), tsrs_ast::is_write_only_access(node));
    }

    fn element_access(&mut self, node: P<Node>) {
        let access = node.as_element_access_expression();
        let argument = access.argument_expression;
        if tsrs_ast::is_string_or_numeric_literal_like(argument) {
            self.read_at(argument, argument.text(), tsrs_ast::is_write_only_access(node));
        } else {
            // `E[key]`: any member may be read.
            self.escape(access.expression);
        }
    }

    fn qualified_name(&mut self, node: P<Node>) {
        let right = node.as_qualified_name().right;
        if tsrs_ast::is_identifier(right) {
            self.read_at(right, right.text(), false);
        }
    }

    fn binding_element(&mut self, node: P<Node>) {
        let Some(pattern) = node
            .parent()
            .filter(|&parent| tsrs_ast::is_object_binding_pattern(parent))
        else {
            return;
        };
        let element = node.as_binding_element();
        if element.dot_dot_dot_token().is_some() {
            return;
        }
        match element.property_name() {
            // `{ m: local }`: the checker resolves the property name to the property.
            Some(property) if tsrs_ast::is_identifier(property) => self.read_at(property, property.text(), false),
            Some(property) if tsrs_ast::is_string_or_numeric_literal_like(property) => {
                self.read_of_pattern(pattern, property.text());
            }
            Some(_) => {}
            // `{ m }`: the name declares the local, so the property is looked up on the pattern's type.
            None => {
                if let Some(name) = element.name().filter(|&name| tsrs_ast::is_identifier(name)) {
                    self.read_of_pattern(pattern, name.text());
                }
            }
        }
    }

    fn read_of_pattern(&mut self, pattern: P<Node>, name: &str) {
        if !self.context.candidates.is_member_name(name) {
            return;
        }
        let pattern_type = self.checker.get_type_at_location(pattern);
        self.read_of_type(pattern_type, name);
    }

    /// A property of an object literal: a read in a destructuring assignment (`({ m } = x)`), else the value
    /// escapes (`{ e: E }`, `{ E }`).
    fn object_literal_property(&mut self, node: P<Node>) {
        let Some(literal) = node.parent() else {
            return;
        };
        if tsrs_ast::is_array_literal_or_object_literal_destructuring_pattern(Some(literal)) {
            let Some(name) = node.name() else {
                return;
            };
            if !(tsrs_ast::is_identifier(name) || tsrs_ast::is_string_or_numeric_literal_like(name)) {
                return;
            }
            let text = name.text();
            if !self.context.candidates.is_member_name(text) {
                return;
            }
            let symbol = self.checker.get_property_symbol_of_destructuring_assignment(name);
            self.record(symbol, text, false);
            return;
        }
        if node.kind() == Kind::ShorthandPropertyAssignment {
            let symbol = self.checker.get_shorthand_assignment_value_symbol(Some(node));
            if let Some(symbol) = symbol {
                self.escape_symbol(symbol);
            }
        } else if let Some(initializer) = node.initializer() {
            self.escape(initializer);
        }
    }

    /// `C['m']`.
    fn indexed_access_type(&mut self, node: P<Node>) {
        let access = node.as_indexed_access_type_node();
        let index = access.index_type();
        if index.kind() != Kind::LiteralType {
            return;
        }
        let literal = index.as_literal_type_node().literal();
        if !tsrs_ast::is_string_or_numeric_literal_like(literal) {
            return;
        }
        let name = literal.text();
        if !self.context.candidates.is_member_name(name) {
            return;
        }
        let object = self.checker.get_type_from_type_node_exported(access.object_type());
        self.read_of_type(object, name);
    }

    /// `'m' in x` reads `m`; `x = E` lets `E` escape.
    fn binary(&mut self, node: P<Node>) {
        let binary = node.as_binary_expression();
        match binary.operator_token().kind() {
            Kind::InKeyword => {
                let left = binary.left();
                if !tsrs_ast::is_string_literal_like(left) || !self.context.candidates.is_member_name(left.text()) {
                    return;
                }
                let right = self.checker.get_type_at_location(binary.right());
                self.read_of_type(right, left.text());
            }
            Kind::EqualsToken => self.escape(binary.right()),
            _ => {}
        }
    }

    /// The read of `name` at the name node `at`.
    fn read_at(&mut self, at: P<Node>, name: &str, write_only: bool) {
        if !self.context.candidates.is_member_name(name) {
            return;
        }
        let symbol = self.checker.get_symbol_at_location_exported(at);
        self.record(symbol, name, write_only);
    }

    /// The read of `name` on a value of `object`: on every member of a union, by name where a member is untyped.
    fn read_of_type(&mut self, object: P<Type>, name: &str) {
        let object = self.checker.get_non_nullable_type(object);
        for part in constituents(object) {
            if part.flags().intersects(TypeFlags::AnyOrUnknown) {
                self.facts.unresolved.insert(name.to_owned());
                continue;
            }
            if let Some(property) = self.checker.get_property_of_type_exported(part, name) {
                self.record(Some(property), name, false);
            }
        }
    }

    /// Credits the members `symbol` lands on through its root symbols (union, intersection, mapped and synthetic
    /// properties). A plain assignment is no read, except of a setter. A read the checker cannot settle is credited
    /// by name.
    fn record(&mut self, symbol: Option<P<Symbol>>, name: &str, write_only: bool) {
        let Some(symbol) = symbol.filter(|&symbol| symbol != self.checker.get_unknown_symbol()) else {
            if !write_only {
                self.facts.unresolved.insert(name.to_owned());
            }
            return;
        };
        for root in self.checker.get_root_symbols(symbol) {
            let root = self.resolve_alias(root);
            if write_only && !root.flags().intersects(SymbolFlags::SetAccessor) {
                continue;
            }
            for owner in self.parent_keys(root) {
                self.facts.reads.push(MemberRead {
                    owner,
                    name: name.to_owned(),
                });
            }
        }
    }

    /// The declarations that own the member `symbol` lands on, through its root symbols.
    fn owners(&mut self, symbol: P<Symbol>) -> Vec<DeclKey> {
        let mut owners = Vec::new();
        for root in self.checker.get_root_symbols(symbol) {
            let root = self.resolve_alias(root);
            owners.extend(self.parent_keys(root));
        }
        owners
    }

    /// The keys of the declarations of the symbol that declares the member `member`.
    fn parent_keys(&self, member: P<Symbol>) -> Vec<DeclKey> {
        member.parent().map_or_else(Vec::new, |parent| {
            parent
                .declarations()
                .iter()
                .filter_map(|&declaration| self.key_of(declaration))
                .collect()
        })
    }

    // Whole uses.

    /// `keyof C`, `keyof typeof E`.
    fn type_operator(&mut self, node: P<Node>) {
        let operator = node.as_type_operator_node();
        if operator.operator() != Kind::KeyOfKeyword {
            return;
        }
        let operand = self.checker.get_type_from_type_node_exported(operator.type_());
        for part in constituents(operand) {
            if let Some(symbol) = part.symbol().filter(|symbol| symbol.flags().intersects(CONTAINER)) {
                self.whole(symbol);
            }
        }
    }

    /// `expression` is passed on where any of its members may be read: a class, enum or namespace, or a variable
    /// that aliases one (`const X = E; f(X)`, `function f(e: typeof E) { g(e) }`).
    fn escape(&mut self, expression: P<Node>) {
        let expression = skip_outer_expressions(expression);
        let name = match expression.kind() {
            Kind::Identifier => expression,
            Kind::PropertyAccessExpression => {
                let name = expression.as_property_access_expression().name();
                if !self.context.escapes.may_reach(name.text()) {
                    return;
                }
                #[cfg(test)]
                tests::record_escape_lookup(name);
                name
            }
            _ => return,
        };
        if let Some(symbol) = self.checker.get_symbol_at_location_exported(name) {
            self.escape_symbol(symbol);
        }
    }

    fn escape_symbol(&mut self, symbol: P<Symbol>) {
        let symbol = self.resolve_alias(symbol);
        if symbol.flags().intersects(CONTAINER) {
            self.whole(symbol);
            return;
        }
        if !symbol.flags().intersects(SymbolFlags::Variable) || !aliases_a_value(symbol) {
            return;
        }
        let value = self.checker.get_type_of_symbol_exported(symbol);
        for part in constituents(value) {
            // The type of a class, enum or namespace value is anonymous; an instance of a class is not.
            if !part.flags().intersects(TypeFlags::Object) || !part.object_flags().intersects(ObjectFlags::Anonymous) {
                continue;
            }
            if let Some(target) = part.symbol().filter(|target| target.flags().intersects(CONTAINER)) {
                self.whole(target);
            }
        }
    }

    fn whole(&mut self, symbol: P<Symbol>) {
        for &declaration in symbol.declarations() {
            if let Some(key) = self.key_of(declaration) {
                self.facts.whole.push(key);
            }
        }
    }

    // Base classes and interfaces.

    /// For every member of a base class or an interface of the class `node` with a name some asked declaration has,
    /// the asked declaration whose member the class has under that name: its own, an override, or one it inherits
    /// from a base class further up. Every class counts, asked or not: one that declares no judged member still
    /// hands a read through its interface to the member it inherits. Instance members pair with the instance side of
    /// the class, static members of a base class with its static side.
    ///
    /// A class none of whose declarations (its own and the interfaces merged into it) has an `extends` or
    /// `implements` clause has no ancestors: the checker takes base types only from those clauses, so it is not asked
    /// for them.
    fn class(&mut self, node: P<Node>) {
        let Some(class) = self.checker.get_symbol_of_declaration_exported(node) else {
            return;
        };
        if !class
            .declarations()
            .iter()
            .any(|&declaration| has_heritage(declaration))
        {
            return;
        }
        let ancestors = self.ancestors(class);
        if ancestors.is_empty() {
            return;
        }
        let instance = self.checker.get_declared_type_of_symbol_exported(class);
        let statics = self.checker.get_type_of_symbol_exported(class);
        for ancestor in ancestors {
            let declared = self.checker.get_declared_type_of_symbol_exported(ancestor);
            self.implement(declared, instance);
            if ancestor.flags().intersects(SymbolFlags::Class) {
                let constructor = self.checker.get_type_of_symbol_exported(ancestor);
                self.implement(constructor, statics);
            }
        }
    }

    /// Records, for each member of `ancestor` with an asked name, the asked declarations that own the member of the
    /// same name on `class` when they are not the member's own.
    fn implement(&mut self, ancestor: P<Type>, class: P<Type>) {
        for &member in self.checker.get_properties_of_type_exported(ancestor) {
            let name = member.name();
            if !self.context.candidates.is_member_name(name) {
                continue;
            }
            let Some(provided) = self.checker.get_property_of_type_exported(class, name) else {
                continue;
            };
            let mut providers = self.owners(provided);
            providers.retain(|provider| self.context.candidates.is_asked(provider));
            if providers.is_empty() {
                continue;
            }
            for owner in self.owners(member) {
                for provider in providers.iter().filter(|&provider| *provider != owner) {
                    self.facts.implementations.insert(Implementation {
                        member: MemberRead {
                            owner: owner.clone(),
                            name: name.to_owned(),
                        },
                        provider: provider.clone(),
                    });
                }
            }
        }
    }

    /// The classes and interfaces `class` extends or implements, transitively.
    fn ancestors(&mut self, class: P<Symbol>) -> Vec<P<Symbol>> {
        let mut seen = Vec::new();
        let mut queue = vec![class];
        while let Some(symbol) = queue.pop() {
            if seen.contains(&symbol) {
                continue;
            }
            seen.push(symbol);
            if symbol.flags().intersects(SymbolFlags::Class | SymbolFlags::Interface) {
                let declared = self.checker.get_declared_type_of_symbol_exported(symbol);
                for &base in self.checker.get_base_types_exported(declared) {
                    queue.extend(base.symbol());
                }
            }
            for &declaration in symbol.declarations() {
                queue.extend(self.implemented(declaration));
            }
        }
        seen.retain(|&symbol| symbol != class);
        seen
    }

    /// The interfaces (or classes) in the `implements` clause of `declaration`.
    fn implemented(&mut self, declaration: P<Node>) -> Vec<P<Symbol>> {
        if !tsrs_ast::is_class_like(declaration) {
            return Vec::new();
        }
        let Some(clause) = tsrs_ast::get_heritage_clause(declaration, Kind::ImplementsKeyword) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        for &implemented in clause.as_heritage_clause().types().nodes() {
            // A type reference or an expression with type arguments; the checker resolves its name, qualified or not.
            let name = tsrs_ast::get_heritage_clause_element_name(implemented);
            if let Some(symbol) = self.checker.get_symbol_at_location_exported(name) {
                symbols.push(self.resolve_alias(symbol));
            }
        }
        symbols
    }

    // Keys.

    fn resolve_alias(&mut self, symbol: P<Symbol>) -> P<Symbol> {
        if symbol.flags().intersects(SymbolFlags::Alias) {
            self.checker.get_aliased_symbol(symbol)
        } else {
            symbol
        }
    }

    fn key_of(&self, declaration: P<Node>) -> Option<DeclKey> {
        key_of(declaration, self.context.boms)
    }
}

/// The key of a class, enum or namespace declaration: its file and the offset of its name in the host's text, or for an
/// anonymous `export default class {}`, which has no name, the offset just past its closing brace.
fn key_of(declaration: P<Node>, boms: &BomFiles) -> Option<DeclKey> {
    let name = declaration_name(declaration);
    if name.is_none() && declaration.kind() != Kind::ClassDeclaration {
        return None;
    }
    let file = tsrs_ast::get_source_file_of_node(declaration)?;
    let file_name = file.file_name();
    let bom = boms.offset(file_name);
    let name_start = match name {
        Some(name) => host_offset(file.text(), name.pos().source_offset()?, bom),
        None => declaration.end().source_offset()?.saturating_add(bom),
    };
    Some(DeclKey {
        file: file_name.to_owned(),
        name_start,
    })
}

/// The name a declaration is known by: its own, or for `const C = class {}` the variable's.
fn declaration_name(declaration: P<Node>) -> Option<P<Node>> {
    if let Some(name) = declaration.name() {
        return Some(name);
    }
    if !tsrs_ast::is_class_like(declaration) {
        return None;
    }
    declaration
        .parent()
        .filter(|parent| parent.kind() == Kind::VariableDeclaration)
        .and_then(|parent| parent.name())
}

/// Whether the class or interface `declaration` extends or implements anything.
fn has_heritage(declaration: P<Node>) -> bool {
    tsrs_ast::get_heritage_clause(declaration, Kind::ExtendsKeyword).is_some()
        || tsrs_ast::get_heritage_clause(declaration, Kind::ImplementsKeyword).is_some()
}

/// Whether the variable `symbol` may hold a class, enum or namespace itself: it is initialized from a name or
/// declared with a `typeof` type.
fn aliases_a_value(symbol: P<Symbol>) -> bool {
    let Some(declaration) = symbol.value_declaration() else {
        return false;
    };
    if !matches!(declaration.kind(), Kind::VariableDeclaration | Kind::Parameter) {
        return false;
    }
    let initialized_from_name = declaration.initializer().is_some_and(|initializer| {
        matches!(
            skip_outer_expressions(initializer).kind(),
            Kind::Identifier | Kind::PropertyAccessExpression
        )
    });
    initialized_from_name
        || declaration
            .type_node()
            .is_some_and(|annotation| annotation.kind() == Kind::TypeQuery)
}

/// `expression` without parentheses, type assertions and non-null assertions around it.
fn skip_outer_expressions(mut expression: P<Node>) -> P<Node> {
    while matches!(
        expression.kind(),
        Kind::ParenthesizedExpression
            | Kind::AsExpression
            | Kind::SatisfiesExpression
            | Kind::NonNullExpression
            | Kind::TypeAssertionExpression
    ) {
        let Some(inner) = expression.expression() else {
            break;
        };
        expression = inner;
    }
    expression
}

/// The members of a union, or the type itself.
fn constituents(t: P<Type>) -> Vec<P<Type>> {
    if t.flags().intersects(TypeFlags::Union) {
        t.types().to_vec()
    } else {
        vec![t]
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{DeclId, DeclKind, DeclQuery, FileSystem, MemoryFs, TypeInput, TypePackage, TypeQuery};

    /// The `(file, name)` of every property-access escape sent to the checker, by every test of the crate; each test
    /// reads only its own files.
    static ESCAPE_LOOKUPS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    pub(super) fn record_escape_lookup(name: P<Node>) {
        if let Some(file) = tsrs_ast::get_source_file_of_node(name) {
            ESCAPE_LOOKUPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((file.file_name().to_owned(), name.text().to_owned()));
        }
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn only_escapes_that_can_reach_an_asked_declaration_ask_the_checker() {
        let root = PathBuf::from("/escape-filter");
        let colors = "export enum Color {\n  Red,\n}\n";
        let files = [
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["src"] }"#,
            ),
            ("src/colors.ts", colors),
            ("src/barrel.ts", "export { Color as Hue } from './colors';\n"),
            (
                "src/main.ts",
                "import * as barrel from './barrel';\n\ndeclare function register(value: object): void;\ndeclare const options: { verbose: object };\nregister(options.verbose);\nregister(barrel.Hue);\n",
            ),
        ];
        let fs: Arc<dyn FileSystem> = Arc::new(
            files
                .iter()
                .fold(MemoryFs::new(), |fs, (rel, text)| fs.with_file(root.join(rel), text)),
        );
        let query = TypeQuery {
            decls: vec![DeclQuery {
                file: root.join("src/colors.ts"),
                name_start: u32::try_from(colors.find("Color").unwrap()).unwrap(),
                kind: DeclKind::Enum,
                members: vec!["Red".to_owned()],
            }],
        };
        let packages = [TypePackage {
            dir: root.clone(),
            name: Some("app".to_owned()),
            files: vec![root.join("src/colors.ts"), root.join("src/main.ts")],
        }];
        let facts = crate::analyze(&TypeInput::new(fs, &root, &packages, &query)).expect("the analysis fails");
        assert_eq!(facts.whole.iter().copied().collect::<Vec<_>>(), [DeclId(0)]);
        let main = tsrs_path_of(&root.join("src/main.ts"));
        let looked_up: Vec<String> = ESCAPE_LOOKUPS
            .lock()
            .unwrap()
            .iter()
            .filter(|(file, _)| *file == main)
            .map(|(_, name)| name.clone())
            .collect();
        assert_eq!(looked_up, ["Hue"]);
    }

    fn tsrs_path_of(path: &Path) -> String {
        polylint_tsrs::tsrs_path(path)
    }

    /// tsrs's position of the first token of `host_text` after `host_prefix`, in tsrs's text (`host_text` without its
    /// byte order mark), as the parser gives it: the end of the previous token, before the trivia.
    fn join(host_text: &str, before_trivia: &str) -> u32 {
        let (text, bom) = match host_text.strip_prefix('\u{feff}') {
            Some(rest) => (rest, 3),
            None => (host_text, 0),
        };
        let pos = u32::try_from(text.find(before_trivia).unwrap_or(0) + before_trivia.len()).unwrap_or(0);
        host_offset(text, pos, bom)
    }

    fn oxc_offset(host_text: &str, name: &str) -> u32 {
        u32::try_from(host_text.find(name).unwrap_or(usize::MAX)).unwrap_or(u32::MAX)
    }

    #[test]
    fn offsets_skip_the_trivia_before_the_name() {
        let text = "export class /* the shape */\n  Shape {}";
        assert_eq!(join(text, "class"), oxc_offset(text, "Shape"));
    }

    #[test]
    fn offsets_count_utf8_bytes_of_non_ascii_text_before_the_name() {
        let text = "// größe → 大きさ 😀\nexport enum Größe { Klein }";
        let name = oxc_offset(text, "Größe");
        assert_eq!(join(text, "enum"), name);
        // UTF-16 would count fewer units: the join is meaningful only because the two differ here.
        let utf16 = text
            .split_at(usize::try_from(name).unwrap_or(0))
            .0
            .encode_utf16()
            .count();
        assert_ne!(usize::try_from(name).unwrap_or(0), utf16);
    }

    #[test]
    fn offsets_add_the_byte_order_mark_tsrs_drops() {
        let text = "\u{feff}/** é */\nexport namespace Ns {}";
        assert_eq!(join(text, "namespace"), oxc_offset(text, "Ns"));
        assert_eq!(
            oxc_offset(text, "Ns"),
            oxc_offset(text.trim_start_matches('\u{feff}'), "Ns") + 3
        );
    }
}
