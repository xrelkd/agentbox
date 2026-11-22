# Agentbox

Agentbox is a Rust CLI that builds a reproducible, zero-image Podman sandbox around the current project directory. It mounts the host Nix store into a minimal container rootfs, then starts an ocelot-supervised service set or an interactive OpenCode, Claude, Gemini, or shell session inside that sandbox.

## Features

- Runs Podman without building or pulling an application image.
- Reuses the host Nix store and optional Nix daemon socket.
- Hashes the canonical project path into stable container, hostname, home, and temporary-directory names.
- Keeps the container home directory on the host across sessions, including caches and shell history.
- Keeps agent state under `~/.agents` and provider config/cache directories.
- Supports rootfs overlays, a `/var/tmp` tmpfs, read-only containers, capability dropping, and resource limits.
- Uses `ocelot supervise` as container PID 1 when available, with a `podman --init` keep-alive fallback.
- Runs OpenCode as a supervised service, not as PID 1, with a `/dev/tcp` readiness check.
- Runs interactive sessions through direct `podman exec -it` without an extra session multiplexer.
- Resumes native Claude and Gemini sessions through their CLI `--resume` flag.
- Ships YAML configuration with partial overrides, `~` expansion, and environment-variable expansion.

## Requirements

- Linux with Podman available in `PATH`.
- Optional: `ocelot` in the sandbox environment for supervised services.
- Optional: a local Nix store, Nix daemon, and any of `opencode`, `claude`, or `gemini` inside the sandbox environment.

## Build and install

```bash
cargo build --release
cargo install --path .
```

With Nix:

```bash
nix build
./result/bin/agentbox --version

nix build .#agentbox-static
nix build .#container
```

## Quick start

```bash
agentbox config init
agentbox config path
agentbox start        # create and start the container
agentbox oc           # attach to the OpenCode server inside the container
agentbox shell        # attach to a shell in the running container
agentbox stop         # stop and remove the container
```

Only `agentbox start` creates a container. `opencode`, `oc`, `agent`, `oc-tui`, `claude`, `gemini`, and `shell` require a running container and only attach to it.

The default configuration is written to `$XDG_CONFIG_HOME/agentbox/config.yaml`, or `~/.config/agentbox/config.yaml` when `XDG_CONFIG_HOME` is unset. Agentbox uses built-in defaults when the file does not exist.

| Command                                 | Description                                                        |
| --------------------------------------- | ------------------------------------------------------------------ |
| `agentbox`                              | Start the container detached (same as `agentbox start`).           |
| `agentbox start [--attach] [--agent N]` | Start the container; bare `start` returns to the host shell.       |
| `agentbox agent <NAME>`                 | Attach to a configured agent in the running container.             |
| `agentbox oc`                           | Attach to the OpenCode server inside the container.                |
| `agentbox oc-tui`                       | Attach to an OpenCode TUI session in the running container.        |
| `agentbox claude`                       | Resume the latest Claude session in the running container.         |
| `agentbox gemini`                       | Resume the latest Gemini session in the running container.         |
| `agentbox shell`                        | Attach to an interactive shell in the running container.           |
| `agentbox tmp`                          | Print the host temporary directory for this project.               |
| `agentbox home`                         | Print the persistent home directory for this project.              |
| `agentbox stop`                         | Stop the container and clean temporary directories.                |
| `agentbox stop --purge-home`            | Also remove the persistent home directory.                         |
| `agentbox list`                         | List project containers. Aliases: `ps`, `ls`.                     |
| `agentbox config init [--force]`        | Write the default YAML configuration.                              |
| `agentbox config path`                  | Print the resolved configuration file path.                        |
| `agentbox completions bash/fish/zsh`    | Generate shell completions.                                        |

Global flags:

- `--config PATH` selects a configuration file.
- `-v`/`-vv` increases log verbosity.
- `RUST_LOG` controls `tracing` filters.

## Configuration

Only the values you want to change need to appear in YAML; missing fields use built-in defaults. Paths support `~` and environment variables.

```yaml
general:
  container_prefix: agentbox
  ai_env_path: ~/.local/share/agentbox-env
  default_agent: shell
  ocelot_path: /run/ai-env/bin/ocelot

container:
  memory_limit: 8g
  read_only: true

server:
  serve_port: 4096
  serve_host: 127.0.0.1
  listen_host: 127.0.0.1
  startup_timeout_secs: 20

agents:
  opencode:
    command: opencode
    serve:
      command: opencode serve
      attach: opencode attach
  claude:
    command: claude --resume
  gemini:
    command: gemini --resume
```

Run `agentbox config init` to print the complete default template.

`agentbox oc` waits for the server in the container to become ready, then runs `<attach> http://localhost:<serve_port>` with `podman exec` inside the container. No host port is published: the server listens on the container loopback address and is reachable only from inside the container.

`agentbox claude` and `agentbox gemini` use the CLI's native `--resume` flag to reopen the most recent provider session in the current project. They are plain `podman exec -it` sessions, so detaching leaves no server running inside the container; override `agents.claude.command` or `agents.gemini.command` to change that behavior. Older provider versions may require at least one prior session before `--resume` works.

## Security considerations

- The YAML config, agent commands, and secret files are trusted host input: agentbox reads and expands them with host privileges before starting the container. Only load configs you control.
- No host port is published. The container server listens on loopback and is reached with `podman exec`; `server.listen_host` defaults to `127.0.0.1`.
- Secret values are written to a `0600` file under the per-project tmp dir and passed with `--env-file`. They are redacted from debug logs, but any process inside the container can still read its own environment, and a host user who can run `podman inspect` may see the env file path.
- AI provider config dirs are mounted read-only; only session/state subdirs are writable. The host Nix config dir is copied into the per-project home and mounted read-only from that copy.
- `/tmp` and `/var/tmp` are size-limited tmpfs mounts with `nodev` (`/var/tmp` also `noexec`). Writable bind mounts use `nosuid,nodev`. Kernel module mounts and proxy environment inheritance are disabled by default.
- Resource limits are defense in depth, not a hard security boundary: a writable bind mount can still consume host disk, and an untrusted agent with a host Nix daemon socket can ask the host daemon to build derivations. Disable `nix.enable_daemon_mount` when running untrusted code.
- On SELinux systems, additional bind mounts may require `z`/`Z` relabeling. Do not weaken `--read-only`, `--cap-drop=ALL`, or `--security-opt=no-new-privileges` unless you accept the consequences.

## Architecture

| File                | Responsibility                                                       |
| ------------------- | -------------------------------------------------------------------- |
| `src/main.rs`       | Entrypoint, logging setup, config and completion fast paths.         |
| `src/cli.rs`        | Clap command definitions.                                            |
| `src/config/mod.rs` | YAML schema, defaults, path resolution, expansion, validation.       |
| `src/error.rs`      | SNAFU error and result types.                                        |
| `src/ocelot.rs`     | ocelot supervise config serialization.                               |
| `src/workspace.rs`  | Project identity, canonical path hashing, host identity.             |
| `src/rootfs.rs`     | Minimal rootfs skeleton, passwd/group files, symlinks.               |
| `src/podman.rs`     | Podman command construction, container lifecycle, readiness polling. |
| `src/app.rs`        | Mount/env planning, init scripts, ocelot config, session launching.  |

The original shell prototype is kept in `prototypes/ai-box` for reference.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo nextest run
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [conventions.md](conventions.md).

## License

GPL-3.0
