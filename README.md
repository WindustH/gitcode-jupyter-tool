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

- Chrome profile: `${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache: `${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/auth.json`
- notebook state: `${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/state.json`

The local API and stream defaults are unchanged:

```bash
JUD_CONFIG_DIR=${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
JUD_API_URL=http://127.0.0.1:18787
JUD_STREAM_URL=tcp://127.0.0.1:18788
JUD_LOG=/tmp/jud.log
JUD_CDP_PORT=9222
JUPYTER_CWD=~
```

The old `GJTD_*` and `JUPYTERD_*` environment names are still accepted for compatibility.

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
  - `127.0.0.1:18787` for the `jud` HTTP API.
  - `127.0.0.1:18788` for the interactive shell TCP stream.
  - `127.0.0.1:9222` for Chrome DevTools.

## Usage

Log in or clear the dedicated GitCode login state:

```bash
juctl login
juctl logout
```

`juctl login` opens visible Chrome, waits for GitCode login, caches auth, and restarts `jud` if it was running. `juctl logout` stops `jud` and removes the auth cache, notebook state, and dedicated Chrome profile; use `juctl logout --keep-profile` to keep the Chrome profile.

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

## Heavy workload queue

Long builds, tests, profiling runs, and large copies can be marked heavy:

```bash
jush --heavy --timeout 1800 -c 'cd /workspace/notebook1/work && bash build.sh && ./test'
jucp --heavy -r ./cases jupyter:/workspace/notebook1/cases
```

Heavy requests are queued by `jud` and run one at a time in submission order. Normal non-heavy commands are not blocked by the heavy queue. `juctl status` shows the current heavy queue state.

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
