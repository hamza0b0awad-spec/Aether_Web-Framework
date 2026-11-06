#Requires -Version 5.1
<#
.SYNOPSIS
    Builds an Aether binary with profile-guided optimisation (PGO) on this Windows/MSVC toolchain.

.DESCRIPTION
    PGO is three passes over the same source, and this script is the driver for all three:

      1. generate - build with -Cprofile-generate. The binary is instrumented: it counts which
                    branches it actually takes and writes that into *.profraw files.
      2. train    - produce the profile. -Action train does this end to end with the in-process
                    training run (framework/tests/examples/pgo_train.rs): it serves the same routes
                    as basic_api, loads them, shuts down and returns from main, so the counters are
                    flushed without anything outside the process.
      3. merge    - llvm-profdata turns the *.profraw files into one merged.profdata, and reports
                    what that profile actually contains.
      4. use      - build again with -Cprofile-use. This binary is the PGO artifact.

    Alternative for step 2: run the instrumented server itself under real load and stop it with
    Ctrl+C. That works only if the process is attached to a console it can receive the key in, and
    only because Aether's Ctrl+C path returns from main; a detached server (e.g. started with
    `cmd start /affinity`) has no such console, so it must be killed - and a killed process trains
    nothing. -Action train exists so the route does not depend on that.

    Four properties of this toolchain that the steps below encode, verified on rustc 1.98.1 / MSVC
    on 2026-10-25 with an instrumented scratch binary and four different endings:

      * Counters flush when the process RETURNS FROM MAIN - including while other threads are still
        running (1 profraw). They do NOT flush when it ends any other way: std::process::exit(0)
        reached mid-work, Stop-Process -Force, and taskkill without /F each produced 0 files. So a
        clean exit code from the run is not enough; the run has to reach the end of main. `merge`
        refusing with exit 2 on an empty directory is usually this.
      * A profraw file appearing is not proof of a profile. `llvm-profdata show` is run after merge,
        and its "Total functions" / "Maximum function count" are reported; a merged profile with 0
        functions describes nothing and is refused before -Action use will build with it.
      * RUSTFLAGS applies to build scripts as well as to the crate, so a generate pass also leaves
        profraw files from `build.rs` runs. `merge -sparse` folds them in harmlessly; the functions
        they describe are not in the final binary and are ignored.
      * A flag rustc does not know in RUSTFLAGS makes cargo fail before it compiles anything, at
        its target-information query. Only stable -C flags appear here, and adding a -Z one will
        produce that confusing error rather than a build.

    What this script never does: measure. It produces two binaries and tells you how to compare
    them. The comparison is release-vs-release, same work, repeated, in one session, on the owner's
    machine.

.PARAMETER Action
    generate | train | merge | use | status.

.PARAMETER Bin
    The cargo bin target to build. basic_api is the pinned-one-core ceiling server (it calls
    AetherApp::run, so AETHER_WORKERS applies to it).

.PARAMETER Package
    The cargo package that owns $Bin, passed as -p. Needed for any bin outside the workspace's
    default-members (examples): sched_compare lives in aether-benchmarks.

.PARAMETER CountersPerSite
    Value-profile counters LLVM allocates per instrumentation site, passed as
    -Cllvm-args=-vp-counters-per-site=n. The default is not enough here: the instrumented
    sched_compare run printed "LLVM Profile Warning: Unable to track new values: Running out of
    static counters" twelve times, i.e. part of the profile was never recorded. 64 is the value
    this script asks for; raise it if that warning still appears, and note that a bigger budget
    makes the instrumented binary and the *.profraw files larger, not the final one.

.PARAMETER TrainRequests
    Total keep-alive requests the -Action train run offers. Raise it if the merged profile comes
    back thin; the run is bounded by requests, not by a timer, so it is the same work every time.

.PARAMETER TrainConnections
    Keep-alive connections the training run opens. This is client shape, not a server setting.

.PARAMETER TargetBase
    Where the three build directories live. Separate --target-dir values keep the PGO artifacts out
    of target/release, so a plain release build is never silently an instrumented one.

.NOTES
    A script file needs an execution policy that permits it. On a host where all five policy scopes
    are Undefined the effective policy for a file is Restricted, and PowerShell refuses to load this
    script before it reads a single argument (measured 2026-11-02). Run it as:
      powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action status
    That scopes the exception to one process; changing the machine policy is a different decision.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action train
    # builds the instrumented training binary, runs it to a natural exit, and reports the profraw
    # files it left. Then: -Action merge, then -Action use.

