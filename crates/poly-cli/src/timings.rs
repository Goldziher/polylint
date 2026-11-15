//! `--timings` / `POLY_TIMINGS`: switching collection on and printing the block.

use crate::CommonArgs;

/// Whether `--timings` or `POLY_TIMINGS` asks for the timings block. The variable
/// counts when set to anything but empty, `0` or `false`.
pub(crate) fn requested(common: &CommonArgs) -> bool {
    common.timings
        || std::env::var("POLY_TIMINGS")
            .is_ok_and(|value| !matches!(value.trim(), "" | "0") && !value.trim().eq_ignore_ascii_case("false"))
}

/// Start collecting when requested, before anything worth timing runs.
pub(crate) fn start(common: &CommonArgs) {
    if requested(common) {
        poly_core::timings::enable();
    }
}

/// Print the collected timings to stderr and stop collecting.
pub(crate) fn print(common: &CommonArgs) {
    if requested(common) {
        eprint!("{}", poly_core::timings::take().render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Probe {
        #[command(flatten)]
        common: CommonArgs,
    }

    #[test]
    fn the_flag_requests_timings() {
        let probe = Probe::parse_from(["probe", "--timings"]);
        assert!(requested(&probe.common));
    }
}
