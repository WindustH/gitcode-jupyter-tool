# gitcode-jupyter-tool

[中文文档](README.zh-CN.md)

`gitcode-jupyter-tool` provides Rust command-line tools for using the GitCode CANN online JupyterLab experience as a remote shell, plus local/remote file copy.

The project now builds four executables:

- `jud`: GitCode Jupyter Tool daemon. It keeps a usable notebook available and exposes a local HTTP API plus a low-latency TCP stream.
- `jush`: Jupyter shell client. It runs remote commands, local scripts, stdin scripts, or an interactive shell through `jud`.
- `jucp`: Jupyter copy client. It copies files or directories between local paths and `jupyter:` remote paths.
- `juctl`: daemon control tool for login, logout, start, stop, restart, reset, status, and resource inspection.

## Configuration

The default config directory is:

```bash
${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
```

By default, `jud` stores:

- Chrome profile: `${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache: `${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/auth.json`
- notebook state: `${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/state.json`

The local API, stream, and Chrome DevTools endpoints are selected per account at runtime:

```bash
JUD_CONFIG_DIR=${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
JUD_CACHE_DIR=${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool
JUD_LOG=/tmp/jud.log
JUPYTER_CWD=~
```

All accounts dynamically select free API/stream ports from `61000–61199` and
Chrome CDP ports from `61800–61999`; the selected endpoints are persisted per
account. Explicit `JUD_API_URL`, `JUD_STREAM_URL`, or `JUD_CDP_PORT` values can
still be used when integrating with an external layout.

Only the `JUD_*` environment names are supported.

## Build

```bash
cargo build --release
```

The binaries are written under `target/release/`:

```bash
target/release/jud
target/release/jush
target/release/jucp
target/release/juctl
```

## Prerequisites

- Linux.
- Google Chrome or a compatible Chrome browser. The default executable is `google-chrome-stable`; set `CHROME` to override it.
- Network access to `https://gitcode.com/cann/cann-learning-hub`.
- A GitCode account that can open the CANN online notebook experience.
- Local loopback ports available by default:
  - a free API/stream pair from `61000–61199`.
  - a free Chrome DevTools port from `61800–61999`.

## Usage

Log in or clear the dedicated GitCode login state:

```bash
juctl login
juctl logout
```

`juctl login` opens visible Chrome, waits for GitCode login, caches auth, and restarts `jud` if it was running. `juctl logout` stops `jud` and removes the auth cache, notebook state, and dedicated Chrome profile; use `juctl logout --keep-profile` to keep the Chrome profile.

Accounts are independent. Use `--account NAME` (or `JUD_ACCOUNT`) with `jud`,
`juctl`, `jush`, and `jucp`; each account gets its own auth/state/profile and
runtime-selected local API/stream ports:

```bash
juctl accounts list
juctl --account default login
juctl --account work login
juctl --account work start
jush --account work -c 'pwd'
```

The profile is disposable browser state: removing `${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool`
will require a new browser login, while the auth cache remains in the config directory.

The notebook NPU resources are shared across accounts. Do not add `--heavy` to
`jush` or `jucp` by default: it only serializes requests inside one `jud`
daemon and cannot isolate shared NPU resources. Timing from `jush` is not valid
performance evidence; use a dedicated isolated environment for benchmarking.

Start the daemon:

```bash
juctl start
```

Check daemon status and remote resources:

```bash
juctl status
juctl status --json
juctl resources --timeout 60
```

`juctl resources` probes the current notebook and returns CPU, memory, NPU, CANN/toolkit, disk, and system details as JSON; `npu-smi info` is parsed into structured device/process fields.

Stop, restart, or reset:

```bash
juctl stop
juctl restart
juctl reset
```

`juctl reset` resets the current notebook: it leaves a unique flag in every running kernel, shuts down all Jupyter kernels, closes notebook sessions and terminals on the remote Jupyter server (the standard `/api/kernels`, `/api/sessions`, `/api/terminals` endpoints), then reopens the notebook with a fresh kernel. After the reset it checks the new kernel and reports whether the flag is gone, so you can see that the reset really took effect (a fresh kernel no longer has the flag; `juctl reset` exits non-zero if the flag survived). If the notebook instance itself is gone, `jud` provisions a new one automatically. Use `juctl reset --timeout 60` to allow more time for kernel shutdown.

Run a remote interactive shell:

```bash
jush
```

Run a command:

```bash
jush -c 'pwd && npu-smi info'
```

Run a local shell script remotely:

```bash
jush ./remote-test.sh arg1 arg2
```

Read a script from stdin:

```bash
printf 'pwd\n' | jush -s
```

Use `JUPYTER_CWD` to set the remote working directory:

```bash
JUPYTER_CWD=/workspace/notebook1 jush -c pwd
```

Copy files:

```bash
jucp ./local.txt jupyter:/workspace/notebook1/local.txt
jucp jupyter:/workspace/notebook1/result.txt ./result.txt
jucp -r ./cases jupyter:/workspace/notebook1/cases
jucp -r jupyter:/workspace/notebook1/logs ./logs
```

Remote paths must start with `jupyter:`. Exactly one side must be local and exactly one side must be remote.

## Execution and performance note

Use ordinary commands for remote work:

```bash
jush --timeout 1800 -c 'cd /workspace/notebook1/work && bash build.sh && ./test'
jucp -r ./cases jupyter:/workspace/notebook1/cases
```

`--heavy` remains accepted for explicit daemon-local serialization, but it is
not needed for normal use and does not make NPU performance measurements valid.

## Direct daemon use

Run one maintenance pass:

```bash
jud --once
```

Probe only:

```bash
jud --status-only
```

Run the daemon in the foreground:

```bash
jud --interval 60
```

The daemon runs Chrome headless by default. If the dedicated profile is not logged in, `jud` opens a visible Chrome window for login unless `--no-login-window` is set. You can also force login refresh with `juctl login`. Force visible Chrome:

```bash
jud --visible
```

Do not expose the local `jud` API to untrusted networks.