.EXAMPLE
    $env:AETHER_WORKERS = '1'; powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action train
    # the same, trained on a one-worker runtime, for the pinned-core ceiling route. The training
    # binary reads AETHER_WORKERS from the environment because it builds its runtime with
    # AetherApp::build_runtime, exactly like basic_api does.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action generate
    # then: run framework\target\pgo-gen\release\basic_api.exe in a console under the load you want
    # trained on, press Ctrl+C (not taskkill), then:
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action merge
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action use

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo.ps1 -Action status
    # reports the toolchain piece, the profraw count, and which artifacts exist yet.

    Each line above carries the policy switch because on this host the bare invocation is refused
    before an argument is read. Where a reader can see that in one place instead of four, the
    comment help repeats it: an example that cannot run is not documentation, it is a defect.
#>
[CmdletBinding()]
param(
    [ValidateSet('generate', 'train', 'merge', 'use', 'status')]
    [string]$Action = 'status',
    [string]$Bin = 'basic_api',
    [string]$Package = '',
    [int]$CountersPerSite = 64,
    [int]$TrainRequests = 50000,
    [int]$TrainConnections = 16,
    [string]$TargetBase = ''
)

$ErrorActionPreference = 'Stop'

$FrameworkDir = Split-Path -Parent $PSScriptRoot
# The workspace's default-members is `examples`, so a bin from another package needs -p or cargo
# answers "no bin target found" for a target that does exist.
$select = @('--bin', $Bin)
if ($Package) { $select = @('-p', $Package, '--bin', $Bin) }
if (-not $TargetBase) { $TargetBase = Join-Path $FrameworkDir 'target' }
$ProfileDir = Join-Path $TargetBase 'pgo-profile'
$MergedProfile = Join-Path $ProfileDir 'merged.profdata'
$TargetGen = Join-Path $TargetBase 'pgo-gen'
$TargetUse = Join-Path $TargetBase 'pgo-use'
$TargetPlain = Join-Path $TargetBase 'pgo-base'

function Find-LlvmProfdata {
    $sysroot = (& rustc --print sysroot).Trim()
    $hostTriple = (& rustc -vV | Where-Object { $_ -like 'host:*' }) -replace 'host:\s*', ''
    return (Join-Path $sysroot "lib\rustlib\$hostTriple\bin\llvm-profdata.exe")
}

function Get-ProfrawFiles {
    if (-not (Test-Path $ProfileDir)) { return @() }
    return @(Get-ChildItem -Path $ProfileDir -Filter '*.profraw' -File -ErrorAction SilentlyContinue)
}

function Test-LlvmProfdata {
    $profdata = Find-LlvmProfdata
    if (-not (Test-Path $profdata)) {
        Write-Host "llvm-profdata.exe is not in this toolchain: $profdata" -ForegroundColor Red
        Write-Host 'Install it with: rustup component add llvm-tools-preview' -ForegroundColor Red
        exit 2
    }
    return $profdata
}

function Reset-ProfileDir {
    New-Item -ItemType Directory -Force -Path $ProfileDir | Out-Null
    # A stale profile from an earlier training run would be merged into this one and describe work
    # the current source may not even contain.
    $stale = Get-ProfrawFiles
    if ($stale.Count -gt 0) {
        Write-Host ("removing {0} profraw file(s) from a previous pass" -f $stale.Count)
        Remove-Item -Path ($stale | ForEach-Object { $_.FullName }) -Force
    }
}

function Set-InstrumentFlags {
    $env:RUSTFLAGS = "-Cprofile-generate=$ProfileDir -Cllvm-args=-vp-counters-per-site=$CountersPerSite"
}

<#  `llvm-profdata show` is the only thing that separates "a profile file exists" from "a profile
    describes work". Returns $null when show itself failed. #>
