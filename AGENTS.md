# AGENTS.md

This file provides guidance for agentic coding assistants working in the Agentbox repository.

## Before Making Any Changes

Always read these files first:

- **[conventions.md](conventions.md)** — Rust coding standards
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — Development workflow, commit conventions

## Build, Test, and Quality Commands

```bash
# Build
cargo build
cargo build --release

# Test (cargo-nextest is used in CI)
cargo nextest run
cargo nextest run --release

# Code quality (order matters!)
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo nextest run
```

> Always run format → lint → typecheck → test. Fix lints in the same change as your code.

## Critical Conventions

- **Error handling**: Use SNAFU. No `unwrap()` in production.
- **Lint suppression**: Use `#[expect(...)]` with a reason, NOT `#[allow(...)]`.
- **Import ordering**: std → third-party → self:: → crate:: (4-group).
- **Derive ordering**: std types → third-party, alphabetical within each group.
- **No glob imports**: Never use `use foo::*`.
- **Podman boundary**: All Podman `Command` construction belongs in `src/podman.rs`.
- **Nix/CI**: Keep existing Nix and workflow structure; remove only genuinely obsolete entries.

## Project Structure

| Path                 | Type          | Description                                            |
| -------------------- | ------------- | ------------------------------------------------------ |
| `src/main.rs`        | Binary module | Entrypoint, module declarations, logging               |
| `src/app.rs`         | Module        | Session orchestration; `app/report.rs`, `app/tests.rs` |
| `src/cli/`           | Module        | Clap commands (`mod.rs` plus one file per command)     |
| `src/config/`        | Module        | YAML schema, defaults, expansion, validation           |
| `src/consts.rs`      | Module        | Shared constants                                       |
| `src/error.rs`       | Module        | SNAFU error and result types                           |
| `src/output.rs`      | Module        | Output format selection                                |
| `src/podman.rs`      | Module        | Container command construction and lifecycle           |
| `src/rootfs.rs`      | Module        | Minimal rootfs skeleton and symlinks                   |
| `src/session.rs`     | Module        | Facade over plan/target/runtime/ocelot/toolchain       |
| `src/session/`       | Submodules    | Container planning, targeting, init, config, tools     |
| `src/shell.rs`       | Module        | POSIX shell quoting helper                             |
| `src/ui/`            | Module        | Report DTOs, table rendering, fuzzy picker             |
| `src/workspace.rs`   | Module        | Canonical project identity and path hashing            |
| `devshell/`          | Nix           | Packages, dev shell, container, nfpm/tarball helpers   |
| `.github/workflows/` | CI            | Rust, lints, Nix, release, and container checks        |

## Branch & Commit Convention

- **Base branch**: `develop` (not `main` for features).
- **Branch prefixes**: `feat/*`, `fix/*`, `release/*`, `ci/*`.
- **Commits**: One scope per commit. Format: `<type>(<scope>): <description>`.

## Required Tools (Non-Nix)

If not using Nix, install manually:

- Rust and Cargo (see `rust-toolchain.toml`)
- cargo-nextest
- Podman and tmux for runtime verification
