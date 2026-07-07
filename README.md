# gitcode-jupyter-tool

[中文文档](README.zh-CN.md)

`gitcode-jupyter-tool` provides Rust command-line tools for using the GitCode CANN online JupyterLab experience as a remote shell, plus local/remote file copy.

The project now builds four executables:

- `gjtd`: GitCode Jupyter Tool daemon. It keeps a usable notebook available and exposes a local HTTP API plus a low-latency TCP stream.
- `jush`: Jupyter shell client. It runs remote commands, local scripts, stdin scripts, or an interactive shell through `gjtd`.
- `jucp`: Jupyter copy client. It copies files or directories between local paths and `jupyter:` remote paths.
- `gjtctl`: daemon control tool for start, stop, restart, and status.

## Configuration

The default config directory is:

```bash
/home/windy/.config/gitcode-jupyter-tool
```

By default, `gjtd` stores:

- Chrome profile: `/home/windy/.config/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache: `/home/windy/.config/gitcode-jupyter-tool/auth.json`
- notebook state: `/home/windy/.config/gitcode-jupyter-tool/state.json`

The local API and stream defaults are unchanged:

```bash
GJTD_API_URL=http://127.0.0.1:18787
GJTD_STREAM_URL=tcp://127.0.0.1:18788
GJTD_LOG=/tmp/gjtd.log
GJTD_CDP_PORT=9222
JUPYTER_CWD=~
```

The old `JUPYTERD_*` environment names are still accepted for compatibility.

## Build

```bash
cargo build --release
```

The binaries are written under `target/release/`:

```bash
target/release/gjtd
target/release/jush
target/release/jucp
target/release/gjtctl
```

## Prerequisites

- Linux.
- Google Chrome or a compatible Chrome browser. The default executable is `google-chrome-stable`; set `CHROME` to override it.
- Network access to `https://gitcode.com/cann/cann-learning-hub`.
- A GitCode account that can open the CANN online notebook experience.
- Local loopback ports available by default:
  - `127.0.0.1:18787` for the `gjtd` HTTP API.
  - `127.0.0.1:18788` for the interactive shell TCP stream.
  - `127.0.0.1:9222` for Chrome DevTools.

## Usage

Start the daemon:

```bash
gjtctl start
```

Check status:

```bash
gjtctl status
gjtctl status --json
```

Stop or restart:

```bash
gjtctl stop
gjtctl restart
```

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

## Direct daemon use

Run one maintenance pass:

```bash
gjtd --once
```

Probe only:

```bash
gjtd --status-only
```

Run the daemon in the foreground:

```bash
gjtd --interval 60
```

The daemon runs Chrome headless by default. If the dedicated profile is not logged in, `gjtd` opens a visible Chrome window for login unless `--no-login-window` is set. Force visible Chrome:

```bash
gjtd --visible
```

Do not expose the local `gjtd` API to untrusted networks.
