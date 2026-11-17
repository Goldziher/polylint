use std::path::PathBuf;

use poly_frontend_api::wire::InstallMode;

use super::*;

fn skip(path: &str, reason: &str) -> SkippedFile {
    SkippedFile {
        path: PathBuf::from(path),
        reason: reason.to_owned(),
    }
}

/// `(name, errors, blocking component, skips over budget, findings, exit code, isError)`.
type Case<'a> = (&'a str, usize, bool, &'a [&'a SkippedFile], bool, u8, bool);

/// One table, two mappings: the CLI exit code and the MCP `isError` flag come from the same
/// verdict, so the surfaces cannot classify one run differently.
#[test]
fn every_run_condition_maps_to_one_exit_code_and_one_error_flag() {
    let over = [skip("a.csproj", "no matching engine for this file type")];
    let over: Vec<&SkippedFile> = over.iter().collect();
    // (name, errors, blocking, over budget, findings) -> (exit code, isError)
    let cases: [Case<'_>; 6] = [
        ("clean", 0, false, &[], false, 0, false),
        ("error-severity findings or drift", 0, false, &[], true, 1, false),
        ("a file poly failed on", 1, false, &[], false, 2, true),
        ("a blocking missing component", 0, true, &[], false, 2, true),
        ("skips over budget", 0, false, &over, false, 2, true),
        ("failure outranks findings", 1, false, &[], true, 2, true),
    ];
    for (name, errors, blocking, over_budget, findings, exit, is_error) in cases {
        let verdict = Verdict::classify(errors, blocking, over_budget, Some(0), findings);
        assert_eq!(verdict.exit_code(), exit, "{name}: {verdict:?}");
        assert_eq!(verdict.is_error(), is_error, "{name}: {verdict:?}");
    }
}

#[test]
fn the_skip_budget_charges_only_unannounced_skips() {
    let skipped = [
        skip("a.csproj", "no matching engine for this file type"),
        skip("b.py", poly_core::runner::FILTERED_SKIP),
    ];
    assert!(over_budget(&skipped, None).is_empty(), "no budget, no charge");
    assert!(
        over_budget(&skipped, Some(1)).is_empty(),
        "one charged skip fits a budget of one"
    );
    let charged = over_budget(&skipped, Some(0));
    assert_eq!(charged.len(), 1, "{charged:?}");
    assert_eq!(charged[0].path, PathBuf::from("a.csproj"));
}

#[test]
fn the_request_maps_onto_the_run_options() {
    let config = Config {
        force_exclude: true,
        ..Config::default()
    };
    let request = RunRequest {
        exclude: vec!["gen/**".to_owned()],
        only: vec!["ruff".to_owned()],
        include_excluded: true,
        no_cache: true,
        jobs: Some(3),
        fix_generated: true,
        config: Some("poly.toml".to_owned()),
        install: InstallMode::Never,
        ..RunRequest::default()
    };
    let options = run_options(&request, &config, None);
    assert_eq!(options.exclude, ["gen/**"]);
    assert_eq!(options.only, ["ruff"]);
    assert!(
        !options.force_exclude,
        "include_excluded beats [discovery] force_exclude"
    );
    assert!(options.no_cache && options.fix_generated && options.explicit_config);
    assert_eq!(options.jobs, Some(3));
    assert_eq!(
        options.on_missing,
        Some(poly_config::v2::OnMissing::Skip),
        "install: never implies skip"
    );
    let explicit = RunRequest {
        on_missing: Some(OnMissingMode::Fail),
        ..request
    };
    assert_eq!(
        run_options(&explicit, &config, None).on_missing,
        Some(poly_config::v2::OnMissing::Fail),
        "an explicit on_missing wins over the one install implies"
    );
    assert!(run_options(&RunRequest::default(), &config, None).force_exclude);
}

#[test]
fn only_and_skip_together_are_refused() {
    let request = RunRequest {
        only: vec!["ruff".to_owned()],
        skip: vec!["typos".to_owned()],
        ..RunRequest::default()
    };
    assert!(validate(&request).is_err());
    assert!(validate(&RunRequest::default()).is_ok());
}
