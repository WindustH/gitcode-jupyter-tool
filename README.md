# jupyter-tool

[中文文档](README.zh-CN.md)

`jupyter-tool` contains small local utilities for working with a GitCode CANN JupyterLab notebook.

`jupyterd` is the daemon. It owns all Chrome DevTools and Jupyter terminal interaction, keeps a notebook available, and exposes a small local HTTP API.

It uses a dedicated Chrome profile at `~/.config/jupyter-tool/chrome-profile`. If that profile is already logged into GitCode, it reuses the login and can run headless. If it is not logged in, a visible Chrome window is opened so you can log in once; later runs reuse that dedicated profile.

`jupyter-sh` and `jupyter-cp` are thin clients. They only talk to `jupyterd`. Non-interactive commands and file copies are submitted as background jobs, and interactive shell traffic uses a persistent local TCP stream instead of one HTTP request per key press.

`jupyter-ctl` starts, stops, restarts, and checks the daemon.

## Prerequisites

- Linux.
- Python 3.10 or newer.
- Google Chrome or a compatible Chrome browser. The default executable is `google-chrome-stable`; set `CHROME` to override it.
- Network access to `https://gitcode.com/cann/cann-learning-hub`.
- A GitCode account that can open the CANN online notebook experience.
- Local loopback ports are available by default:
  - `127.0.0.1:18787` for the `jupyterd` HTTP API.
  - `127.0.0.1:18788` for the interactive shell TCP stream.
  - `127.0.0.1:9222` for Chrome DevTools.

No token or cookie export is required. `jupyterd` uses a dedicated Chrome profile at `~/.config/jupyter-tool/chrome-profile`; if that profile is not logged into GitCode yet, it opens a visible Chrome window for login. Do not expose the local `jupyterd` API to untrusted networks.

## Usage

Start the daemon:

```bash
./jupyterd --interval 60 --state-file /tmp/jupyterd-state.json
```

Or let the control script manage it:

```bash
./jupyter-ctl start
./jupyter-ctl status
./jupyter-ctl stop
./jupyter-ctl restart
```

Interactive shell through the daemon:

```bash
./jupyter-sh
```

Run a command:

```bash
./jupyter-sh -c 'pwd && npu-smi info'
```

`jupyter-sh` reads local `JUPYTER_CWD` before running remote commands. If it is unset, the remote working directory defaults to `~`.

```bash
JUPYTER_CWD=/workspace/notebook1 ./jupyter-sh -c pwd
```

Start the daemon automatically if needed:

```bash
./jupyter-sh --start-daemon -c 'pwd && npu-smi info'
```

Run a local shell script remotely. The script is copied to `/tmp`, executed, then removed:

```bash
./jupyter-sh ./remote-test.sh arg1 arg2
```

Read a script from stdin:

```bash
printf 'pwd\n' | ./jupyter-sh -s
```

Useful options:

```bash
JUPYTER_SH_TIMEOUT=600 ./jupyter-sh -c 'long-running-command'
```

Press `Ctrl-D` to exit the remote shell. `Ctrl-]` force-disconnects locally.

## Copy Files

Use `jupyter:` to mark the Jupyter side. Exactly one side must be local and exactly one side must be remote. `jupyter:/absolute/path` is remote absolute; `jupyter:relative/path` is relative to local `JUPYTER_CWD`, or remote `~` if `JUPYTER_CWD` is unset.

Upload a file:

```bash
./jupyter-cp ./local.txt jupyter:/workspace/notebook1/local.txt
./jupyter-cp ./local.txt jupyter:local.txt
./jupyter-cp --start-daemon ./local.txt jupyter:/workspace/notebook1/local.txt
```

Download a file:

```bash
./jupyter-cp jupyter:/workspace/notebook1/result.txt ./result.txt
```

Copy directory contents recursively:

```bash
./jupyter-cp -r ./cases jupyter:/workspace/notebook1/cases
./jupyter-cp -r jupyter:/workspace/notebook1/logs ./logs
```

Remote temporary payload files are created under `/tmp` and removed automatically.

## Keep Notebook Alive

`jupyterd` keeps a usable GitCode CANN notebook available. Credentials stay inside the dedicated Chrome profile; the script does not decrypt or print cookies/passwords.

Run one status check:

```bash
./jupyterd --status-only
```

Run one maintenance pass, creating a notebook from `https://gitcode.com/cann/cann-learning-hub` if needed:

```bash
./jupyterd --once
```

Run continuously as the local API daemon. It runs headless by default:

```bash
./jupyterd --interval 60 --state-file /tmp/jupyterd-state.json
```

By default the HTTP API listens on `http://127.0.0.1:18787`, and the low-latency interactive shell stream listens on `tcp://127.0.0.1:18788`.

Force a visible Chrome window:

```bash
./jupyterd --visible --interval 60 --state-file /tmp/jupyterd-state.json
```

If the dedicated profile is not logged in yet, headless `jupyterd` will open a visible Chrome window only for login. Disable that with `--no-login-window`.

If you want to open the login/profile window manually:

```bash
./jupyterd --visible --once
```
