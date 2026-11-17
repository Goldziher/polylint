use std::fs;
use std::path::{Path, PathBuf};

use super::{PackageRoots, module_tree, module_tree_at, package_root, walk};

fn touch(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "").unwrap();
}

fn digest_of(root: &Path) -> String {
    let mut entries = walk(root);
    entries.sort_unstable();
    entries.join("\n")
}

#[test]
fn the_package_root_follows_init_files_up_to_the_first_gap() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("proj/pkg");
    touch(&pkg.join("__init__.py"));
    touch(&pkg.join("sub/__init__.py"));
    touch(&pkg.join("sub/mod.py"));

    assert_eq!(package_root(&pkg.join("sub/mod.py"), dir.path()), "proj/pkg");
    assert_eq!(
        package_root(&pkg.join("sub/mod.py"), Path::new("/elsewhere")),
        pkg.to_string_lossy()
    );
}

#[test]
fn a_directory_without_an_init_file_has_no_package_root() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("loose/mod.py"));

    assert_eq!(package_root(&dir.path().join("loose/mod.py"), dir.path()), "-");
}

#[test]
fn adding_or_removing_a_sibling_init_file_changes_the_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("pkg/mod.py");
    touch(&module);
    let before = package_root(&module, dir.path());

    touch(&dir.path().join("pkg/__init__.py"));
    let during = package_root(&module, dir.path());
    fs::remove_file(dir.path().join("pkg/__init__.py")).unwrap();

    assert_ne!(before, during);
    assert_eq!(before, package_root(&module, dir.path()));
}

#[test]
fn an_init_file_in_a_parent_directory_moves_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("outer/inner/mod.py");
    touch(&module);
    touch(&dir.path().join("outer/inner/__init__.py"));
    let inner_only = package_root(&module, dir.path());

    touch(&dir.path().join("outer/__init__.py"));

    assert_ne!(inner_only, package_root(&module, dir.path()));
}

#[test]
fn remembered_package_roots_hold_until_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    touch(&pkg.join("mod.py"));
    let roots = PackageRoots::default();
    assert_eq!(roots.get(&pkg), None);

    touch(&pkg.join("__init__.py"));
    assert_eq!(roots.get(&pkg), None, "the answer is remembered, not recomputed");

    roots.clear();
    assert_eq!(roots.get(&pkg), Some(pkg.clone()));
}

#[test]
fn the_module_tree_changes_when_a_module_is_added_removed_or_renamed() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("a/x.py"));
    let src = [PathBuf::from(dir.path())];
    let before = module_tree(&src);

    touch(&dir.path().join("a/y.py"));
    let added = module_tree(&src);
    fs::rename(dir.path().join("a/y.py"), dir.path().join("a/z.py")).unwrap();
    let renamed = module_tree(&src);
    fs::remove_file(dir.path().join("a/z.py")).unwrap();

    assert_ne!(before, added);
    assert_ne!(added, renamed);
    assert_eq!(before, module_tree(&src));
}

#[test]
fn editing_a_module_does_not_change_the_module_tree() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("a/x.py"));
    let src = [PathBuf::from(dir.path())];
    let before = module_tree(&src);

    fs::write(dir.path().join("a/x.py"), "import os\n").unwrap();

    assert_eq!(before, module_tree(&src));
}

#[test]
fn the_module_tree_ignores_vendored_and_non_identifier_directories() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("a/x.py"));
    let src = [PathBuf::from(dir.path())];
    let before = module_tree(&src);

    touch(&dir.path().join("node_modules/pkg/index.py"));
    touch(&dir.path().join("bazel-out/gen.py"));
    touch(&dir.path().join(".git/hooks/pre.py"));
    touch(&dir.path().join("a/notes.txt"));

    assert_eq!(before, module_tree(&src));
    assert!(!digest_of(dir.path()).contains("bazel-out"));
}

