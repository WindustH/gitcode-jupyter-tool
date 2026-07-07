# gitcode-jupyter-tool

[English README](README.md)

`gitcode-jupyter-tool` 是一组 Rust 命令行工具，用来把 GitCode CANN 在线体验里的 JupyterLab notebook 当作远端 shell 使用，并支持本地和远端之间复制文件。

项目会输出四个可执行文件：

- `gjtd`：GitCode Jupyter Tool daemon，负责维护可用 notebook，并暴露本地 HTTP API 和低延迟 TCP stream。
- `jush`：Jupyter shell 客户端，支持交互式 shell、`-c` 命令、本地脚本、stdin 脚本。
- `jucp`：Jupyter copy 客户端，支持本地路径和 `jupyter:` 远端路径之间复制文件或目录。
- `gjtctl`：daemon 控制工具，支持 start、status、stop、restart。

## 配置目录

默认配置目录已经改为：

```bash
/home/windy/.config/gitcode-jupyter-tool
```

默认文件位置：

- Chrome profile：`/home/windy/.config/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache：`/home/windy/.config/gitcode-jupyter-tool/auth.json`
- notebook state：`/home/windy/.config/gitcode-jupyter-tool/state.json`

默认本地端口：

```bash
GJTD_API_URL=http://127.0.0.1:18787
GJTD_STREAM_URL=tcp://127.0.0.1:18788
GJTD_LOG=/tmp/gjtd.log
GJTD_CDP_PORT=9222
JUPYTER_CWD=~
```

为了兼容旧调用，原来的 `JUPYTERD_*` 环境变量名仍然会被读取。

## 构建

```bash
cargo build --release
```

输出文件：

```bash
target/release/gjtd
target/release/jush
target/release/jucp
target/release/gjtctl
```

## 使用要求

- Linux 环境。
- 已安装 Google Chrome 或兼容的 Chrome 浏览器；默认命令是 `google-chrome-stable`，也可以用环境变量 `CHROME` 指定。
- 能访问 `https://gitcode.com/cann/cann-learning-hub`。
- 有可登录 GitCode 的账号。首次使用时，如果专用 profile 未登录，`gjtd` 会打开可见 Chrome 窗口让你登录。
- 默认需要以下本地端口可用：
  - `127.0.0.1:18787`：`gjtd` HTTP API
  - `127.0.0.1:18788`：交互式 shell TCP stream
  - `127.0.0.1:9222`：Chrome DevTools

## 快速开始

启动 daemon：

```bash
gjtctl start
```

查看状态：

```bash
gjtctl status
gjtctl status --json
```

停止或重启：

```bash
gjtctl stop
gjtctl restart
```

进入交互式 shell：

```bash
jush
```

执行命令：

```bash
jush -c 'pwd && npu-smi info'
```

传递 bash 风格参数：

```bash
jush -c 'printf "%s %s\n" "$0" "$1"' name arg
```

执行本地脚本：

```bash
jush ./remote-test.sh arg1 arg2
```

从 stdin 读取脚本：

```bash
printf 'pwd\n' | jush -s
```

指定远端工作目录：

```bash
JUPYTER_CWD=/tmp jush -c pwd
```

## 复制文件

远端路径必须以 `jupyter:` 开头，并且每次复制必须正好一个本地路径、一个远端路径。

上传文件：

```bash
jucp ./local.txt jupyter:/workspace/notebook1/local.txt
jucp ./local.txt jupyter:local.txt
```

下载文件：

```bash
jucp jupyter:/workspace/notebook1/result.txt ./result.txt
jucp jupyter:result.txt ./result.txt
```

递归复制目录：

```bash
jucp -r ./cases jupyter:/workspace/notebook1/cases
jucp -r jupyter:/workspace/notebook1/logs ./logs
```

## 直接运行 daemon

运行一次维护：

```bash
gjtd --once
```

只检查状态：

```bash
gjtd --status-only
```

前台运行 daemon：

```bash
gjtd --interval 60
```

默认 headless 运行。如果专用 profile 没登录，`gjtd` 会临时打开可见 Chrome 登录窗口。强制可见窗口：

```bash
gjtd --visible
```

不要把本地 `gjtd` API 暴露到不可信网络。
