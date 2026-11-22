# Coding Conventions

These are the Rust conventions used in Agentbox.

## Project structure

```text
agentbox/
├── Cargo.toml
├── src/
│   ├── main.rs       # Entrypoint and logging setup
│   ├── cli.rs        # Clap definitions
│   ├── config.rs     # YAML schema, defaults, expansion, validation
│   ├── error.rs      # SNAFU error types
│   ├── workspace.rs  # Project identity and path hashing
│   ├── rootfs.rs     # Minimal rootfs construction
│   ├── podman.rs     # Podman commands and container lifecycle
│   └── app.rs        # Mount/env planning and session orchestration
├── devshell/         # Nix packaging and dev shell
└── .github/          # CI workflows
```

Prefer refactoring an existing module over introducing a new crate.

## Imports

Order imports in four groups with a blank line between each:

1. `std`
2. third-party crates
3. `self::`
4. `crate::`

Do not use glob imports such as `use std::collections::*;`.

```rust
use std::{fs, path::PathBuf};

use serde::Deserialize;
use snafu::ResultExt;

use crate::error::{self, Result};
```

## Derives

Order standard-library derives before third-party derives, alphabetically within each group:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Config;
```

## Error handling

Use SNAFU for all fallible production code. Define domain variants in `src/error.rs` and propagate with `?`.

```rust
pub type Result<T> = std::result::Result<T, Error>;

let content = fs::read_to_string(path).with_context(|_| error::ReadConfigSnafu {
    path: path.to_path_buf(),
})?;
```

- `unwrap()` is prohibited in production code.
- `expect()` is allowed only when failure is impossible and the invariant is obvious.
- Include context that identifies the failing path, command, or container.
- Do not log secrets, tokens, or environment values from the config.

## Lint suppressions

Use `#[expect(...)]` with a reason, never a bare `#[allow(...)]`:

```rust
#[expect(clippy::too_many_arguments, reason = "Podman CLI mapping is intentionally explicit")]
fn build_container_spec(/* ... */) {}
```

## Paths and commands

- Accept path-like inputs as `impl AsRef<Path>` where practical.
- Canonicalize user paths before hashing or comparing them.
- Build external commands as direct arguments; shell quoting belongs only in generated init scripts.
- Keep every Podman invocation inside `src/podman.rs`.

## Logging

Use `tracing` at `debug`/`trace` level for command construction and lifecycle details. User-facing output goes to stdout/stderr directly, and `RUST_LOG` overrides the `-v` verbosity level.

## Tests

Unit tests live next to their modules in `src/` and use `cargo-nextest`:

```bash
cargo nextest run
cargo nextest run -p agentbox
```

Tests may use `expect()` freely. Prefer table-driven cases for parsing and argument construction. Tests that require Podman or tmux should fail clearly when those binaries are unavailable.

## Quality checks

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo nextest run
nix build
```

Run format, lint, type-check, and tests in that order.