fn checkout(layout: &[&str], contents: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in layout {
        touch(&dir.path().join(file));
    }
    fs::write(dir.path().join(layout[0]), contents).unwrap();
    dir
}

#[test]
fn identical_checkouts_share_a_module_tree_digest() {
    let layout = ["a/x.py", "a/__init__.py", "src/b/y.py"];
    let (one, two) = (checkout(&layout, ""), checkout(&layout, ""));

    assert_eq!(module_tree_at(one.path(), &[]), module_tree_at(two.path(), &[]));
    assert_eq!(
        module_tree_at(one.path(), &[PathBuf::from("src")]),
        module_tree_at(two.path(), &[PathBuf::from("src")]),
    );
    assert_eq!(
        module_tree_at(one.path(), &[one.path().join("src")]),
        module_tree_at(two.path(), &[PathBuf::from("src")]),
        "an absolute root inside the checkout keys like its relative spelling",
    );
}

#[test]
fn checkouts_with_different_module_trees_digest_differently() {
    let one = checkout(&["a/x.py"], "");
    let two = checkout(&["a/x.py", "a/y.py"], "");

    assert_ne!(module_tree_at(one.path(), &[]), module_tree_at(two.path(), &[]));
}

#[test]
fn a_source_root_outside_the_checkout_keeps_its_full_path() {
    let (one, two) = (checkout(&["a/x.py"], ""), checkout(&["a/x.py"], ""));
    let outside = checkout(&["m/z.py"], "");
    let other = checkout(&["m/z.py"], "");

    assert_ne!(
        module_tree_at(one.path(), &[outside.path().to_path_buf()]),
        module_tree_at(two.path(), &[other.path().to_path_buf()]),
    );
}

#[test]
fn identical_checkouts_share_a_package_root() {
    let layout = ["pkg/__init__.py", "pkg/m.py"];
    let (one, two) = (checkout(&layout, ""), checkout(&layout, ""));

    assert_eq!(
        package_root(&one.path().join("pkg/m.py"), one.path()),
        package_root(&two.path().join("pkg/m.py"), two.path()),
    );
}

#[test]
fn the_module_tree_skips_virtualenvs_and_gitignored_directories() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("pkg/__init__.py"));
    touch(&dir.path().join("pkg/mod.py"));
    let before = digest_of(dir.path());

    touch(&dir.path().join("venv/pyvenv.cfg"));
    touch(&dir.path().join("venv/lib/site.py"));
    touch(&dir.path().join("myenv/pyvenv.cfg"));
    touch(&dir.path().join("myenv/lib/site.py"));
    touch(&dir.path().join("env/site-packages/dep/__init__.py"));
    touch(&dir.path().join("build/gen.py"));
    touch(&dir.path().join("generated/gen.py"));
    fs::write(dir.path().join(".gitignore"), "generated/\n").unwrap();

    assert_eq!(before, digest_of(dir.path()).replace("f:.gitignore\n", ""));
    let digest = digest_of(dir.path());
    assert!(digest.contains("d:pkg") && digest.contains("f:pkg/mod.py"));
    for hidden in ["venv", "myenv", "env", "build", "generated"] {
        assert!(!digest.contains(&format!("d:{hidden}")), "{hidden} was walked");
    }
}

#[test]
fn a_plain_venv_named_package_is_still_walked() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("venv/__init__.py"));

    let digest = digest_of(dir.path());
    assert!(digest.contains("d:venv") && digest.contains("f:venv/__init__.py"));
}

/// A file reached as `./pkg/m.py` and by its absolute path has one package-root key.
#[test]
fn a_relative_and_an_absolute_package_root_key_alike() {
    let base = Path::new("/work/repo");
    assert_eq!(super::checkout_relative(Path::new("./pkg"), base), "pkg");
    assert_eq!(super::checkout_relative(&base.join("pkg"), base), "pkg");
    assert_eq!(super::checkout_relative(Path::new("."), base), ".");
}
