use std::{
    env::args_os,
    process::{exit, Command},
};

fn main() {
    exit(
        Command::new("cargo")
            .env("CARGO_BUILD_TARGET", "wasm32-wasi")
            .env("CARGO_TARGET_WASM32_WASI_RUNNER", "lunatic")
            .args(args_os().skip(2))
            .status()
            .unwrap()
            .code()
            .unwrap(),
    );
}
