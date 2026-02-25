# Napkin

## Corrections
| Date | Source | What Went Wrong | What To Do Instead |
|------|--------|----------------|-------------------|
| 2026-02-24 | self | Missed that `test_plugin.rs` was untracked by git, causing nix sandbox fmt failure | Always check `git status` for untracked files when nix flake check fails on missing files |
| 2026-02-24 | self | Didn't update wasm target regex in main.rs when changing to wasip1 | When renaming wasm targets, grep for old target names in regex patterns too |
| 2026-02-24 | self | Test path regex assumed `target/` dir name, but CARGO_TARGET_DIR=~/.cargo-target breaks that | Don't hardcode `target` dir name in path regexes — custom CARGO_TARGET_DIR means the dir can be anything |

## User Preferences
- User wants to get things building and running, not just reading

## Patterns That Work
- `wasm32-wasi` renamed to `wasm32-wasip1` in modern Rust — just update `.cargo/config.toml`
- lunatic runtime binary lands at `~/.cargo-target/release/lunatic` (CARGO_TARGET_DIR is set)
- `nix develop --command cargo build` is the build incantation in this setup
- submillisecond 0.4.1 works with lunatic 0.13.2 runtime with zero code changes

## Patterns That Don't Work
- `rustc`/`cargo` bare PATH — only available inside `nix develop` shell or via `~/.local/bin/cargo`
- Don't forget `git add flake.nix` before `nix develop` in a repo — nix won't see untracked files

## Domain Notes
- Lunatic is an Erlang-inspired actor runtime for WebAssembly
- `rustc` not on PATH directly, but `cargo` is at `~/.local/bin/cargo`
- Project has a `flake.nix`, no `rust-toolchain.toml`
- submillisecond is a companion web framework for lunatic
- CARGO_TARGET_DIR is `~/.cargo-target` (shared across projects)
- submillisecond lives at `/home/brittonr/git/submillisecond`
- Port 3000 is occupied on this machine — use 3001+ for testing
