param(
  [string]$Distro = "Ubuntu",
  [string]$ProjectPath = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
  [string]$TestProject = "",
  [switch]$KeepTestStack
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

foreach ($name in @(
  "CODEX_API_KEY",
  "OPENAI_API_KEY",
  "OPENAI_API_BASE",
  "CODEX_API_BASE",
  "OPENAI_REFRESH_TOKEN",
  "OPENAI_AUTH_COOKIE",
  "CODEXMANAGER_AUTH_COOKIE",
  "OPENAI_SESSION_COOKIE"
)) {
  $value = [Environment]::GetEnvironmentVariable($name)
  if (-not [string]::IsNullOrWhiteSpace($value)) {
    throw "CLI child-key 验证禁止携带真实上游环境变量或 cookie: $name"
  }
}

$projectPathResolved = (Resolve-Path $ProjectPath).Path
$projectPathForWsl = $projectPathResolved -replace '\\', '/'
$wslProjectPath = (wsl.exe -d $Distro -- wslpath -a $projectPathForWsl).Trim()
if ([string]::IsNullOrWhiteSpace($wslProjectPath)) {
  throw "无法将项目路径转换为 WSL 路径: $projectPathResolved"
}

$verifyScriptWsl = "$wslProjectPath/scripts/tests/docker/verify_cli_child_key_stack.sh"

$wslArgs = New-Object System.Collections.Generic.List[string]
$wslArgs.Add("-d")
$wslArgs.Add($Distro)
$wslArgs.Add("--")
$wslArgs.Add("bash")
$wslArgs.Add($verifyScriptWsl)
if (-not [string]::IsNullOrWhiteSpace($TestProject)) {
  $wslArgs.Add("--test-project")
  $wslArgs.Add($TestProject)
}
if ($KeepTestStack) {
  $wslArgs.Add("--keep-test-stack")
}

& wsl.exe @wslArgs
exit $LASTEXITCODE
