//! The declarations the host asks about and how what the checker found credits their members (skif ADR 0010): a read credits
//! the member of the declaration it lands on, and a read of a base class's or an interface's member also credits the
//! member each class that extends or implements it has under that name: its own, or the one it inherits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{DeclId, TypeQuery};
use polylint_tsrs::tsrs_path;

/// A declaration named as oxc names it: its file as tsrs names files and the UTF-8 byte offset of its name in the host's
/// text of the file (past the closing brace of an anonymous default class, [`crate::DeclQuery::name_start`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DeclKey {
    pub(crate) file: String,
    pub(crate) name_start: u32,
}

impl DeclKey {
    pub(crate) fn new(file: &Path, name_start: u32) -> Self {
        Self {
            file: tsrs_path(file),
            name_start,
        }
    }
}

/// A read of `name` that the checker resolved to a member of the declaration `owner`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct MemberRead {
    pub(crate) owner: DeclKey,
    pub(crate) name: String,
}

/// A class that extends or implements the owner of `member` has that member as the member of the asked declaration
/// `provider`: its own, an override, or one it inherits from a base class further up.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Implementation {
    pub(crate) member: MemberRead,
    pub(crate) provider: DeclKey,
}

/// The declarations of a [`TypeQuery`], by key.
#[derive(Debug, Default)]
pub(crate) struct Candidates {
    by_key: crate::hash::HashMap<DeclKey, Vec<DeclId>>,
    decls: Vec<Candidate>,
    names: crate::hash::HashSet<String>,
    files: crate::hash::HashSet<String>,
}

#[derive(Debug)]
struct Candidate {
    members: crate::hash::HashSet<String>,
}

impl Candidates {
    pub(crate) fn new(query: &TypeQuery) -> Self {
        let mut candidates = Self::default();
        for (index, decl) in query.decls.iter().enumerate() {
            let id = DeclId(u32::try_from(index).unwrap_or(u32::MAX));
            candidates
                .by_key
                .entry(DeclKey::new(&decl.file, decl.name_start))
                .or_default()
                .push(id);
            candidates.files.insert(tsrs_path(&decl.file));
            candidates.names.extend(decl.members.iter().cloned());
            candidates.decls.push(Candidate {
                members: decl.members.iter().cloned().collect(),
            });
        }
        candidates
    }

    /// Whether no declaration is asked about.
    pub(crate) fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }

    /// Whether some asked declaration has a member `name`: reads of other names are not sent to the checker.
    pub(crate) fn is_member_name(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Whether some asked declaration is in the file tsrs names `file`.
    pub(crate) fn has_file(&self, file: &str) -> bool {
        self.files.contains(file)
    }

    /// The asked declarations at `key`.
    pub(crate) fn at(&self, key: &DeclKey) -> &[DeclId] {
        self.by_key.get(key).map_or(&[], Vec::as_slice)
    }

    /// Whether some declaration at `key` is asked about.
    pub(crate) fn is_asked(&self, key: &DeclKey) -> bool {
        self.by_key.contains_key(key)
    }

    fn candidate(&self, id: DeclId) -> Option<&Candidate> {
        usize::try_from(id.0).ok().and_then(|index| self.decls.get(index))
    }

    fn has_member(&self, id: DeclId, name: &str) -> bool {
        self.candidate(id)
            .is_some_and(|candidate| candidate.members.contains(name))
    }
}

/// What the walks of every program found, joined across programs.
#[derive(Debug, Default)]
pub(crate) struct Found {
    pub(crate) reads: Vec<MemberRead>,
    pub(crate) whole: Vec<DeclKey>,
    /// The members of base classes and interfaces that classes have as members of asked declarations.
    pub(crate) implementations: BTreeSet<Implementation>,
}

/// The credited members and the declarations used as a whole.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Credits {
    pub(crate) reads: BTreeMap<DeclId, BTreeSet<String>>,
    pub(crate) whole: BTreeSet<DeclId>,
}

