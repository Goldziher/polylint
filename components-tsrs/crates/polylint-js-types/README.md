# polylint-js-types

Type-aware facts about the members of JavaScript and TypeScript classes, enums and namespaces, from the TypeScript
checker of [tsrs](https://github.com/oxc-project/tsrs), the Rust port of the TypeScript 7 compiler. A host that finds
unused members from syntax alone asks it which members are read where the project has types. The programs are built
on [`polylint-tsrs`](https://crates.io/crates/polylint-tsrs).

- `analyze` builds a program per selected `tsconfig.json`, queries the checker for the declarations in a `TypeQuery`
  and returns `TypeFacts`: typed member reads, uses of a declaration as a whole, reads it could not settle (credited
  by name), and the type coverage of every package with the reason when a package or file stays untyped.
- `start` builds the programs before the query is known, from the files that may be judged, so they build while the
  host works its query out; `PendingTypes::finish` hands over the judged files and the query and returns the facts
  `analyze` gives for them. A program no judged file needs is freed unwalked.
- Declarations are named by file and the UTF-8 byte offset of their name, as oxc's spans count it.
- Every file and config is read through the host's `FileSystem`: `OsFs` for the disk, `MemoryFs` for a tree in memory,
  or the host's own view, such as an editor's unsaved buffers.

No tsrs type appears in the public API. Programs are built a few at a time (a quarter of the threads, the rest shared
out as checkers; one thread runs tsrs single-threaded), each program's memory is freed as soon as its walk ends, and a
panic inside the checker leaves that program's packages untyped with the reason `checker-failed`. Every file a program
reads is parsed and bound once and shared by every program that reads it: the programs of one analysis, or of every
analysis of a run that shares `TypePrograms`, which also builds only once a program that the judged files of several
nested roots go to. A bundled lib file or a declaration file below `node_modules` is kept until the run's last program
is freed, a project file until the last program that read it is.

## Use

```toml
[dependencies]
polylint-js-types = "0.1"
```

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use polylint_js_types::{DeclId, DeclKind, DeclQuery, MemoryFs, TypeInput, TypePackage, TypeQuery, analyze};

let tsconfig = r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["src"] }"#;
let source = "export class Box { width = 1; height = 2; }\nexport const area = (box: Box) => box.width;\n";
let fs = MemoryFs::new()
    .with_file("/ws/tsconfig.json", tsconfig)
    .with_file("/ws/src/box.ts", source);
let file = PathBuf::from("/ws/src/box.ts");
let packages = [TypePackage { dir: PathBuf::from("/ws"), name: None, files: vec![file.clone()] }];
let query = TypeQuery {
    decls: vec![DeclQuery {
        file,
        // The UTF-8 byte offset of the class's name.
        name_start: 13,
        kind: DeclKind::Class,
        members: vec!["width".to_owned(), "height".to_owned()],
    }],
};

let facts = analyze(&TypeInput::new(Arc::new(fs), Path::new("/ws"), &packages, &query)).expect("the analysis runs");
// `width` is read through the typed parameter; `height` is never read.
let read: Vec<&str> = facts.reads[&DeclId(0)].iter().map(String::as_str).collect();
assert_eq!(read, ["width"]);
```

## tsrs version and features

The crate depends on `polylint-tsrs` and the tsrs crates it pins, held to one exact version (`=7.1.0-beta.tsrs2`).
It reads no environment variable and does no I/O beyond the file system it is given.

The `plain-ptrs` feature, off by default, turns on `plain-ptrs` in `polylint-tsrs` and `tsrs_core`: tsrs then uses
plain pointers instead of compressed ones, which reserve 32 GiB of address space for the process, for hosts such as
Node.js that cannot give up that range. The crate is correct either way; `retire_freed_sources` and the arena
statistics have an effect only with compressed pointers.

## Known defects

The typed run has known false findings: a live member reported unused when it is read through an interface the class
satisfies without `implements`, through a generic constraint, or through a key of literal type. They are listed in
[`KNOWN-DEFECTS.md`](KNOWN-DEFECTS.md), with the measurement they come from.

## Origin

The crate was written as `skif-js-types` in [skif](https://github.com/polylint/skif), polylint's JavaScript and
TypeScript dependency linter, and moved here at skif commit `75500e05` when skif 0.9.0 dropped its type-aware
analysis. skif's ADR 0010 records its design. Both projects are by the same author and under the MIT licence.

## Licence

MIT. See `LICENSE`.
