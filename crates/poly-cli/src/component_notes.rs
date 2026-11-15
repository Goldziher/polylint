//! Notes for engine components a run could not use.

/// Print why each needed component could not be used, once, and say whether any fails the run.
///
/// A component `on_missing = "skip"` turned into skips does not fail the run (the user chose
/// that), but it is never silent: a warning names it and the files it left unchecked, and the
/// document's `summary.missing_components` counts it.
pub(crate) fn report_blocking_components(components: &poly_core::components::ComponentsReport) -> bool {
    for line in component_lines(components) {
        eprintln!("{line}");
    }
    components.blocking()
}

/// One `error:` or `warning:` line per missing component; see [`report_blocking_components`].
pub(crate) fn component_lines(components: &poly_core::components::ComponentsReport) -> Vec<String> {
    components
        .missing
        .iter()
        .map(|missing| {
            if missing.blocking {
                format!("error: {}", missing.reason)
            } else {
                format!(
                    "warning: {}; the files it handles were skipped, not checked (`on_missing = \"skip\"`)",
                    missing.reason
                )
            }
        })
        .collect()
}
