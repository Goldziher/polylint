//! `poly` — the single universal, zero-dependency linter & formatter CLI.

use std::process::ExitCode;

// musl's allocator serialises the rayon workers behind one lock (#179); glibc, macOS and Windows
// keep their system allocators.
#[cfg(all(target_env = "musl", not(feature = "dhat-heap")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() -> ExitCode {
    // Writes dhat-heap.json to the working directory when dropped at the end of main.
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    poly_cli::cli::main()
}
