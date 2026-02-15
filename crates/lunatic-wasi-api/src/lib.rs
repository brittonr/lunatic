use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::Result;
use lunatic_common_api::{get_memory, IntoTrap};
use lunatic_process::state::ProcessState;
use lunatic_stdout_capture::StdoutCapture;
use tokio::io::AsyncWrite;
use wasmtime::{Caller, Linker};
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

/// Adapts `StdoutCapture` to `wasmtime_wasi::cli::StdoutStream`.
#[derive(Clone)]
struct CaptureOutputStream(StdoutCapture);

impl IsTerminal for CaptureOutputStream {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for CaptureOutputStream {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(CaptureWriter(self.0.clone()))
    }
}

/// Implements `tokio::io::AsyncWrite` by delegating to `StdoutCapture::write_bytes()`.
///
/// All writes are synchronous (always `Poll::Ready`) since `StdoutCapture`
/// writes to an in-memory buffer behind a mutex.
struct CaptureWriter(StdoutCapture);

impl AsyncWrite for CaptureWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.0.write_bytes(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Create a `WasiP1Ctx` from configuration settings.
pub fn build_wasi(
    args: Option<&Vec<String>>,
    envs: Option<&Vec<(String, String)>>,
    dirs: &[(String, String)],
    stdout: Option<StdoutCapture>,
    stderr: Option<StdoutCapture>,
) -> Result<WasiP1Ctx> {
    let mut builder = WasiCtxBuilder::new();
    builder.inherit_stdin();
    match stdout {
        Some(capture) => builder.stdout(CaptureOutputStream(capture)),
        None => builder.inherit_stdout(),
    };
    match stderr {
        Some(capture) => builder.stderr(CaptureOutputStream(capture)),
        None => builder.inherit_stderr(),
    };
    if let Some(envs) = envs {
        for (key, value) in envs {
            builder.env(key, value);
        }
    }
    if let Some(args) = args {
        builder.args(args);
    }
    for (preopen_dir_path, resolved_path) in dirs {
        builder.preopened_dir(
            Path::new(resolved_path),
            preopen_dir_path,
            DirPerms::all(),
            FilePerms::all(),
        )?;
    }
    Ok(builder.build_p1())
}

pub trait LunaticWasiConfigCtx {
    fn add_environment_variable(&mut self, key: String, value: String);
    fn add_command_line_argument(&mut self, argument: String);
    fn preopen_dir(&mut self, dir: String);
}

pub trait LunaticWasiCtx {
    fn wasi(&self) -> &WasiP1Ctx;
    fn wasi_mut(&mut self) -> &mut WasiP1Ctx;
    fn set_stdout(&mut self, stdout: StdoutCapture);
    fn get_stdout(&self) -> Option<&StdoutCapture>;
    fn set_stderr(&mut self, stderr: StdoutCapture);
    fn get_stderr(&self) -> Option<&StdoutCapture>;
}

// Register WASI APIs to the linker
pub fn register<T>(linker: &mut Linker<T>) -> Result<()>
where
    T: ProcessState + LunaticWasiCtx + Send + 'static,
    T::Config: LunaticWasiConfigCtx,
{
    // Register all wasi host functions using the new p1 async API
    wasmtime_wasi::p1::add_to_linker_async(linker, |ctx| ctx.wasi_mut())?;

    // Register host functions to configure wasi
    linker.func_wrap(
        "lunatic::wasi",
        "config_add_environment_variable",
        add_environment_variable,
    )?;
    linker.func_wrap(
        "lunatic::wasi",
        "config_add_command_line_argument",
        add_command_line_argument,
    )?;
    linker.func_wrap("lunatic::wasi", "config_preopen_dir", preopen_dir)?;

    Ok(())
}

// Adds environment variable to a configuration.
//
// Traps:
// * If the config ID doesn't exist.
// * If the key or value string is not a valid utf8 string.
// * If any of the memory slices falls outside the memory.
fn add_environment_variable<T>(
    mut caller: Caller<T>,
    config_id: u64,
    key_ptr: u32,
    key_len: u32,
    value_ptr: u32,
    value_len: u32,
) -> Result<()>
where
    T: ProcessState,
    T::Config: LunaticWasiConfigCtx,
{
    let memory = get_memory(&mut caller)?;
    let key_str = memory
        .data(&caller)
        .get(key_ptr as usize..(key_ptr + key_len) as usize)
        .or_trap("lunatic::wasi::config_add_environment_variable")?;
    let key = std::str::from_utf8(key_str)
        .or_trap("lunatic::wasi::config_add_environment_variable")?
        .to_string();
    let value_str = memory
        .data(&caller)
        .get(value_ptr as usize..(value_ptr + value_len) as usize)
        .or_trap("lunatic::wasi::config_add_environment_variable")?;
    let value = std::str::from_utf8(value_str)
        .or_trap("lunatic::wasi::config_add_environment_variable")?
        .to_string();

    caller
        .data_mut()
        .config_resources_mut()
        .get_mut(config_id)
        .or_trap("lunatic::wasi::config_set_max_memory: Config ID doesn't exist")?
        .add_environment_variable(key, value);
    Ok(())
}

// Adds command line argument to a configuration.
//
// Traps:
// * If the config ID doesn't exist.
// * If the argument string is not a valid utf8 string.
// * If any of the memory slices falls outside the memory.
fn add_command_line_argument<T>(
    mut caller: Caller<T>,
    config_id: u64,
    argument_ptr: u32,
    argument_len: u32,
) -> Result<()>
where
    T: ProcessState,
    T::Config: LunaticWasiConfigCtx,
{
    let memory = get_memory(&mut caller)?;
    let argument_str = memory
        .data(&caller)
        .get(argument_ptr as usize..(argument_ptr + argument_len) as usize)
        .or_trap("lunatic::wasi::add_command_line_argument")?;
    let argument = std::str::from_utf8(argument_str)
        .or_trap("lunatic::wasi::add_command_line_argument")?
        .to_string();

    caller
        .data_mut()
        .config_resources_mut()
        .get_mut(config_id)
        .or_trap("lunatic::wasi::add_command_line_argument: Config ID doesn't exist")?
        .add_command_line_argument(argument);
    Ok(())
}

// Mark a directory as preopened in the configuration.
//
// Traps:
// * If the config ID doesn't exist.
// * If the directory string is not a valid utf8 string.
// * If any of the memory slices falls outside the memory.
fn preopen_dir<T>(mut caller: Caller<T>, config_id: u64, dir_ptr: u32, dir_len: u32) -> Result<()>
where
    T: ProcessState,
    T::Config: LunaticWasiConfigCtx,
{
    let memory = get_memory(&mut caller)?;
    let dir_str = memory
        .data(&caller)
        .get(dir_ptr as usize..(dir_ptr + dir_len) as usize)
        .or_trap("lunatic::wasi::preopen_dir")?;
    let dir = std::str::from_utf8(dir_str)
        .or_trap("lunatic::wasi::preopen_dir")?
        .to_string();

    caller
        .data_mut()
        .config_resources_mut()
        .get_mut(config_id)
        .or_trap("lunatic::wasi::preopen_dir: Config ID doesn't exist")?
        .preopen_dir(dir);
    Ok(())
}
