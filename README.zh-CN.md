# gitcode-jupyter-tool

[English README](README.md)

`gitcode-jupyter-tool` 是一组 Rust 命令行工具，用来把 GitCode CANN 在线体验里的 JupyterLab notebook 当作远端 shell 使用，并支持本地和远端之间复制文件。

项目会输出四个可执行文件：

- `jud`：GitCode Jupyter Tool daemon，负责维护可用 notebook，并暴露本地 HTTP API 和低延迟 TCP stream。
- `jush`：Jupyter shell 客户端，支持交互式 shell、`-c` 命令、本地脚本、stdin 脚本。
- `jucp`：Jupyter copy 客户端，支持本地路径和 `jupyter:` 远端路径之间复制文件或目录。
- `juctl`：daemon 控制工具，支持 login、logout、start、status、stop、restart、reset 和资源探测。

## 配置目录

默认配置目录已经改为：

```bash
${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
```

默认文件位置：

- Chrome profile：`${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool/chrome-profile`
- GitCode auth cache：`${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/auth.json`
- notebook state：`${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool/state.json`

本地 API、stream 和 Chrome DevTools 端口都按账号运行时动态选择：

```bash
JUD_CONFIG_DIR=${XDG_CONFIG_HOME:-~/.config}/gitcode-jupyter-tool
JUD_CACHE_DIR=${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool
JUD_LOG=/tmp/jud.log
JUPYTER_CWD=~
```

所有账号都会在 `61000–61199` 中动态选择 API/stream 端口，在 `61800–61999` 中动态
选择 Chrome CDP 端口，并按账号保存选择结果。需要接入外部端口布局时，可以显式设置
`JUD_API_URL`、`JUD_STREAM_URL` 或 `JUD_CDP_PORT`。

只支持 `JUD_*` 环境变量名。

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
- 默认需要端口池中有可用端口：API/stream 使用 `61000–61199`，Chrome DevTools 使用
  `61800–61999`。

## 快速开始

登录或清理专用 GitCode 登录状态：

```bash
juctl accounts list
juctl --account default login
juctl login
juctl logout
```

`juctl login` 会打开可见 Chrome，等待你完成 GitCode 登录，缓存 auth；如果 `jud` 原本在运行，登录成功后会自动重启。`juctl logout` 会停止 `jud`，删除 auth cache、notebook state 和专用 Chrome profile；如果想保留 Chrome profile，可用 `juctl logout --keep-profile`。

多个账号可以并存。给 `jud`、`juctl`、`jush`、`jucp` 加上
`--account NAME`（或设置 `JUD_ACCOUNT`），每个账号会使用独立的 auth/state/profile，
并在运行时选择独立的本地 API/stream 端口：

```bash
juctl --account work login
juctl --account work start
jush --account work -c 'pwd'
```

Chrome profile 属于可清理的浏览器状态；删除 `${XDG_CACHE_HOME:-~/.cache}/gitcode-jupyter-tool`
后只需重新登录，配置目录中的 auth cache 不会随之删除。

Notebook 的 NPU 资源会被多个账号共享。默认不要给 `jush` 或 `jucp` 添加
`--heavy`：它最多只会在单个 `jud` daemon 内串行请求，不能隔离共享的 NPU 资源。
因此 `jush` 的耗时不能作为性能依据；需要性能测试时应使用独立隔离的环境。

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

停止、重启或重置 notebook：

```bash
juctl stop
juctl restart
juctl reset
```

`juctl reset` 会重置当前 notebook：先在每个运行中的 kernel 里留下唯一 flag，然后关闭远端 Jupyter 服务器上所有的 kernel（通过标准 `/api/kernels`、`/api/sessions`、`/api/terminals` 接口），关闭 notebook session 和终端，再重新打开 notebook 并启动全新 kernel。重置完成后会检查新 kernel 里的 flag 是否已消失并输出结果，用于确认 reset 确实生效（新 kernel 里 flag 已不在；如果 flag 还在，`juctl reset` 会以非零码退出）。如果 notebook 实例本身已经失效，`jud` 会自动新建一个。kernel 关闭耗时较长时可用 `juctl reset --timeout 60` 调整。

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

## 执行和性能说明

远端工作直接使用普通命令：

```bash
jush --timeout 1800 -c 'cd /workspace/notebook1/work && bash build.sh && ./test'
jucp -r ./cases jupyter:/workspace/notebook1/cases
```

`--heavy` 仍可用于显式要求单个 daemon 内串行执行，但普通使用不需要添加，
也不能让 NPU 性能测试变得有效。

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
