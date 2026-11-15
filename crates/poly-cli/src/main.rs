//! `poly` — the single universal, zero-dependency linter & formatter CLI.

use std::process::ExitCode;

// musl's allocator serialises the rayon workers behind one lock (#179); glibc, macOS and Windows
// keep their system allocators.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    poly_cli::cli::main()
}
