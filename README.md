# Agentbox

[![Rust CI](https://github.com/xrelkd/agentbox/actions/workflows/rust.yaml/badge.svg)](https://github.com/xrelkd/agentbox/actions/workflows/rust.yaml)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
[![MSRV 1.98](https://img.shields.io/badge/rust-1.98%2B-orange.svg)](rust-toolchain.toml)

Agentbox is a Rust CLI that builds a reproducible, zero-image Podman sandbox around the current project directory. It mounts the host Nix store into a minimal container rootfs, then starts an ocelot-supervised service set or an interactive OpenCode, Claude, Gemini, or shell session inside that sandbox.

## Table of Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Command Line Interface](#command-line-interface)
- [Configuration](#configuration)
- [Integration](#integration)
- [Security considerations](#security-considerations)
- [How it works](#how-it-works)
- [Architecture](#architecture)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Features

- Runs Podman without building or pulling an application image.
- Reuses the host Nix store and optional Nix daemon socket.
- Hashes the canonical project path into stable container, hostname, home, and temporary-directory names.
- Keeps the container home directory on the host across sessions, including caches and shell history.
- Keeps agent state under `~/.agents` and provider config/cache directories.
- Supports rootfs overlays, a `/var/tmp` tmpfs, read-only containers, capability dropping, and resource limits.
- Supervises long-running agent services when `ocelot` is available, with Podman's init as a fallback.
- Waits for OpenCode's server to become ready before attaching to it.
- Runs interactive agent and shell sessions directly inside the container.
- Resumes native Claude and Gemini sessions through their CLI `--resume` flag.
- Ships YAML configuration with partial overrides, `~` expansion, and environment-variable expansion.

## Requirements

- Linux with Podman available in `PATH`.
- Rust 1.98 or newer when building from source.
- Optional: `ocelot` in the sandbox environment for supervised services.
- Optional: a local Nix store, Nix daemon, and any of `opencode`, `claude`, or `gemini` inside the sandbox environment.

## Installation

### From source

```bash
cargo build --release
cargo install --path .
```

### With Nix

```bash
nix build
./result/bin/agentbox --version
./result/bin/agentbox version --client

nix build .#agentbox-static
nix build .#container
```

## Quick start

```bash
agentbox config init
agentbox config path
agentbox              # list project containers
agentbox start        # create and start the container
agentbox agent        # attach to the default agent (or choose with the fuzzy picker)
agentbox oc           # attach to the OpenCode server inside the container
agentbox shell        # attach to a shell in the running container
agentbox stop         # stop and remove the container
```

Only `agentbox start` creates a container. `opencode`, `oc`, `agent`, `claude`, `gemini`, and `shell` require a running container and only attach to it. OpenCode's TUI is available with `agentbox agent opencode`. Bare `agentbox agent` uses `general.default_agent`; in an interactive terminal it first opens a fuzzy picker containing all configured agents plus `shell`.

`stop`, `status`, `tmp`, `home`, and the attach commands also work outside the project directory: they use the current directory's container when there is one, otherwise they open a container picker in an interactive terminal. Pass `--container NAME` to select explicitly (required in scripts when multiple containers exist), or run from another directory with:

```bash
agentbox --container agentbox-my-project-abcd1234 oc
```

| Command                                 | Description                                                                             |
| --------------------------------------- | --------------------------------------------------------------------------------------- |
| `agentbox`                              | List project containers (same as `agentbox list`).                                      |
| `agentbox start [--attach] [--agent N]` | Start the container; bare `start` returns to the host shell. Aliases: `s`, `up`.        |
| `agentbox agent [NAME]`                 | Attach to a configured agent; omit `NAME` to use the default or fuzzy-pick. Alias: `a`. |
| `agentbox pick`                         | Print the agent chosen by the default chain or fuzzy picker.                            |
| `agentbox oc`                           | Attach to the OpenCode server inside the container. Alias: `opencode`.                  |
| `agentbox claude`                       | Resume the latest Claude session in the running container. Alias: `c`.                  |
| `agentbox gemini`                       | Resume the latest Gemini session in the running container. Alias: `g`.                  |
| `agentbox shell`                        | Attach to an interactive shell in the running container. Alias: `sh`.                   |
| `agentbox tmp`                          | Print the host temporary directory for this project.                                    |
| `agentbox home`                         | Print the persistent home directory for this project.                                   |
| `agentbox stop`                         | Stop the container and clean temporary directories. Aliases: `x`, `down`.               |
| `agentbox stop --purge-home`            | Also remove the persistent home directory.                                              |
| `agentbox list [--format json] [-q]`    | List containers and the current project. Aliases: `l`, `ps`, `ls`.                      |
| `agentbox status [--format json] [-q]`  | Show current-directory identity and paths. Aliases: `i`, `st`, `info`, `project`.       |
| `agentbox config init [--full]`         | Write minimal YAML config; `--full` writes all settings.                                |
| `agentbox config show [--full]`         | Print the built-in minimal/full YAML template to stdout.                                |
| `agentbox config path`                  | Print the resolved configuration file path.                                             |
| `agentbox version [--client]`           | Print build metadata; `--client` skips config and Podman.                               |
| `agentbox completions <SHELL>`          | Generate completions (bash, elvish, fish, powershell, zsh).                             |

Global flags:

- `--config PATH` selects a configuration file.
- `--container NAME` selects the container for `stop`, `status`, `tmp`, `home`, and attach commands.
- `-v`/`-vv` increases log verbosity.
- `RUST_LOG` controls `tracing` filters.

`agentbox list` lists every agentbox-managed container, then prints a `CURRENT PROJECT` block derived from the current directory:

```text
NAME                              STATUS       PORTS
agentbox-my-project-abcd1234      Up 2 minutes -
CURRENT PROJECT
  Dir       /work/my-project
  Container agentbox-my-project-abcd1234 (running)
  Home      /tmp/agentbox/1000-my-project-abcd1234/home
  Tmp       /tmp/agentbox/1000-my-project-abcd1234/tmp
```

`agentbox status` expands that block with the project hash, hostname, config path, user, shell, environment variable names, and a `MOUNTS` section listing each planned bind mount with its host path, container path, and `ro`/`rw` mode. Both commands accept `--format json` for machine-readable output and `-q`/`--quiet` to print only container names (`status` prints the current container name). Secret values are never printed; only their variable names appear.

Runtime directories use `<runtime_base_dir>/<uid>-<project_name>-<hash>/{home,tmp,meta}`; the default base is `/tmp/agentbox`, and `meta/rootfs` is the generated rootfs passed to Podman.

## Command Line Interface

Complete `--help` output for the main command and every subcommand.

### Main command

```text
$ agentbox --help
Zero-image Podman AI sandboxes for isolated agent sessions

Usage: agentbox [OPTIONS] [COMMAND]

Commands:
  start        Start the project container; other commands only attach to it
  stop         Stop the project container and clean temporary directories
  list         List agentbox-managed containers and the current project directory
  status       Show identity and paths for the current project directory
  shell        Attach to a shell inside the session (alias: `sh`)
  agent        Launch a configured agent (alias: `a`)
  pick         Print the agent chosen by the default chain or interactive picker
  opencode     Attach to the `OpenCode` server in the running container (alias: `oc`)
  claude       Attach to the `Claude` server in the running container
  gemini       Attach to the `Gemini` server in the running container
  tmp          Print the temporary directory for the current workspace
  home         Print the container home directory for the current workspace
  version      Print client and server version information
  completions  Generate shell completion scripts
  config       Manage the agentbox configuration file
  help         Print this message or the help of the given subcommand(s)

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
  -V, --version           Print version
```

### Subcommands

#### start

```text
$ agentbox start --help
Start the project container; other commands only attach to it

Usage: agentbox start [OPTIONS]

Options:
      --attach            Attach to the server after creating the session
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --agent <NAME>      Agent to launch (`general.default_agent` when omitted)
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### stop

```text
$ agentbox stop --help
Stop the project container and clean temporary directories

Usage: agentbox stop [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --purge-home        Remove container-created home data before stopping
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### list

```text
$ agentbox list --help
List agentbox-managed containers and the current project directory

Usage: agentbox list [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --format <FORMAT>   Emit JSON instead of a table [possible values: table, json]
  -q, --quiet             Print only container names
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### status

```text
$ agentbox status --help
Show identity and paths for the current project directory

Usage: agentbox status [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --format <FORMAT>   Emit JSON instead of a key/value report [possible values: table, json]
  -q, --quiet             Print only the project container name
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### shell

```text
$ agentbox shell --help
Attach to a shell inside the session (alias: `sh`)

Usage: agentbox shell [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### agent

```text
$ agentbox agent --help
Launch a configured agent (alias: `a`)

Usage: agentbox agent [OPTIONS] [NAME]

Arguments:
  [NAME]  Configured agent name (`general.default_agent` when omitted)

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### pick

```text
$ agentbox pick --help
Print the agent chosen by the default chain or interactive picker

Usage: agentbox pick [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### opencode

```text
$ agentbox opencode --help
Attach to the `OpenCode` server in the running container (alias: `oc`)

Usage: agentbox opencode [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### claude

```text
$ agentbox claude --help
Attach to the `Claude` server in the running container

Usage: agentbox claude [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### gemini

```text
$ agentbox gemini --help
Attach to the `Gemini` server in the running container

Usage: agentbox gemini [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### tmp

```text
$ agentbox tmp --help
Print the temporary directory for the current workspace

Usage: agentbox tmp [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### home

```text
$ agentbox home --help
Print the container home directory for the current workspace

Usage: agentbox home [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### version

```text
$ agentbox version --help
Print client and server version information

Usage: agentbox version [OPTIONS]

Options:
      --client            Print only client build metadata
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### completions

```text
$ agentbox completions --help
Generate shell completion scripts

Usage: agentbox completions [OPTIONS] <SHELL>

Arguments:
  <SHELL>  Shell to generate completions for [possible values: bash, elvish, fish, powershell, zsh]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

#### config

```text
$ agentbox config --help
Manage the agentbox configuration file

Usage: agentbox config [OPTIONS] <COMMAND>

Commands:
  init  Write a config file
  path  Print the resolved config path
  show  Print the built-in config template without reading it from disk
  help  Print this message or the help of the given subcommand(s)

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

##### config init

```text
$ agentbox config init --help
Write a config file

Usage: agentbox config init [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --force             Overwrite an existing config file
      --full              Include the fully commented example config
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

##### config path

```text
$ agentbox config path --help
Print the resolved config path

Usage: agentbox config path [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

##### config show

```text
$ agentbox config show --help
Print the built-in config template without reading it from disk

Usage: agentbox config show [OPTIONS]

Options:
      --config <PATH>     Path to an alternate config file [env: AGENTBOX_CONFIG=]
      --full              Show the fully commented example config
  -v, --verbose...        Increase logging verbosity (repeat for more detail)
      --container <NAME>  Operate on a specific agentbox container instead of the current project
  -h, --help              Print help
```

## Configuration

Agentbox resolves the YAML config path from the `--config PATH` flag, `AGENTBOX_CONFIG`, `$XDG_CONFIG_HOME/agentbox/config.yaml`, or `~/.config/agentbox/config.yaml`, in that order. A missing file is not an error: built-in defaults are used instead.

Only the values you want to change need to appear in YAML; missing fields use built-in defaults. Paths support `~` and environment variables. `shell` and `ocelot_path` are names searched under `<ai_env_path>/bin`; absolute paths are also accepted.

Mounts are read-only by default. Set `read_only: false` on an entry to make it writable; missing host paths are skipped with a warning.

### Minimal configuration example

```yaml
general:
  container_prefix: agentbox
  ai_env_path: ~/.local/share/agentbox-env
  default_agent: shell
  shell: zsh
  ocelot_path: ocelot

container:
  memory_limit: 32g
  read_only: true

mounts:
  - host: ~/.local/bin
    container: ~/.local/bin
    read_only: true
  - host: ~/.cache/my-agent
    container: ~/.cache/my-agent
    read_only: false

server:
  serve_port: 4096
  serve_host: 127.0.0.1
  listen_host: 127.0.0.1
  startup_timeout_secs: 15

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

`agentbox config init` writes a minimal file, `agentbox config init --full` writes the fully commented template with every setting and default mount, and `agentbox config show [--full]` prints either template to stdout without writing a file. Add `--force` to `config init` to overwrite an existing file.

### Environment variables

| Variable          | Description                                                                         |
| ----------------- | ----------------------------------------------------------------------------------- |
| `AGENTBOX_CONFIG` | Alternate config file path; equivalent to `--config PATH`.                          |
| `XDG_CONFIG_HOME` | Base directory for the default `agentbox/config.yaml` location.                     |
| `RUST_LOG`        | Overrides `-v`/`-vv` with a `tracing` filter directive.                             |
| `TERM`            | Terminal type forwarded to the container; falls back to `environment.default_term`. |

### Complete option reference

<details>
<summary>All YAML keys, defaults, and descriptions</summary>

| Key                            | Default                                                     | Description                                                                                      |
| ------------------------------ | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `general.ai_env_path`          | `~/.local/share/agentbox-env`                               | Nix-built AI environment mounted at `/run/ai-env`.                                               |
| `general.container_prefix`     | `agentbox`                                                  | Prefix for generated container, home, temp, and metadata names.                                  |
| `general.runtime_base_dir`     | `/tmp/agentbox`                                             | Host base for `<uid>-<name>-<hash>/{home,tmp,meta}`.                                             |
| `general.forbidden_dirs`       | `["/", "/tmp"]`                                             | Project prefixes `start` refuses; root is always rejected.                                       |
| `general.shell`                | `zsh`                                                       | Shell name under `<ai_env_path>/bin`; falls back to bash.                                        |
| `general.default_agent`        | `shell`                                                     | Agent used when no name is given.                                                                |
| `general.ocelot_path`          | `ocelot`                                                    | Supervisor name under `<ai_env_path>/bin`; missing supervisor falls back to `podman run --init`. |
| `general.podman_binary`        | `podman`                                                    | Podman executable used for all container operations.                                             |
| `container.hostname_prefix`    | `sandbox`                                                   | Prefix for the generated container hostname.                                                     |
| `container.workdir`            | `/workspace`                                                | Working directory inside the container.                                                          |
| `container.read_only`          | `true`                                                      | Mount the container root filesystem read-only.                                                   |
| `container.no_new_privileges`  | `true`                                                      | Prevent child processes from gaining new privileges.                                             |
| `container.drop_all_caps`      | `true`                                                      | Drop every Linux capability.                                                                     |
| `container.pids_limit`         | `1024`                                                      | Maximum number of container processes.                                                           |
| `container.memory_limit`       | `32g`                                                       | Container memory limit passed to Podman.                                                         |
| `container.memory_swap`        | `null`                                                      | Memory-plus-swap limit; null lets Podman choose.                                                 |
| `container.cpus`               | `null`                                                      | CPU quota; null disables the limit.                                                              |
| `container.ulimits`            | `core: "0"`, `nofile: "4096:8192"`                          | Additional container ulimits.                                                                    |
| `container.tmp_size`           | `2g`                                                        | Size of the `/tmp` tmpfs.                                                                        |
| `container.var_tmp_size`       | `2g`                                                        | Size of the `/var/tmp` tmpfs.                                                                    |
| `container.rootfs_overlay`     | `true`                                                      | Mount the generated rootfs as an overlay.                                                        |
| `container.inherit_proxy`      | `false`                                                     | Inherit host proxy environment variables.                                                        |
| `nix.enable_daemon_mount`      | `true`                                                      | Mount the daemon socket, profiles, and `/etc/nix` when available.                                |
| `nix.store_path`               | `/nix/store`                                                | Host Nix store, mounted read-only.                                                               |
| `nix.daemon_socket_dir`        | `/nix/var/nix/daemon-socket`                                | Host directory containing the daemon socket.                                                     |
| `nix.profiles_dir`             | `/nix/var/nix/profiles`                                     | Host Nix profiles directory.                                                                     |
| `nix.etc_nix_dir`              | `/etc/nix`                                                  | Host `/etc/nix` directory.                                                                       |
| `nix.mount_kernel_modules`     | `false`                                                     | Mount matching host kernel modules when available.                                               |
| `nix.kernel_modules_dir`       | `/run/current-system/kernel-modules/lib/modules`            | Host kernel module version directories.                                                          |
| `nix.kernel_modules_mount_dir` | `/lib/modules`                                              | Container destination for kernel modules.                                                        |
| `nix.host_config_dir`          | `~/.config/nix`                                             | Host Nix config directory; only an allowlist is copied into the container.                       |
| `nix.host_cache_dir`           | `~/.cache/nix`                                              | Host Nix cache directory created before startup.                                                 |
| `mounts`                       | 17 default entries                                          | List of `{ host, container, read_only }` bind mounts applied after the persistent home.          |
| `mounts[].host`                | none                                                        | Host source path; supports `~` and `$ENV`.                                                       |
| `mounts[].container`           | none                                                        | Container destination path; supports `~` and `$ENV`.                                             |
| `mounts[].read_only`           | `true`                                                      | Set to `false` for a writable mount.                                                             |
| `environment.path`             | `~/.local/bin:/run/ai-env/bin:/usr/bin:/bin`                | Container `PATH`.                                                                                |
| `environment.ssl_cert_file`    | `/run/ai-env/etc/ssl/certs/ca-bundle.crt`                   | Container `SSL_CERT_FILE` and `NIX_SSL_CERT_FILE`.                                               |
| `environment.cpath`            | `/run/ai-env/include`                                       | Container `CPATH`.                                                                               |
| `environment.library_path`     | `/run/ai-env/lib`                                           | Container `LIBRARY_PATH`.                                                                        |
| `environment.pkg_config_path`  | `/run/ai-env/lib/pkgconfig:/run/ai-env/share/pkgconfig`     | Container `PKG_CONFIG_PATH`.                                                                     |
| `environment.lang`             | `C.UTF-8`                                                   | Container `LANG` and `LC_ALL`.                                                                   |
| `environment.histfile`         | `~/.cache/zsh_history`                                      | Shell history file in the persistent home.                                                       |
| `environment.default_term`     | `xterm-256color`                                            | `TERM` when the host does not provide one.                                                       |
| `environment.nix_remote`       | `daemon`                                                    | Container `NIX_REMOTE`.                                                                          |
| `environment.extra_env`        | `{}`                                                        | Additional name/string environment variables.                                                    |
| `secrets.base_dir`             | `~/.config/sops-nix/secrets`                                | Host base directory for SOPS secret files.                                                       |
| `secrets.entries`              | `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY` | Environment variable name to secret path relative to `base_dir`.                                 |
| `server.serve_port`            | `4096`                                                      | Port passed to agent serve commands and attach clients.                                          |
| `server.serve_host`            | `127.0.0.1`                                                 | Host used for readiness probes inside the container.                                             |
| `server.listen_host`           | `127.0.0.1`                                                 | Address the server listens on; no host port is published.                                        |
| `server.startup_timeout_secs`  | `15`                                                        | Maximum readiness wait in seconds.                                                               |
| `server.poll_interval_ms`      | `250`                                                       | Delay between readiness probes in milliseconds.                                                  |
| `agents.<name>.command`        | none                                                        | Interactive command for a custom agent.                                                          |
| `agents.<name>.serve.command`  | none                                                        | Optional server command for the agent.                                                           |
| `agents.<name>.serve.attach`   | none                                                        | Optional attach command; the URL is appended.                                                    |
| `agents.opencode.command`      | `opencode`                                                  | Default OpenCode agent command.                                                                  |
| `agents.opencode.serve`        | `opencode serve` / `opencode attach`                        | Default OpenCode client/server commands.                                                         |
| `agents.claude.command`        | `claude --resume`                                           | Resume the latest native Claude session.                                                         |
| `agents.gemini.command`        | `gemini --resume`                                           | Resume the latest native Gemini session.                                                         |

</details>

`agentbox oc` waits for the container server to become ready and attaches to it from inside the container. No host port is published: the server listens on loopback and is reachable only from within the container.

`agentbox claude` and `agentbox gemini` use the provider CLI's native `--resume` flag to reopen the most recent session in the current project. Older provider versions may require at least one prior session before `--resume` works.

## Integration

### Shell completions

Completion scripts are generated with `agentbox completions <SHELL>` for `bash`, `elvish`, `fish`, `powershell`, and `zsh`:

```bash
agentbox completions bash > /etc/bash_completion.d/agentbox
agentbox completions zsh > ~/.zsh/completions/_agentbox
agentbox completions fish > ~/.config/fish/completions/agentbox.fish
```

Create the target directories first if they do not exist.

## Security considerations

- The YAML config, agent commands, and secret files are trusted host input: agentbox reads and expands them with host privileges before starting the container. Only load configs you control.
- No host port is published. The container server listens on loopback and is reached with `podman exec`; `server.listen_host` defaults to `127.0.0.1`.
- Secret values are written to a `0600` file under the per-project tmp dir and passed with `--env-file`. They are redacted from debug logs, but any process inside the container can still read its own environment, and a host user who can run `podman inspect` may see the env file path.
- AI provider config dirs are mounted read-only; only session/state subdirs and OpenCode's data dir (`~/.local/share/opencode`) are writable. A filtered copy of the host Nix config is mounted read-only: credential-bearing settings such as `access-tokens` and `secret-key-files` are never copied, and the real host directory is never exposed.
- `/tmp` and `/var/tmp` are size-limited tmpfs mounts with `nodev` and `mode=1777` so the non-root container user can write to them (`/var/tmp` also `noexec`). Writable bind mounts use `nosuid,nodev`. Kernel module mounts and proxy environment inheritance are disabled by default.
- Resource limits are defense in depth, not a hard security boundary: a writable bind mount can still consume host disk, and an untrusted agent with a host Nix daemon socket can ask the host daemon to build derivations. Disable `nix.enable_daemon_mount` when running untrusted code.
- On SELinux systems, additional bind mounts may require `z`/`Z` relabeling.

## How it works

1. `agentbox start` resolves the YAML config, canonicalizes the current project path, and hashes it with the host UID to derive stable container, hostname, home, and temporary-directory names.
2. A minimal rootfs skeleton is generated under the per-project `meta/rootfs` directory; the host Nix store and selected host paths are added as bind mounts, and secrets are passed through a temporary `--env-file`.
3. Podman starts the container without an application image. When `ocelot` is available it supervises the configured services; otherwise agentbox uses Podman's init process.
4. Server-backed agents such as OpenCode start inside the container and are probed for readiness on the container loopback address. Attach commands use `podman exec` and never publish a host port.
5. Interactive agents and shells run directly through `podman exec -it`.

## Architecture

| File                | Responsibility                                                       |
| ------------------- | -------------------------------------------------------------------- |
| `src/main.rs`       | Entrypoint, logging setup, config and completion fast paths.         |
| `src/cli/`          | Clap command definitions.                                            |
| `src/config/mod.rs` | YAML schema, defaults, path resolution, expansion, validation.       |
| `src/error.rs`      | SNAFU error and result types.                                        |
| `src/ocelot.rs`     | ocelot supervise config serialization.                               |
| `src/workspace.rs`  | Project identity, canonical path hashing, host identity.             |
| `src/rootfs.rs`     | Minimal rootfs skeleton, passwd/group files, symlinks.               |
| `src/podman.rs`     | Podman command construction, container lifecycle, readiness polling. |
| `src/app.rs`        | Mount/env planning, init scripts, ocelot config, session launching.  |

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo nextest run
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and commit conventions, and [conventions.md](conventions.md) for the Rust coding standards.

## License

GPL-3.0