function Get-ProfileSummary {
    param([string]$Path)
    $profdata = Find-LlvmProfdata
    $lines = @(& $profdata show $Path 2>$null)
    if ($LASTEXITCODE -ne 0) { return $null }
    $summary = [pscustomobject]@{ Functions = -1; MaxCount = -1; TotalCount = -1 }
    foreach ($line in $lines) {
        if ("$line" -match '^Total functions:\s*(\d+)') { $summary.Functions = [int]$Matches[1] }
        if ("$line" -match '^Maximum function count:\s*(\d+)') { $summary.MaxCount = [int]$Matches[1] }
        if ("$line" -match '^Total count:\s*(\d+)') { $summary.TotalCount = [int]$Matches[1] }
    }
    return $summary
}

function Write-ProfrawReport {
    $files = Get-ProfrawFiles
    $bytes = 0
    if ($files.Count -gt 0) { $bytes = ($files | Measure-Object -Property Length -Sum).Sum }
    Write-Host ("profraw files   : {0} ({1} KiB total) in {2}" -f $files.Count, [int]($bytes / 1KB), $ProfileDir)
    return $files
}

function Write-NextStep {
    $profdata = Find-LlvmProfdata
    if (-not (Test-Path $profdata)) {
        Write-Host "llvm-profdata : MISSING  -> rustup component add llvm-tools-preview" -ForegroundColor Red
    }
    else {
        Write-Host "llvm-profdata : $profdata"
    }
    Write-ProfrawReport | Out-Null
    if (Test-Path $MergedProfile) {
        Write-Host ("merged profile  : {0} ({1} KiB)" -f $MergedProfile,
            [int]((Get-Item $MergedProfile).Length / 1KB))
        $summary = Get-ProfileSummary -Path $MergedProfile
        if ($null -ne $summary) {
            Write-Host ("profile content: functions={0} max_function_count={1} total_count={2}" -f
                $summary.Functions, $summary.MaxCount, $summary.TotalCount)
        }
        else { Write-Host 'profile content: llvm-profdata could not read it' -ForegroundColor Red }
    }
    else { Write-Host 'merged profile  : not created yet' }
    foreach ($pair in @(@('instrumented', $TargetGen), @('pgo', $TargetUse), @('plain base', $TargetPlain))) {
        $exe = Join-Path $pair[1] "release\$Bin.exe"
        if (Test-Path $exe) { Write-Host ("{0,-15}: {1}" -f $pair[0], $exe) }
    }
}

