# 部署

[English](deployment.md) · [**中文**](deployment-zh.md)

## 构建

前置：Rust 1.75+、Node.js（仅用于构建控制台）。Windows 上需要能找到 MSVC 链接器。
`build.ps1` 的查找顺序：`VCBUILD_DEVCMD` 环境变量（若已设置）→ `vswhere` 定位到的
Visual Studio → 在标准安装根目录下搜索 `vcvars64.bat` → `PATH` 上已有的 `link.exe`
（在 Developer Command Prompt 里即是这种情况）。传 `-NoDevCmd` 可完全跳过查找。

两个脚本每次都会重新构建前端再内嵌：

```powershell
.\build.ps1                 # Windows x64 -> dist\windows-x64
.\build.ps1 -DebugBuild
```

```bash
./build.sh                  # Linux x64 -> dist/linux-x64
./build.sh --musl           # 完全静态
```

只改 Rust 时直接调用 cargo，不要走这两个脚本：cargo 按 `web/out` 的现有内容重新内嵌，
不会重建前端。

```bash
cargo test --target x86_64-pc-windows-msvc   # Windows 见下方说明
cargo test                                   # Linux/macOS
```

44 个单元测试 + 30 个集成测试，均不访问网络。集成测试会拉起真实的路由进程，
配合本地 mock 上游。

Windows 注意：始终带上 `--target x86_64-pc-windows-msvc`（或从 Developer Command Prompt 运行）。
不加时 `PATH` 上的 x86 `link.exe` 会被选中，构建脚本报
`LNK2019: unresolved external symbol memcpy`。`build.ps1` 会自动处理这一点。

### 内嵌资源与增量构建

`src/webui.rs` 用 `include_dir!` 内嵌 `web/out`，`src/library.rs` 用 `include_str!` 内嵌
`assets/models.library.json`。两者都在编译期把文件内容烤进产物，但**不会向 cargo 注册依赖**。
因此 `build.rs` 会为每个内嵌文件发出 `cargo:rerun-if-changed`。没有它时，只改前端会让
`target/` 保持陈旧，二进制继续提供上一版控制台——这是**静默失败**：构建成功、进程行为正常，
只是内容是旧的。

因此构建脚本改为无条件重建 `web/out`。早先版本提供 `-NoWeb` / `--no-web`，靠与 `web/src`
比对时间戳放行；该开关已移除——守卫本身的代码量超过它省下的那次重建，而守卫一旦判断错，
就会发出陈旧控制台。

> **注意。** 现在构建脚本是第二个编译单元，裸跑 `cargo build` 可能因 `PATH` 上 x86 的
> `link.exe` 遮蔽 x64 版本而报 `LNK2019: unresolved external symbol memcpy`。`build.ps1`
> 始终传 `--target` 因而不会遇到；若直接调用 cargo，请加
> `--target x86_64-pc-windows-msvc`，或先导入 x64 开发环境。

## 产物结构

```
dist/windows-x64/
  ar-ocg-router.exe        单个 MSVC 二进制，rustls 静态，控制台已内嵌
  ar-ocg-router.exe.sha256
  config.yaml              可直接开跑的配置
  config.example.yaml      带完整注释的模板
  models.library.json      模型库模板（程序内同样内置一份）
  service.ps1              install / uninstall / start / stop / restart / status / logs / run
  install-service.cmd      一键安装（会弹 UAC）
  uninstall-service.cmd
  README.md

dist/linux-x64/
  ar-ocg-router            ELF x86-64，glibc，控制台已内嵌
  ar-ocg-router.sha256
  config.example.yaml
  models.library.json
  install.sh               install / uninstall / start / stop / restart / status / logs / check
  ar-ocg-router.service    systemd 单元模板（install.sh 会替换路径）
  README.md
```

## Windows 服务

原生 SCM 支持，不依赖 NSSM / WinSW 之类的包装器。服务名 `ar-ocg-router`，
显示名 `ar-OCG-Router`。

```powershell
.\service.ps1 install      # 安装 + 设为自动启动 + 立即启动（需要管理员，会弹 UAC）
.\service.ps1 status
.\service.ps1 logs
.\service.ps1 restart | stop
.\service.ps1 uninstall
.\service.ps1 run          # 前台运行，调试用
```

注册的命令行使用二进制、配置与日志的绝对路径，所以安装目录可以随意：

```
"<exe>\ar-ocg-router.exe" --service --config "<cfg>\config.yaml" --log-file "<exe>\ar-ocg-router.log"
```

| 参数 | 效果 |
| --- | --- |
| `--service-name <name>` | 服务名（默认 `ar-ocg-router`） |
| `--service-display-name <text>` | 显示名 |
| `--service-account <user>` | 以该账号运行（默认 `LocalSystem`）；配 `--service-password` |

服务自动启动，崩溃后按 5 秒 / 10 秒 / 30 秒重启，并把配置路径写进描述。
停止走 SCM STOP，进程优雅退出。

> **出网提醒。** 服务运行在 session 0，Windows 不会弹出「是否允许联网」对话框。
> 如果日志出现 `os error 10013 / WSAEACCES`，需要显式放行该二进制：
>
> ```powershell
> netsh advfirewall firewall add rule name="ar-OCG-Router out" dir=out action=allow ^
>   program="D:\path\ar-ocg-router.exe" enable=yes
> ```
>
> 部分机器上该策略**按二进制产物**生效，所以每次重新编译都可能需要重新放行一次。

## Linux systemd

```bash
sudo ./install.sh install       # 二进制装到 /usr/local/bin，配置到 /etc/ar-ocg-router
sudo ./install.sh status | logs | restart | stop
sudo ./install.sh check         # 真实上游自检（--selftest）
sudo ./install.sh uninstall     # 保留配置与日志
```

可用 `PREFIX`、`CONFIG_DIR`、`LOG_DIR` 覆盖路径。单元里已应用
`NoNewPrivileges`、`ProtectSystem=strict`、`ProtectHome`、`PrivateTmp`，
`ReadWritePaths` 只放开配置与日志目录。

## 仓库结构

```
src/            Rust 源码（按职责分模块：config、router、proxy、quota、api …）
tests/          集成测试（拉起真实路由进程配合 mock 上游）
web/            控制台源码；`web/out` 是构建时内嵌的静态导出
assets/         编译进二进制的数据（模型库）
deploy/         各平台的服务模板与安装脚本
docs/           本文档，英文为默认，`-zh` 为中文
dist/           打包产物（生成）
```
