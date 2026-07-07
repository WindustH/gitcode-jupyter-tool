# jupyter-tool

[English README](README.md)

`jupyter-tool` 是一组自用脚本，用来把 GitCode CANN 在线体验里的 JupyterLab notebook 当作远端 shell 使用，并支持本地和远端之间复制文件。

## 工具

`jupyterd` 是 daemon。它负责维护 Chrome DevTools 连接、创建或复用 GitCode CANN notebook，并暴露本地 API。

`jupyter-sh` 是类 bash 的远端 shell 客户端：

- 无参数进入交互式远端 shell
- `-c` 执行一段命令
- `-s` 从 stdin 读取脚本
- `script-file [args...]` 上传本地脚本到远端临时文件并执行

`jupyter-cp` 是类 cp 的文件复制工具，只允许两个地址，并且必须一边是本地、一边是远端 `jupyter:` 路径。

`jupyter-ctl` 用来启动、停止、重启和查看 `jupyterd`。

这些入口脚本可以通过符号链接放到任意目录执行；即使符号链接改名，也会按真实脚本路径找到同目录模块和 `jupyterd`。

## 使用要求

- Linux 环境。
- Python 3.10 或更高版本。
- 已安装 Google Chrome 或兼容的 Chrome 浏览器；默认命令是 `google-chrome-stable`，也可以用环境变量 `CHROME` 指定。
- 能访问 `https://gitcode.com/cann/cann-learning-hub`。
- 有可登录 GitCode 的账号。首次使用时，如果专用 profile 未登录，`jupyterd` 会打开可见 Chrome 窗口让你登录。
- 本地端口默认需要可用：
  - `127.0.0.1:18787`：`jupyterd` HTTP API
  - `127.0.0.1:18788`：交互式 shell 低延迟 TCP stream
  - `127.0.0.1:9222`：Chrome DevTools
- 工具会使用专用 Chrome profile：`~/.config/jupyter-tool/chrome-profile`。不会复用默认 Chrome profile，也不会导出或打印 cookie/password。
- 远端 notebook 由 GitCode CANN 在线体验提供。远端环境是否有 NPU、CANN、样例文件，取决于 GitCode 当前 notebook 实例。

## 快速开始

启动 daemon：

```bash
./jupyter-ctl start
```

查看状态：

```bash
./jupyter-ctl status
```

停止 daemon：

```bash
./jupyter-ctl stop
```

重启 daemon：

```bash
./jupyter-ctl restart
```

也可以直接运行 daemon：

```bash
./jupyterd --interval 60 --state-file /tmp/jupyterd-state.json
```

默认 headless 运行。如果专用 profile 没登录，daemon 会临时打开可见 Chrome 登录窗口。强制可见窗口：

```bash
./jupyterd --visible
```

## jupyter-sh

进入交互式 shell：

```bash
./jupyter-sh
```

交互式 shell 默认有彩色短 prompt，只显示路径和 `$`，并内置：

```bash
alias ll="ls -l --color=always"
```

退出交互式 shell：

- `Ctrl-D`：像 ssh 一样退出远端 shell，并关闭本地连接。
- `Ctrl-]`：本地强制断开。正常情况下 daemon 仍会清理远端 Jupyter terminal。

执行命令：

```bash
./jupyter-sh -c 'pwd && npu-smi info'
```

传递 bash 风格参数：

```bash
./jupyter-sh -c 'printf "%s %s\n" "$0" "$1"' name arg
```

执行本地脚本：

```bash
./jupyter-sh ./remote-test.sh arg1 arg2
```

从 stdin 读取脚本：

```bash
printf 'pwd\n' | ./jupyter-sh -s
```

命令超时：

```bash
JUPYTER_SH_TIMEOUT=600 ./jupyter-sh -c 'long-running-command'
```

非交互输出默认会去掉 ANSI 控制序列，便于脚本处理。需要保留颜色时加 `--raw`：

```bash
./jupyter-sh --raw -c 'ls --color=always'
```

## 工作目录

`jupyter-sh` 和 `jupyter-cp` 会读取本地环境变量 `JUPYTER_CWD`。

如果未设置，远端默认工作目录是 `~`。

```bash
./jupyter-sh -c pwd
```

指定远端工作目录：

```bash
JUPYTER_CWD=/tmp ./jupyter-sh -c pwd
```

注意：子进程不能修改父 shell 的环境变量，所以这些脚本只读取 `JUPYTER_CWD`，不会尝试写回本地 shell。

## jupyter-cp

远端路径必须以 `jupyter:` 开头，并且每次复制必须正好一个本地路径、一个远端路径。

- `jupyter:/absolute/path`：远端绝对路径。
- `jupyter:relative/path`：相对本地 `JUPYTER_CWD`；如果本地未设置 `JUPYTER_CWD`，则相对远端 `~`。

上传文件：

```bash
./jupyter-cp ./local.txt jupyter:/workspace/notebook1/local.txt
./jupyter-cp ./local.txt jupyter:local.txt
```

下载文件：

```bash
./jupyter-cp jupyter:/workspace/notebook1/result.txt ./result.txt
./jupyter-cp jupyter:result.txt ./result.txt
```

递归复制目录：

```bash
./jupyter-cp -r ./cases jupyter:/workspace/notebook1/cases
./jupyter-cp -r jupyter:/workspace/notebook1/logs ./logs
```

旧的 `remote:` 和 `:` 前缀不支持。

## 常用端口和环境变量

```bash
JUPYTERD_API_URL=http://127.0.0.1:18787
JUPYTERD_STREAM_URL=tcp://127.0.0.1:18788
JUPYTERD_LOG=/tmp/jupyterd.log
JUPYTERD_CHROME_PROFILE_DIR=~/.config/jupyter-tool/chrome-profile
JUPYTERD_CDP_PORT=9222
JUPYTER_CWD=~
```

## 安全说明

工具通过本地 Chrome DevTools 操作已登录的 GitCode 页面。GitCode 凭据保存在专用 Chrome profile 中，脚本不会解密、导出或打印 cookie/password。

本地 API 默认只监听 `127.0.0.1`。不要把 `jupyterd` 监听地址暴露到不可信网络。
