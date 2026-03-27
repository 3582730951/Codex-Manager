param(
    [string]$Distro = "Ubuntu",
    [string]$TestProject = "",
    [switch]$SkipLiveGateway,
    [switch]$KeepTestStack,
    [switch]$Promote,
    [switch]$SkipGitPush,
    [string]$ProdProject = "codexmanager"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-WslEnvArgs {
    $args = New-Object System.Collections.Generic.List[string]
    foreach ($name in @("CODEX_API_KEY", "OPENAI_API_KEY")) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value -and $value.Trim().Length -gt 0) {
            $args.Add("$name=$($value.Trim())")
        }
    }
    return $args
}

function Invoke-WslBash {
    param(
        [string]$DistroName,
        [string]$CommandLine,
        [System.Collections.Generic.List[string]]$EnvArgs
    )

    $wslArgs = New-Object System.Collections.Generic.List[string]
    $wslArgs.Add("-d")
    $wslArgs.Add($DistroName)
    $wslArgs.Add("--")
    if ($EnvArgs.Count -gt 0) {
        $wslArgs.Add("env")
        foreach ($envArg in $EnvArgs) {
            $wslArgs.Add($envArg)
        }
    }
    $wslArgs.Add("bash")
    $wslArgs.Add("-lc")
    $wslArgs.Add($CommandLine)

    & wsl @wslArgs
    return $LASTEXITCODE
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\\..")
$verifyScript = Join-Path $repoRoot "scripts\\tests\\docker\\verify_and_test_stack.sh"
$promoteScript = Join-Path $repoRoot "scripts\\tests\\docker\\promote_running_stack.sh"

$repoRootWsl = (& wsl -d $Distro -- wslpath -a $repoRoot.Path).Trim()
$verifyScriptWsl = (& wsl -d $Distro -- wslpath -a $verifyScript).Trim()
$promoteScriptWsl = (& wsl -d $Distro -- wslpath -a $promoteScript).Trim()
$wslEnvArgs = Get-WslEnvArgs

$verifyArgs = New-Object System.Collections.Generic.List[string]
$verifyArgs.Add("cd `"$repoRootWsl`"")
$verifyArgs.Add("&&")
$verifyArgs.Add("bash")
$verifyArgs.Add("`"$verifyScriptWsl`"")
if ($TestProject) {
    $verifyArgs.Add("--test-project")
    $verifyArgs.Add("`"$TestProject`"")
}
if ($SkipLiveGateway) {
    $verifyArgs.Add("--skip-live-gateway")
}
if ($KeepTestStack) {
    $verifyArgs.Add("--keep-test-stack")
}

$verifyExitCode = Invoke-WslBash -DistroName $Distro -CommandLine ($verifyArgs -join " ") -EnvArgs $wslEnvArgs
if ($verifyExitCode -ne 0) {
    throw "WSL verification failed"
}

if ($Promote) {
    $promoteArgs = New-Object System.Collections.Generic.List[string]
    $promoteArgs.Add("cd `"$repoRootWsl`"")
    $promoteArgs.Add("&&")
    $promoteArgs.Add("bash")
    $promoteArgs.Add("`"$promoteScriptWsl`"")
    $promoteArgs.Add("--project")
    $promoteArgs.Add("`"$ProdProject`"")
    if ($SkipGitPush) {
        $promoteArgs.Add("--skip-git-push")
    }

    $promoteExitCode = Invoke-WslBash -DistroName $Distro -CommandLine ($promoteArgs -join " ") -EnvArgs $wslEnvArgs
    if ($promoteExitCode -ne 0) {
        throw "WSL promotion failed"
    }
}
