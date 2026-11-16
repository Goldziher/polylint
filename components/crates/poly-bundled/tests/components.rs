//! `poly components` on the bundled binary: every engine family the build can link is linked, so nothing
//! it could link is downloadable, the flavor is `bundled`, and `install` is a no-op that downloads nothing.
#![cfg(unix)]

use std::process::{Command, Output};

use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

struct Sandbox {
    repo: TempDir,
    cache: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::write(repo.path().join("ok.rb"), "x = 1\n").expect("write ok.rb");
        Self {
            repo,
            cache: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(POLY)
            .args(args)
            .env_remove("POLY_OFFLINE")
            .env_remove("POLY_COMPONENT_DEV_LOCK")
            .env_remove("POLY_COMPONENT_MIRROR")
            .env("POLY_COMPONENT_CACHE", self.cache.path())
            .current_dir(self.repo.path())
            .output()
            .expect("run poly")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.run(args);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{args:?}: {error}: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

#[test]
fn list_reports_every_family_as_bundled() {
    let value = Sandbox::new().json(&["components", "list", "--format", "json"]);
    assert_eq!(value["flavor"], "bundled");
    let components = value["components"].as_array().expect("components");
    assert!(!components.is_empty());
    // The `mcp` front end hosts no engines and is never linked; only engine components count.
    let engines: Vec<_> = components.iter().filter(|c| c["kind"] == "engines").collect();
    assert!(!engines.is_empty());
    // `pyrefly`, `r`, `secrets` and `tsrs` are built in workspaces of their own, so no bundled build links them: they
    // stay downloadable.
    let (download_only, linkable): (Vec<&&serde_json::Value>, Vec<&&serde_json::Value>) = engines
        .iter()
        .partition(|c| c["name"] == "pyrefly" || c["name"] == "r" || c["name"] == "secrets" || c["name"] == "tsrs");
    assert_eq!(download_only.len(), 4, "{value}");
    assert!(download_only.iter().all(|c| c["state"] != "bundled"), "{value}");
    assert!(!linkable.is_empty());
    assert!(linkable.iter().all(|c| c["state"] == "bundled"), "{value}");
    // The MCP server is compiled in (`bundled-mcp`), so its front end is bundled too, not unavailable.
    let mcp = components.iter().find(|c| c["name"] == "mcp").expect("mcp row");
    assert_eq!(mcp["state"], "bundled", "{value}");
    // No build links the `sh` sidecar's Go executable: the row says the host `shfmt` stands in for it.
    let sh = components.iter().find(|c| c["name"] == "sh").expect("sh row");
    let detail = sh["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("host `shfmt`"), "{value}");
}

#[test]
fn status_needs_nothing_for_a_ruby_file() {
    let value = Sandbox::new().json(&["components", "status", "--format", "json", "ok.rb"]);
    assert_eq!(value["flavor"], "bundled");
    assert_eq!(value["missing"], serde_json::json!([]));
}

#[test]
fn installing_the_linked_families_is_a_noop() {
    let sandbox = Sandbox::new();
    // `all` also names components built in workspaces of their own (`r`), which a bundled build downloads; install
    // only the engine components this build links.
    let listed = sandbox.json(&["components", "list", "--format", "json"]);
    let linked: Vec<&str> = listed["components"]
        .as_array()
        .expect("components")
        .iter()
        .filter(|c| c["kind"] == "engines" && c["state"] == "bundled")
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(!linked.is_empty(), "{listed}");
    let mut args = vec!["component", "install"];
    args.extend(&linked);
    args.extend(["--format", "json"]);
    let value = sandbox.json(&args);
    assert_eq!(value["ok"], true);
    let results = value["results"].as_array().expect("results");
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r["action"] == "bundled"), "{value}");
    assert!(!sandbox.cache.path().join("sha256").exists());
}

#[test]
fn doctor_reports_the_bundled_flavor() {
    // `doctor` exits non-zero on unrelated host findings; only its components section matters here.
    let value = Sandbox::new().json(&["doctor", "--format", "json"]);
    assert_eq!(value["components"]["flavor"], "bundled");
    assert_eq!(value["components"]["needed_missing"], serde_json::json!([]));
}
