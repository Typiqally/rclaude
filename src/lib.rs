//! Isolated Claude remote controller. Provider credentials never enter its broker protocol.

pub mod protocol;

pub mod auth;
pub mod client;

pub mod broker;
pub mod stream;

pub mod history;

pub mod cli;
pub mod launcher;

pub fn engine_main() -> i32 {
    // Never let provider data or credential-bearing panic values enter diagnostics.
    std::panic::set_hook(Box::new(|_| eprintln!("rclaude: broker execution failed")));
    std::panic::catch_unwind(cli::run).unwrap_or(1)
}

// ADR-0001: the only engine ABI is a versioned C function without Rust values.
// All Rust panics are caught before returning across the ABI boundary.
#[unsafe(no_mangle)]
pub extern "C" fn rclaude_engine_main_v1() -> i32 {
    engine_main()
}
