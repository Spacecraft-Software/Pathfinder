//! Pathfinder: jaq's command line on the driver in this crate's library.
#![expect(
    clippy::multiple_crate_versions,
    reason = "syn 2 (jiff-static, under jiff) and syn 3 (serde_derive) are both build-time proc-macro dependencies of third-party crates"
)]

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    jaq::main()
}