/// Credits what `found` holds to the declarations of `candidates`.
pub(crate) fn credit(candidates: &Candidates, found: &Found) -> Credits {
    let mut providers: crate::hash::HashMap<&MemberRead, Vec<&DeclKey>> = crate::hash::HashMap::default();
    for implementation in &found.implementations {
        providers
            .entry(&implementation.member)
            .or_default()
            .push(&implementation.provider);
    }
    let mut credits = Credits::default();
    for read in &found.reads {
        let direct = candidates.at(&read.owner).iter();
        let provided = providers
            .get(read)
            .into_iter()
            .flatten()
            .flat_map(|&provider| candidates.at(provider));
        for &id in direct.chain(provided) {
            if candidates.has_member(id, &read.name) {
                credits.reads.entry(id).or_default().insert(read.name.clone());
            }
        }
    }
    for key in &found.whole {
        credits.whole.extend(candidates.at(key).iter().copied());
    }
    credits
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{DeclKind, DeclQuery};

    fn decl(file: &str, name_start: u32, kind: DeclKind, members: &[&str]) -> DeclQuery {
        DeclQuery {
            file: PathBuf::from(file),
            name_start,
            kind,
            members: members.iter().map(|&member| member.to_owned()).collect(),
        }
    }

    fn key(file: &str, name_start: u32) -> DeclKey {
        DeclKey::new(Path::new(file), name_start)
    }

    fn read(file: &str, name_start: u32, name: &str) -> MemberRead {
        MemberRead {
            owner: key(file, name_start),
            name: name.to_owned(),
        }
    }

    fn implementation(owner: (&str, u32), name: &str, provider: (&str, u32)) -> Implementation {
        Implementation {
            member: read(owner.0, owner.1, name),
            provider: key(provider.0, provider.1),
        }
    }

    /// `A` (0) and `B` (1) both declare `dispose`; `Child` (2) extends `Base` (not asked) and implements `Disposable`.
    fn candidates() -> Candidates {
        Candidates::new(&TypeQuery {
            decls: vec![
                decl("/ws/a.ts", 13, DeclKind::Class, &["dispose", "size"]),
                decl("/ws/b.ts", 13, DeclKind::Class, &["dispose"]),
                decl("/ws/child.ts", 13, DeclKind::Class, &["dispose", "render"]),
                decl("/ws/e.ts", 12, DeclKind::Enum, &["Red", "Green"]),
            ],
        })
    }

    fn reads(credits: &Credits) -> Vec<(u32, Vec<&str>)> {
        credits
            .reads
            .iter()
            .map(|(id, names)| (id.0, names.iter().map(String::as_str).collect()))
            .collect()
    }

    #[test]
    fn a_read_credits_only_the_declaration_it_resolves_to() {
        let found = Found {
            reads: vec![read("/ws/a.ts", 13, "dispose"), read("/ws/a.ts", 13, "dispose")],
            ..Found::default()
        };
        assert_eq!(reads(&credit(&candidates(), &found)), [(0, vec!["dispose"])]);
    }

    #[test]
    fn reads_of_unasked_members_or_declarations_credit_nothing() {
        let found = Found {
            reads: vec![
                read("/ws/b.ts", 13, "size"),
                read("/ws/other.ts", 13, "dispose"),
                read("/ws/a.ts", 14, "dispose"),
            ],
            ..Found::default()
        };
        assert_eq!(credit(&candidates(), &found), Credits::default());
    }

    #[test]
    fn a_read_through_a_base_or_an_interface_credits_the_overriding_members() {
        let found = Found {
            reads: vec![
                read("/ws/base.ts", 13, "render"),
                read("/ws/disposable.ts", 17, "dispose"),
            ],
            implementations: BTreeSet::from([
                implementation(("/ws/base.ts", 13), "render", ("/ws/child.ts", 13)),
                implementation(("/ws/disposable.ts", 17), "dispose", ("/ws/child.ts", 13)),
            ]),
            ..Found::default()
        };
        assert_eq!(reads(&credit(&candidates(), &found)), [(2, vec!["dispose", "render"])]);
    }

    #[test]
    fn a_read_through_an_interface_credits_the_member_a_class_inherits_from_an_asked_base() {
        let found = Found {
            reads: vec![read("/ws/disposable.ts", 17, "dispose")],
            implementations: BTreeSet::from([implementation(("/ws/disposable.ts", 17), "dispose", ("/ws/a.ts", 13))]),
            ..Found::default()
        };
        assert_eq!(reads(&credit(&candidates(), &found)), [(0, vec!["dispose"])]);
    }

    #[test]
    fn an_implementation_credits_only_the_member_read_and_only_when_the_provider_declares_it() {
        let found = Found {
            reads: vec![read("/ws/disposable.ts", 17, "dispose")],
            implementations: BTreeSet::from([
                implementation(("/ws/disposable.ts", 17), "size", ("/ws/a.ts", 13)),
                implementation(("/ws/disposable.ts", 17), "dispose", ("/ws/e.ts", 12)),
                implementation(("/ws/other.ts", 17), "dispose", ("/ws/b.ts", 13)),
            ]),
            ..Found::default()
        };
        assert_eq!(credit(&candidates(), &found), Credits::default());
    }

    #[test]
    fn an_asked_base_class_credits_its_own_member_and_the_overrides() {
        let found = Found {
            reads: vec![read("/ws/a.ts", 13, "dispose")],
            implementations: BTreeSet::from([implementation(("/ws/a.ts", 13), "dispose", ("/ws/child.ts", 13))]),
            ..Found::default()
        };
        assert_eq!(
            reads(&credit(&candidates(), &found)),
            [(0, vec!["dispose"]), (2, vec!["dispose"])]
        );
    }

    #[test]
    fn whole_uses_name_the_asked_declarations() {
        let found = Found {
            whole: vec![key("/ws/e.ts", 12), key("/ws/unknown.ts", 0)],
            ..Found::default()
        };
        assert_eq!(credit(&candidates(), &found).whole, BTreeSet::from([DeclId(3)]));
    }

    #[test]
    fn the_name_prefilter_holds_every_asked_member_name() {
        let candidates = candidates();
        assert!(candidates.is_member_name("dispose") && candidates.is_member_name("Green"));
        assert!(!candidates.is_member_name("length"));
        assert!(candidates.is_asked(&key("/ws/child.ts", 13)) && candidates.is_asked(&key("/ws/e.ts", 12)));
        assert!(!candidates.is_asked(&key("/ws/base.ts", 13)));
        assert!(candidates.has_file(&key("/ws/e.ts", 0).file) && !candidates.has_file(&key("/ws/base.ts", 0).file));
        assert!(Candidates::new(&TypeQuery::default()).is_empty());
    }
}
