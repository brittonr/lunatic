use std::{
    fs::{OpenOptions, create_dir_all},
    io::{Read, Seek, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use toml::{Value, value::Table};

pub(crate) fn start() -> Result<()> {
    // Check if the current directory is a Rust cargo project.
    if !Path::new("Cargo.toml").exists() {
        return Err(anyhow!("Must be called inside a cargo project"));
    }

    // Open or create cargo config file.
    create_dir_all(".cargo").context("failed to create `.cargo/` directory")?;
    let mut config_toml = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(".cargo/config.toml")
        .context("failed to open `.cargo/config.toml`")?;

    let mut content = String::new();
    config_toml
        .read_to_string(&mut content)
        .context("failed to read `.cargo/config.toml`")?;

    let mut content = content
        .parse::<Value>()
        .context("failed to parse `.cargo/config.toml` as TOML")?;
    let table = content
        .as_table_mut()
        .ok_or_else(|| anyhow!("`.cargo/config.toml` root is not a TOML table"))?;

    // Set correct target
    match table.get_mut("build") {
        Some(value) => {
            let build = value
                .as_table_mut()
                .ok_or_else(|| anyhow!("`build` in `.cargo/config.toml` is not a table"))?;
            if let Some(target) = build.get_mut("target") {
                let target_str = target.as_str().ok_or_else(|| {
                    anyhow!("`build.target` in `.cargo/config.toml` is not a string")
                })?;
                if target_str != "wasm32-wasi" {
                    return Err(anyhow!(
                        "value `build.target` inside `.cargo/config.toml` already set to something else than `wasm32-wasi`"
                    ));
                }
            } else {
                // If value is missing, add it.
                build.insert("target".to_owned(), Value::String("wasm32-wasi".to_owned()));
            }
        }
        None => {
            let mut new_build = Table::new();
            new_build.insert("target".to_owned(), Value::String("wasm32-wasi".to_owned()));
            table.insert("build".to_owned(), Value::Table(new_build));
        }
    };

    // Set correct runner
    match table.get_mut("target") {
        Some(value) => {
            let target = value
                .as_table_mut()
                .ok_or_else(|| anyhow!("`target` in `.cargo/config.toml` is not a table"))?;
            match target.get_mut("wasm32-wasi") {
                Some(value) => {
                    let wasm_target = value.as_table_mut().ok_or_else(|| {
                        anyhow!("`target.wasm32-wasi` in `.cargo/config.toml` is not a table")
                    })?;
                    if let Some(runner) = wasm_target.get_mut("runner") {
                        let runner_str = runner.as_str().ok_or_else(|| {
                            anyhow!(
                                "`target.wasm32-wasi.runner` in `.cargo/config.toml` is not a string"
                            )
                        })?;
                        match runner_str {
                            "lunatic" => {
                                // Update old runner to new one
                                wasm_target.insert(
                                    "runner".to_owned(),
                                    Value::String("lunatic run".to_owned()),
                                );
                            }
                            "lunatic run" => {
                                // Correct value is already set, don't do anything.
                            }
                            _ => {
                                return Err(anyhow!(
                                    "value `target.wasm32-wasi.runner` inside `.cargo/config.toml` already set to something else than `lunatic run`"
                                ));
                            }
                        }
                    } else {
                        // If value is missing, add it.
                        wasm_target
                            .insert("runner".to_owned(), Value::String("lunatic run".to_owned()));
                    }
                }
                None => {
                    // Create sub-table `wasm32-wasi` with runner set.
                    let mut new_wasm32_wasi = Table::new();
                    new_wasm32_wasi
                        .insert("runner".to_owned(), Value::String("lunatic run".to_owned()));
                    target.insert("wasm32-wasi".to_owned(), Value::Table(new_wasm32_wasi));
                }
            }
        }
        None => {
            // Create sub-table `wasm32-wasi` with runner set.
            let mut new_wasm32_wasi = Table::new();
            new_wasm32_wasi.insert("runner".to_owned(), Value::String("lunatic run".to_owned()));
            // Create table `target` with value `wasm32-wasi`.
            let mut new_target = Table::new();
            new_target.insert("wasm32-wasi".to_owned(), Value::Table(new_wasm32_wasi));
            table.insert("target".to_owned(), Value::Table(new_target));
        }
    };

    let new_config = toml::to_string(table).context("failed to serialize `.cargo/config.toml`")?;
    // Truncate existing config
    config_toml
        .set_len(0)
        .context("failed to truncate `.cargo/config.toml`")?;
    config_toml
        .rewind()
        .context("failed to rewind `.cargo/config.toml`")?;
    config_toml
        .write_all(new_config.as_bytes())
        .context("failed to write `.cargo/config.toml`")?;

    println!("Cargo project initialized!");

    Ok(())
}
