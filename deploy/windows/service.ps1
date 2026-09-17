<#
  ar-OCG-Router 服务管理脚本（code name: ar-ocg-router）

  用法：
    .\service.ps1 install          安装为 Windows 服务（自动启动 + 崩溃自动重启，需要管理员）
    .\service.ps1 uninstall        停止并删除服务
    .\service.ps1 start|stop|restart|status
    .\service.ps1 logs             打印服务日志尾部
    .\service.ps1 run              前台运行（调试用，不注册服务）

  参数：
    -ConfigFile <path>   指定配置（默认：本脚本同目录的 config.yaml）
    -ServiceName <name>  服务名（默认 ar-ocg-router；显示名 ar-OCG-Router）
    -TailLines <n>       logs 动作打印的行数（默认 40）
    -NoElevate           不提权，直接用当前权限执行（权限不足时报错）

  安装时写入服务的命令行："<exe>" --service --config "<cfg>" --log-file "<log>"
  所以 exe / 配置 / 日志都可以放在任意本机目录。
#>
[CmdletBinding()]
param(
  [Parameter(Position = 0)]
  [ValidateSet('install','uninstall','start','stop','restart','status','logs','run')]
  [string]$Action = 'status',
  [string]$ConfigFile = '',
  [string]$ServiceName = 'ar-ocg-router',
  [int]$TailLines = 40,
  [switch]$NoElevate
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe = Join-Path $scriptDir 'ar-ocg-router.exe'
if (-not $ConfigFile) { $ConfigFile = Join-Path $scriptDir 'config.yaml' }
$logFile = Join-Path $scriptDir 'ar-ocg-router.log'

function Test-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Path $exe)) { throw "找不到 $exe（请把 service.ps1 与 ar-ocg-router.exe 放同一目录）" }

$needsAdmin = $Action -in @('install', 'uninstall')
if ($needsAdmin -and -not (Test-Admin)) {
  if ($NoElevate) { throw '该操作需要管理员权限：请右键『以管理员身份运行』' }
  Write-Host '需要管理员权限，正在请求提权（会弹 UAC）...' -ForegroundColor Yellow
  $hostExe = (Get-Process -Id $PID).Path
  $dq = [char]34
  $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ($dq + $PSCommandPath + $dq), $Action,
            '-ConfigFile', ($dq + $ConfigFile + $dq), '-ServiceName', ($dq + $ServiceName + $dq))
  $p = Start-Process -FilePath $hostExe -ArgumentList $argv -Verb RunAs -PassThru
  $p.WaitForExit()
  exit $p.ExitCode
}

switch ($Action) {
  'install' {
    if (-not (Test-Path $ConfigFile)) { throw "找不到配置文件 $ConfigFile（可从 config.example.yaml 复制一份填 key）" }
    $absCfg = (Resolve-Path $ConfigFile).Path
    & $exe --install-service --service-name $ServiceName --config $absCfg --log-file $logFile
    if ($LASTEXITCODE -ne 0) { throw "安装失败（退出码 $LASTEXITCODE）" }
    Start-Sleep -Milliseconds 500
    & $exe --service-start --service-name $ServiceName
    Start-Sleep -Seconds 2
    & $exe --service-status --service-name $ServiceName
  }
  'uninstall' { & $exe --uninstall-service --service-name $ServiceName }
  'start'     { & $exe --service-start --service-name $ServiceName }
  'stop'      { & $exe --service-stop --service-name $ServiceName }
  'restart'   { & $exe --service-restart --service-name $ServiceName }
  'status'    { & $exe --service-status --service-name $ServiceName }
  'logs' {
    if (-not (Test-Path $logFile)) { throw "还没有日志文件 $logFile" }
    Get-Content $logFile -Tail $TailLines
  }
  'run' {
    Write-Host "前台运行（Ctrl+C 退出）：$exe --config $ConfigFile" -ForegroundColor Cyan
    & $exe --config $ConfigFile
  }
}
