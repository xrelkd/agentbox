# Contributing to Agentbox

> For Rust coding standards, see [conventions.md](conventions.md).

Agentbox is a single Rust crate. The existing Nix flake and GitHub Actions workflows are part of the product: keep them working when renaming or pruning legacy settings.

## Development environment

Nix is recommended:

```bash
nix develop
cargo build
cargo nextest run
```

Without Nix, install:

- Rust and Cargo (see `rust-toolchain.toml`)
- `cargo-nextest`
- Podman and tmux for runtime verification
- `rustfmt` and `clippy` for quality checks

## Build, test, and quality

Always run these before opening a pull request:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo nextest run
nix build
```

Fix lint and compilation problems in the same commit as the change that introduced them.

## Project layout

| Path                 | Description                                                   |
| -------------------- | ------------------------------------------------------------- |
| `src/`               | Agentbox CLI implementation.                                  |
| `devshell/`          | Nix packages, dev shell, container, and nfpm/tarball helpers. |
| `.github/workflows/` | Rust, lints, Nix, release, and container CI.                  |

## Branching and pull requests

- Base branch: `develop`.
- Branch prefixes: `feat/*`, `fix/*`, `release/*`, `ci/*`.
- Keep one logical scope per commit.
- Update docs and tests with behavior changes.
- Confirm all CI jobs pass before requesting review.

## Commit messages

Follow Conventional Commits:

```text
<type>(<scope>): <description>
```

Common types: `feat`, `fix`, `refactor`, `docs`, `style`, `test`, `ci`, `chore`, `build`.

Common scopes: `actions`, `nix`, `docs`.

Examples:

```text
feat: add container log capture on startup timeout
fix: validate absolute container workdir
docs(readme): document shell completions
ci(nix): verify the agentbox executable in Nix build
```

Prefer a single scope. If a change touches several areas, split it into separate commits where practical.
