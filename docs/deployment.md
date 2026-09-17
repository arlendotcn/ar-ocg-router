# Deployment

[**English**](deployment.md) · [中文](deployment-zh.md)

## Build

Requirements: Rust 1.75+ and Node.js (only to build the console). On Windows the MSVC linker
must be reachable. `build.ps1` resolves it in this order: the `VCBUILD_DEVCMD` environment
variable if set, then the Visual Studio installation located by `vswhere`, then a
`vcvars64.bat` search under the standard install roots, and finally `link.exe` already on
`PATH` (which is the case inside a Developer Command Prompt). Pass `-NoDevCmd` to skip the
search entirely.

Both scripts rebuild the web console on every run, then embed it:

```powershell
.\build.ps1                 # Windows x64 -> dist\windows-x64
.\build.ps1 -DebugBuild
```

```bash
./build.sh                  # Linux x64 -> dist/linux-x64
./build.sh --musl           # fully static
```

To iterate on Rust only, skip these scripts and call cargo directly; that re-embeds whatever
`web/out` already holds and does not rebuild the frontend.

```bash
cargo test --target x86_64-pc-windows-msvc   # Windows: see the note below
cargo test                                   # Linux/macOS
```

44 unit tests and 30 integration tests, none of which touch the network. The integration tests
start a real router process against local mock upstreams.

Windows note: always pass `--target x86_64-pc-windows-msvc` (or run from a Developer Command
Prompt). Without it an x86 `link.exe` on `PATH` will be picked up and the build script fails
with `LNK2019: unresolved external symbol memcpy`. `build.ps1` handles this automatically.

### Embedded assets and incremental builds

`src/webui.rs` embeds `web/out` with `include_dir!`, and `src/library.rs` embeds
`assets/models.library.json` with `include_str!`. Both bake file contents at compile time but
register no cargo dependency of their own, so a build script (`build.rs`) emits
`cargo:rerun-if-changed` for every embedded file. Without it, changing only the frontend leaves
`target/` stale and the binary keeps serving the previous console - a silent failure, because
the build succeeds and the process behaves normally.

The build scripts therefore rebuild `web/out` unconditionally. An earlier revision offered a
`-NoWeb` / `--no-web` flag guarded by a timestamp comparison against `web/src`; the flag is gone
because the guard was more code than the rebuild it avoided, and getting it wrong is how a stale
console ships.

> **Note.** A bare `cargo build` can fail with `LNK2019: unresolved external symbol memcpy`
> when an x86 `link.exe` shadows the x64 one on `PATH` (now that the build script is a second
> compilation unit). `build.ps1` avoids this by always passing `--target`; if you invoke cargo
> directly, pass `--target x86_64-pc-windows-msvc` or source the x64 dev environment first.

## Package layout

```
dist/windows-x64/
  ar-ocg-router.exe        single MSVC binary, rustls static, console embedded
  ar-ocg-router.exe.sha256
  config.yaml              ready-to-run configuration
  config.example.yaml      fully commented template
  models.library.json      model library template (built into the binary as well)
  service.ps1              install / uninstall / start / stop / restart / status / logs / run
  install-service.cmd      one-click install (raises UAC)
  uninstall-service.cmd
  README.md

dist/linux-x64/
  ar-ocg-router            ELF x86-64, glibc, console embedded
  ar-ocg-router.sha256
  config.example.yaml
  models.library.json
  install.sh               install / uninstall / start / stop / restart / status / logs / check
  ar-ocg-router.service    systemd unit template (install.sh substitutes paths)
  README.md
```

## Windows service

Native SCM support; no NSSM or WinSW wrapper. Service name `ar-ocg-router`, display name
`ar-OCG-Router`.

```powershell
.\service.ps1 install      # install, set auto-start, start now (needs admin, raises UAC)
.\service.ps1 status
.\service.ps1 logs
.\service.ps1 restart | stop
.\service.ps1 uninstall
.\service.ps1 run          # foreground, for debugging
```

The registered command line uses absolute paths for the binary, the config and the log, so
the install directory can be anywhere:

```
"<exe>\ar-ocg-router.exe" --service --config "<cfg>\config.yaml" --log-file "<exe>\ar-ocg-router.log"
```

| Option | Effect |
| --- | --- |
| `--service-name <name>` | Service name (default `ar-ocg-router`) |
| `--service-display-name <text>` | Display name |
| `--service-account <user>` | Run as this account (default `LocalSystem`); pair with `--service-password` |

The service auto-starts, restarts after crashes at 5 s / 10 s / 30 s, and records the config
path in its description. Stopping goes through SCM STOP and the process exits gracefully.

> **Outbound access.** A service runs in session 0, so Windows never shows the "allow network
> access" prompt. If the log shows `os error 10013 / WSAEACCES`, allow the binary explicitly:
>
> ```powershell
> netsh advfirewall firewall add rule name="ar-OCG-Router out" dir=out action=allow ^
>   program="D:\path\ar-ocg-router.exe" enable=yes
> ```
>
> On some machines this policy is keyed to the exact binary, so a freshly compiled executable
> needs the rule added again.

## Linux systemd

```bash
sudo ./install.sh install       # binary to /usr/local/bin, config to /etc/ar-ocg-router
sudo ./install.sh status | logs | restart | stop
sudo ./install.sh check         # live upstream self-test (--selftest)
sudo ./install.sh uninstall     # keeps config and logs
```

Override paths with `PREFIX`, `CONFIG_DIR` and `LOG_DIR`. The unit already applies
`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome` and `PrivateTmp`, with
`ReadWritePaths` limited to the config and log directories.

## Repository layout

```
src/            Rust sources (one module per concern: config, router, proxy, quota, api, ...)
tests/          integration tests (spawn a real router against mock upstreams)
web/            console sources; `web/out` is the static export embedded at build time
assets/         data compiled into the binary (model library)
deploy/         service templates and installers per platform
docs/           this documentation, English default with `-zh` translations
dist/           packaged build output (generated)
```
