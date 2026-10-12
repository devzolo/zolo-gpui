#requires -Version 7.3
param(
    [string] $Zolo = 'zolo',
    [switch] $SkipInstall,
    [switch] $Async,
    [switch] $AsyncFailure
)

$ErrorActionPreference = 'Stop'
$library = Split-Path $PSScriptRoot -Parent
$cli = (Get-Command $Zolo -ErrorAction Stop).Source
$example = if ($AsyncFailure) { 'async_failure_smoke' } elseif ($Async) { 'async_smoke' } else { 'integration' }
$expected = if ($AsyncFailure) {
    'async event failure cancels siblings, drains cleanup and preserves the original error: ok'
} elseif ($Async) {
    'async completion, responsive rendering, Shutdown cancellation and cleanup: ok'
} else { @(
    'GPUI bridge: typed trees, fragments, events and Verniz children passed',
    'GPUI window: native click updated Zolo state'
) -join "`n" }

function Invoke-Zolo([string[]] $Arguments) {
    $output = @(& $cli @Arguments)
    if ($LASTEXITCODE -ne 0) { throw "Zolo failed: $($Arguments -join ' ')" }
    return $output
}

function Assert-Output([string[]] $Output, [string] $Label) {
    $actual = ($Output -join "`n").Trim()
    if ($actual -ne $expected) { throw "${Label}: unexpected output:`n$actual" }
    Write-Host "$Label passed"
}

Push-Location (Join-Path $library 'examples')
try {
    if (!$SkipInstall) { Invoke-Zolo -Arguments @('install', '--offline') | Write-Host }
    Assert-Output (Invoke-Zolo -Arguments @('run', "$example.zolo", '--no-cache')) 'VM and native window'
    foreach ($backend in @('native', 'llvm')) {
        $artifacts = Join-Path $library "target/verification/$backend"
        New-Item -ItemType Directory -Path $artifacts -Force | Out-Null
        $destination = Join-Path $artifacts $example
        Invoke-Zolo -Arguments @('build', "$example.zolo", '--emit', $backend, '--plugins', 'embed', '-o', $destination) | Write-Host

        # Only the executable is copied. No package source, Rust adapter or DLL
        # is installed in the deployment directory.
        $deployment = Join-Path $artifacts 'deployment'
        New-Item -ItemType Directory -Path $deployment -Force | Out-Null
        $executable = Join-Path $deployment "$example.exe"
        Copy-Item -LiteralPath "$destination.exe" -Destination $executable -Force
        $start = [System.Diagnostics.ProcessStartInfo]::new($executable)
        $start.WorkingDirectory = $deployment
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.Environment['PATH'] = "$env:SystemRoot/System32;$env:SystemRoot"
        foreach ($name in @('ZOLO_RUSTC', 'ZOLO_CARGO', 'ZOLO_CARGO_EXECUTABLE')) {
            $start.Environment[$name] = 'missing-compiler-for-source-free-gate'
        }
        foreach ($name in @('ZOLO_PLUGIN_DIR', 'ZOLO_STORE_DIR', 'ZOLO_FOREIGN_CACHE', 'ZOLO_FOREIGN_PREPARE_CACHE')) {
            $start.Environment[$name] = Join-Path $deployment "missing-$name"
        }
        $process = [System.Diagnostics.Process]::Start($start)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (!$process.WaitForExit(30000)) {
                $process.Kill($true)
                throw "$backend deployment did not close its window within 30 seconds"
            }
            if ($process.ExitCode -ne 0) { throw "$backend deployment failed: $($stderr.Result)" }
            Assert-Output ($stdout.Result.Trim() -split "\r?\n") "$backend source-free deployment and native window"
        } finally { $process.Dispose() }
    }
} finally { Pop-Location }
