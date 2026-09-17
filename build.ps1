<#
  build.ps1 - build a single-file ar-ocg-router binary for Windows x64

  Usage:
    .\build.ps1                    # release build, packaged into dist\windows-x64
    .\build.ps1 -DebugBuild        # debug build
    .\build.ps1 -Target x86_64-pc-windows-msvc -OutDir D:\dist

  The web console is rebuilt on every run and then embedded. To iterate on Rust only, skip this
  script and call cargo directly (cargo build needs no frontend rebuild once web/out exists).

  Toolchain: run from a Developer Command Prompt for VS, or let the script locate a Visual
  Studio installation itself. For a portable toolchain, point the VCBUILD_DEVCMD environment
  variable at its devcmd.ps1. Pass -NoDevCmd when link.exe is already on PATH.
#>
[CmdletBinding()]
param(
  [string]$Target = "x86_64-pc-windows-msvc",
  [string]$OutDir = "",
  [switch]$DebugBuild,
  [switch]$NoDevCmd
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# Adopt the environment produced by a cmd-syntax VS setup script (VsDevCmd.bat /
# vcvars64.bat): those emit KEY=VALUE lines and cannot be dot-sourced by PowerShell.
function Import-CmdEnvironment {
  param([Parameter(Mandatory = $true)][string]$Script)
  $lines = cmd /c "call `"$Script`" -arch=x64 >nul 2>&1 && set"
  foreach ($line in $lines) {
    if ($line -match "^([^=]+)=(.*)$") {
      if ($matches[1] -ieq "PATH") { $env:PATH = $matches[2] }
      else { Set-Item -Path ("env:" + $matches[1]) -Value $matches[2] -ErrorAction SilentlyContinue }
    }
  }
}

# A linker is only usable if its machine type matches the target. Read the PE header of
# link.exe itself: 0x8664 = x64, 0x14c = x86. Cheaper and more reliable than guessing from
# the path, which varies between Visual Studio layouts.
function Test-LinkArchitecture {
  param([Parameter(Mandatory = $true)][string]$Path)
  try {
    $fs = [System.IO.File]::OpenRead($Path)
    try {
      $br = New-Object System.IO.BinaryReader($fs)
      $fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
      $peOffset = $br.ReadInt32()
      $fs.Seek($peOffset + 4, [System.IO.SeekOrigin]::Begin) | Out-Null
      $machine = $br.ReadUInt16()
      return ($machine -eq 0x8664)
    } finally { $fs.Dispose() }
  } catch { return $false }
}

if (-not $NoDevCmd) {
  # Resolution order, most specific first. Nothing here is machine-specific:
  #   1. VCBUILD_DEVCMD env var  - explicit override (portable toolchains, CI)
  #   2. vswhere                 - the official VS locator, present on VS 2017+
  #   3. vcvars64.bat search     - fallback for installs without vswhere
  #   4. link.exe already on PATH
  $imported = $false

  if ($env:VCBUILD_DEVCMD -and (Test-Path $env:VCBUILD_DEVCMD)) {
    . $env:VCBUILD_DEVCMD | Out-Null
    Write-Host "imported VC environment: $env:VCBUILD_DEVCMD"
    $imported = $true
  }

  if (-not $imported) {
    $pf86 = [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")
    if ($pf86) {
      $vswhere = Join-Path $pf86 "Microsoft Visual Studio\Installer\vswhere.exe"
      if (Test-Path $vswhere) {
        $vsPath = & $vswhere -latest -products * -property installationPath 2>$null | Select-Object -First 1
        if ($vsPath) {
          $devcmd = Join-Path $vsPath "Common7\Tools\VsDevCmd.bat"
          if (Test-Path $devcmd) {
            Import-CmdEnvironment $devcmd
            Write-Host "imported VC environment via vswhere: $devcmd"
            $imported = $true
          }
        }
      }
    }
  }

  if (-not $imported) {
    $roots = @([Environment]::GetEnvironmentVariable("ProgramFiles"),
               [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")) |
      Where-Object { $_ } |
      ForEach-Object { Join-Path $_ "Microsoft Visual Studio" } |
      Where-Object { Test-Path $_ }
    foreach ($r in $roots) {
      $vcvars = Get-ChildItem $r -Recurse -Filter "vcvars64.bat" -ErrorAction SilentlyContinue |
        Select-Object -First 1
      if ($vcvars) {
        Import-CmdEnvironment $vcvars.FullName
        Write-Host "imported VC environment: $($vcvars.FullName)"
        $imported = $true
        break
      }
    }
  }

  if (-not $imported) {
    # Validate what we found. A bare "link.exe exists" check is not enough: an x86 linker on
    # PATH (or git's own link.exe) makes the build fail with a wall of LNK2019/LNK4272 errors
    # that look like a source problem. Only accept a linker whose machine type matches.
    $link = Get-Command link.exe -ErrorAction SilentlyContinue |
      Where-Object { $_.Source -notmatch '[\\/]\.git[\\/]' } |
      Select-Object -First 1
    if ($link -and (Test-LinkArchitecture $link.Source)) {
      Write-Host "using link.exe already on PATH: $($link.Source)"
    } elseif ($link) {
      # Fail here rather than letting cargo produce ~145 unresolved-external errors, which
      # look like a source problem and send people down the wrong path entirely.
      throw ("Found $($link.Source), but it is not an x64 linker. " +
             "Run this script from a Developer Command Prompt for VS, or set the " +
             "VCBUILD_DEVCMD environment variable to a devcmd.ps1 / vcvars64.bat.")
    } else {
      throw ("No MSVC toolchain found. Install Visual Studio Build Tools with the " +
             "Desktop development with C++ workload, or point the VCBUILD_DEVCMD " +
             "environment variable at a devcmd.ps1 / vcvars64.bat.")
    }
  }
}

# ---- web console ------------------------------------------------------------
# The console is compiled into the binary (include_dir), so the static export has to exist
# before cargo runs. It is rebuilt on every invocation: a guarded "is web/out stale?" check
# is exactly what let an older console ship once, and a correct guard is more code than just
# running the build. For Rust-only iteration use cargo directly, not this script.
$webDir = Join-Path $root "web"
$webOut = Join-Path $webDir "out"
if (-not (Test-Path (Join-Path $webDir "node_modules"))) {
  Write-Host "npm install (web/)"
  Push-Location $webDir
  npm install --no-audit --no-fund
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "npm install failed" }
  Pop-Location
}
Write-Host "npm run build (web/ -> web/out)"
Push-Location $webDir
npm run build
$webCode = $LASTEXITCODE
Pop-Location
if ($webCode -ne 0) { throw "web build failed" }
if (-not (Test-Path (Join-Path $webOut "index.html"))) {
  throw "web/out/index.html is missing after the web build"
}

$profile = if ($DebugBuild) { "debug" } else { "release" }
Write-Host "cargo build --profile $profile --target $Target"
cargo build --profile $profile --target $Target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = Join-Path $root "target\$Target\$profile\ar-ocg-router.exe"
if (-not (Test-Path $exe)) { throw "binary not found: $exe" }

if (-not $OutDir) { $OutDir = Join-Path $root "dist\windows-x64" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Copy-Item $exe (Join-Path $OutDir "ar-ocg-router.exe") -Force
Copy-Item (Join-Path $root "config.example.yaml") $OutDir -Force
# the model library ships as a starting point; the router also has it built in
Copy-Item (Join-Path $root "assets\models.library.json") $OutDir -Force
Copy-Item (Join-Path $root "README.md") $OutDir -Force
Copy-Item (Join-Path $root "README-zh.md") $OutDir -Force
# ship the documentation tree so the package is self-describing offline
Copy-Item (Join-Path $root "docs") $OutDir -Recurse -Force
# service registration scripts (Windows)
Copy-Item (Join-Path $root "deploy\windows\service.ps1") $OutDir -Force
Copy-Item (Join-Path $root "deploy\windows\install-service.cmd") $OutDir -Force
Copy-Item (Join-Path $root "deploy\windows\uninstall-service.cmd") $OutDir -Force

$hash = (Get-FileHash (Join-Path $OutDir "ar-ocg-router.exe") -Algorithm SHA256).Hash
"$hash  ar-ocg-router.exe" | Set-Content (Join-Path $OutDir "ar-ocg-router.exe.sha256") -Encoding ASCII

$size = [math]::Round((Get-Item (Join-Path $OutDir "ar-ocg-router.exe")).Length / 1MB, 2)
Write-Host ""
Write-Host "OK  $OutDir\ar-ocg-router.exe  ($size MB)"
Write-Host "SHA256 $hash"
Write-Host "Deploy  : put ar-ocg-router.exe + config.yaml in one directory, run .\ar-ocg-router.exe --selftest"
Write-Host "Service : install-service.cmd (or .\service.ps1 install) -> Windows service ar-ocg-router"
