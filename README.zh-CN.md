# gitcode-jupyter-tool

[English README](README.md)

`gitcode-jupyter-tool` 是一组 Rust 命令行工具，用来把 GitCode CANN 在线体验里的 JupyterLab notebook 当作远端 shell 使用，并支持本地和远端之间复制文件。

项目会输出四个可执行文件：

- `jud`：GitCode Jupyter Tool daemon，负责维护可用 notebook，并暴露本地 HTTP API 和低延迟 TCP stream。
- `jush`：Jupyter shell 客户端，支持交互式 shell、`-c` 命令、本地脚本、stdin 脚本。
- `jucp`：Jupyter copy 客户端，支持本地路径和 `jupyter:` 远端路径之间复制文件或目录。
- `juctl`：daemon 控制工具，支持 login、logout、start、status、stop、restart 和资源探测。

## 配置目录

默认配置目录已经改为：

```bash
${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
```

默认文件位置：

- Chrome profile：`${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache：`${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/auth.json`
- notebook state：`${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/state.json`

默认本地端口：

```bash
JUD_CONFIG_DIR=${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
JUD_API_URL=http://127.0.0.1:18787
JUD_STREAM_URL=tcp://127.0.0.1:18788
JUD_LOG=/tmp/jud.log
JUD_CDP_PORT=9222
JUPYTER_CWD=~
```

为了兼容旧调用，原来的 `GJTD_*` 和 `JUPYTERD_*` 环境变量名仍然会被读取。

## 构建

```bash
cargo build --release
```

输出文件：

```bash
target/release/jud
target/release/jush
target/release/jucp
target/release/juctl
```

## 使用要求

- Linux 环境。
- 已安装 Google Chrome 或兼容的 Chrome 浏览器；默认命令是 `google-chrome-stable`，也可以用环境变量 `CHROME` 指定。
- 能访问 `https://gitcode.com/cann/cann-learning-hub`。
- 有可登录 GitCode 的账号。首次使用时，如果专用 profile 未登录，`jud` 会打开可见 Chrome 窗口让你登录。
- 默认需要以下本地端口可用：
  - `127.0.0.1:18787`：`jud` HTTP API
  - `127.0.0.1:18788`：交互式 shell TCP stream
  - `127.0.0.1:9222`：Chrome DevTools

## 快速开始

登录或清理专用 GitCode 登录状态：

```bash
juctl login
juctl logout
```

`juctl login` 会打开可见 Chrome，等待你完成 GitCode 登录，缓存 auth；如果 `jud` 原本在运行，登录成功后会自动重启。`juctl logout` 会停止 `jud`，删除 auth cache、notebook state 和专用 Chrome profile；如果想保留 Chrome profile，可用 `juctl logout --keep-profile`。

启动 daemon：

```bash
juctl start
```

查看 daemon 状态和远端资源：

```bash
juctl status
juctl status --json
juctl resources --timeout 60
```

`juctl resources` 会探测当前 notebook，并以 JSON 输出 CPU、内存、NPU、CANN/toolkit、磁盘和系统信息；其中 `npu-smi info` 会被解析成结构化字段。

停止或重启：

```bash
juctl stop
juctl restart
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

## Heavy 任务队列

长时间构建、测试、profiling 和大文件复制可以标记为 heavy：

```bash
jush --heavy --timeout 1800 -c 'cd /workspace/notebook1/work && bash build.sh && ./test'
jucp --heavy -r ./cases jupyter:/workspace/notebook1/cases
```

`jud` 会把 heavy 请求放进队列，并按提交顺序一次只执行一个。普通非 heavy 命令不会被 heavy 队列阻塞。`juctl status` 会显示当前 heavy 队列状态。

## 直接运行 daemon

运行一次维护：

```bash
jud --once
```

只检查状态：

```bash
jud --status-only
```

前台运行 daemon：

```bash
jud --interval 60
```

默认 headless 运行。如果专用 profile 没登录，`jud` 会临时打开可见 Chrome 登录窗口。也可以用 `juctl login` 强制刷新登录。强制可见窗口：

```bash
jud --visible
```

不要把本地 `jud` API 暴露到不可信网络。