switch ($Action) {
    'status' {
        Write-NextStep
        exit 0
    }

    'generate' {
        Test-LlvmProfdata | Out-Null
        Reset-ProfileDir
        Set-InstrumentFlags
        Push-Location $FrameworkDir
        try { & cargo build --release @select --target-dir $TargetGen }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { Write-Host 'instrumented build failed' -ForegroundColor Red; exit 3 }
        Write-Host ''
        Write-Host ("instrumented binary : {0}" -f (Join-Path $TargetGen "release\$Bin.exe"))
        Write-Host 'now run it doing the real work in a console you can press Ctrl+C in, then: pgo.ps1 -Action merge'
        Write-Host 'or skip that and let this script do the load and the exit: pgo.ps1 -Action train'
        exit 0
    }

    'train' {
        Test-LlvmProfdata | Out-Null
        Reset-ProfileDir
        Set-InstrumentFlags
        Push-Location $FrameworkDir
        try { & cargo build --release -p aether-tests --example pgo_train --target-dir $TargetGen }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { Write-Host 'instrumented training build failed' -ForegroundColor Red; exit 3 }
        $trainExe = Join-Path $TargetGen 'release\examples\pgo_train.exe'
        if (-not (Test-Path $trainExe)) {
            Write-Host "training binary is not where cargo should have put it: $trainExe" -ForegroundColor Red
            exit 3
        }
        Write-Host ''
        Write-Host ("training run    : {0} {1} {2}" -f $trainExe, $TrainRequests, $TrainConnections)
        # The run serves its own routes, loads them, drains the server and returns from main, which
        # is the one ending measured to flush the counters. Its exit code is the load result.
        & $trainExe $TrainRequests $TrainConnections
        $trained = $LASTEXITCODE
        $files = Write-ProfrawReport
        if ($trained -ne 0) {
            Write-Host 'the training run reported failed requests - see its output above' -ForegroundColor Red
            exit 3
        }
        if ($files.Count -eq 0) {
            Write-Host "the run exited cleanly but left no profraw in $ProfileDir" -ForegroundColor Red
            Write-Host 'That contradicts what was measured on this toolchain; do not merge, investigate instead.'
            exit 2
        }
        Write-Host 'next: pgo.ps1 -Action merge'
        exit 0
    }

    'merge' {
        $profdata = Test-LlvmProfdata
        $files = Write-ProfrawReport
        if ($files.Count -eq 0) {
            Write-Host "no *.profraw in $ProfileDir" -ForegroundColor Red
            Write-Host 'Either the instrumented binary never ran, or it ended without returning from main.'
            Write-Host 'taskkill /F, Stop-Process -Force and std::process::exit(0) each flushed 0 files when measured.'
            Write-Host 'Use -Action train, which drives its own load and exits normally.'
            exit 2
        }
        $mergeArgs = @('merge', '-o', $MergedProfile, '-sparse') + ($files | ForEach-Object { $_.FullName })
        & $profdata @mergeArgs
        if ($LASTEXITCODE -ne 0) { Write-Host 'llvm-profdata merge failed' -ForegroundColor Red; exit 3 }
        Write-Host ("merged {0} file(s) -> {1}" -f $files.Count, $MergedProfile)
        # Step 3 of the ladder: a successful command is not proof of a useful profile. `show` is what
        # says whether the merge recorded any execution at all.
        $summary = Get-ProfileSummary -Path $MergedProfile
        if ($null -eq $summary) {
            Write-Host 'llvm-profdata could not read the profile it just wrote' -ForegroundColor Red
            exit 3
        }
        Write-Host ("profile         : functions={0} max_function_count={1} total_count={2}" -f
            $summary.Functions, $summary.MaxCount, $summary.TotalCount)
        if ($summary.Functions -lt 1 -or $summary.MaxCount -lt 1) {
            Write-Host 'The merged profile records no executions; building with it would guide nothing.' -ForegroundColor Red
            exit 2
        }
        if ($summary.Functions -lt 8) {
            Write-Host ("only {0} instrumented functions recorded - the run barely exercised this binary" -f $summary.Functions) -ForegroundColor Yellow
        }
        Write-Host 'next: pgo.ps1 -Action use'
        exit 0
    }

    'use' {
        if (-not (Test-Path $MergedProfile)) {
            Write-Host "no merged profile at $MergedProfile - run -Action merge first" -ForegroundColor Red
            exit 2
        }
        # Re-read the profile here rather than trusting that -Action merge was run against the
        # source being built now: an empty or stale merged.profdata produces a binary that is
        # release-with-no-guidance, and the A/B would then measure nothing while looking like PGO.
        $summary = Get-ProfileSummary -Path $MergedProfile
        if ($null -eq $summary) {
            Write-Host "llvm-profdata cannot read $MergedProfile - re-run -Action merge" -ForegroundColor Red
            exit 2
        }
        if ($summary.Functions -lt 1 -or $summary.MaxCount -lt 1) {
            Write-Host ("profile records nothing (functions={0} max_function_count={1}) - re-run -Action train" -f
                $summary.Functions, $summary.MaxCount) -ForegroundColor Red
            exit 2
        }
        Write-Host ("using profile   : functions={0} max_function_count={1} total_count={2}" -f
            $summary.Functions, $summary.MaxCount, $summary.TotalCount)
        $env:RUSTFLAGS = "-Cprofile-use=$MergedProfile"
        Push-Location $FrameworkDir
        try { & cargo build --release @select --target-dir $TargetUse }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { Write-Host 'PGO build failed' -ForegroundColor Red; exit 3 }
        Write-Host ''
        Write-Host ("PGO binary      : {0}" -f (Join-Path $TargetUse "release\$Bin.exe"))
        Write-Host "Baseline binary : build the SAME source with no RUSTFLAGS at all, into $TargetPlain"
        $plainSelect = ($select -join ' ')
        Write-Host "  (Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue); cargo build --release $plainSelect --target-dir $TargetPlain"
        Write-Host ''
        Write-Host 'The comparison is yours to run: release vs release, same source, same request shape,'
        Write-Host 'same connections, same worker count, pinned cores, repeated in one session. Two arms'
        Write-Host 'from different sessions are not a comparison - machine drift moves both numbers.'
        Write-Host 'Keep PGO only at >=3% rps or >=3% us-CPU/request. This script reports no number.'
        exit 0
    }
}
