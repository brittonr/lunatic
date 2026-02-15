use std::{
    env::{args_os, set_var},
    process::{exit, Command},
};

fn main() {
    // SAFETY: Called in single-threaded main before spawning any threads.
    unsafe {
        set_var("CARGO_BUILD_TARGET", "wasm32-wasi");
        set_var("CARGO_TARGET_WASM32_WASI_RUNNER", "lunatic");
    }
    exit(
        Command::new("cargo")
            .args(args_os().skip(2))
            .status()
            .unwrap()
            .code()
            .unwrap(),
    );
}
