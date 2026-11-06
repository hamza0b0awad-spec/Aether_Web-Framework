#Requires -Version 5.1
<#
.SYNOPSIS
    Compares two release builds of the same source under one identical open-loop load, in one session.

.DESCRIPTION
    This is the measurement half of the PGO route: `-Action use` produces a PGO binary and the
    baseline build produces the arm to compare it against. Everything else about a comparison is
    decided here so that the two arms differ in exactly one thing.

    One arm, one rep:
      1. start the server (its own process), optionally pinned to a core mask,
      2. wait until the port actually accepts — a banner is not proof of readiness,
      3. run framework's open-loop load generator against it at a fixed rate for a fixed time,
      4. read the server process' total CPU seconds before and after, and its peak working set,
      5. stop the server.

    Arms are rotated: rep 1 runs base first, rep 2 runs PGO first. Order effects have flipped
    conclusions on this machine before, so the rotation is not a courtesy.

    What the numbers mean, and do not:
      * achieved_rps and the latency percentiles come from the load generator, which is running on
        the SAME box as the server. A run that exits 2 did not deliver the offered rate, and the
        generator does not claim which side failed; those reps are reported and excluded from the
        verdict. Excluded is not the same as meaningless — a server-bound exit 2 still says the
        server could not hold that rate — so read the per-rep rows, not only the medians.
      * us_cpu_per_request is the server process' own CPU time divided by the requests it answered.
        It is the only figure here that a co-located client cannot contaminate.
      * This is a capacity and CPU comparison. A latency claim from a laptop with the generator in
        the same session is not a latency claim about production.

    Keep the optimisation only at >=3% better rps OR >=3% better us/request, measured here. A
    smaller difference is inside this harness's own spread.

.PARAMETER Base / Pgo
    The two server executables. They must be built from the SAME source; the PGO binary from a
    different tree is not an A/B.

.PARAMETER Loadgen
    The open-loop generator binary. Build it with:
      cargo build --release -p aether-tests --example openloop_loadgen

.PARAMETER Rate / Duration / Connections
    The offered load. Connections must satisfy connections >= rate x expected latency, and on
    Windows rate / connections must stay <= 500/s (the generator's 1ms timer floor).

.PARAMETER Affinity
    A process mask, e.g. 1 = core 0 only. Applied to the SERVER; the generator then runs on the
    rest of the box. With no affinity both arms share every core, which is a different question.

.PARAMETER Workers
    AETHER_WORKERS for both arms. 0 means inherit the environment.

.PARAMETER WorkersB / AffinityB
    A second runtime shape for the `pgo` arm only, so a worker-count or affinity change can be
    measured in one session against the same binary (`-Self`) instead of across two runs, where
    machine drift would be indistinguishable from the effect. -1 means "same as -Workers/-Affinity".
    When these are set, the printed delta describes the shape change, not a build change.

.PARAMETER Reps
    Repeats per arm. One rep is an anecdote; the spread across reps is the error bar.

.NOTES
    A script file needs an execution policy that permits it. On a host where all five policy scopes
    are Undefined the effective policy for a file is Restricted, and PowerShell refuses to load this
    script before it reads a single argument (measured 2026-11-02). Run it as:
      powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 -Self -Reps 3
    That scopes the exception to one process; changing the machine policy is a different decision.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 -Rate 5000 -Duration 10 -Connections 64 -Workers 1 -Affinity 1 -Reps 3

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 -Self -Workers 1 -Affinity 1 -Reps 3
    # noise floor: the same binary against itself in the same shape. Whatever delta prints here is
    # what this harness calls zero, and it is what any build or shape delta has to beat.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 -Self -Workers 1 -WorkersB 8 -Affinity 0 -Reps 3
    # the worker-count effect alone, one session, same binary, arms rotated.

    Every line above carries the policy switch because on this host the bare invocation is refused
    before an argument is read. An example that cannot run is not documentation, it is a defect.
#>
[CmdletBinding()]
param(
    [string]$Base = '',
    [string]$Pgo = '',
    [string]$Loadgen = '',
    [int]$Port = 3199,
    [string]$Path = '/health',
    [string]$Expect = '{"status":"healthy","engine":"Aether Rust"}',
    [double]$Rate = 5000,
    [int]$Duration = 10,
    [int]$Connections = 64,
    [int]$Workers = 0,
    [int]$Affinity = 0,
    [int]$WorkersB = -1,
    [int]$AffinityB = -1,
    [int]$Reps = 3,
    [switch]$Self,
    [string]$OutDir = ''
)

$ErrorActionPreference = 'Stop'

# The arm-identity rules live in one file, shared with workers_sweep.ps1: a guard that exists only
# in a copy is a guard that gets dropped when the next harness is written.
. (Join-Path $PSScriptRoot 'load_arm.ps1')

$FrameworkDir = Split-Path -Parent $PSScriptRoot
if (-not $Base) { $Base = Join-Path $FrameworkDir 'target\pgo-base\release\basic_api.exe' }
if (-not $Pgo) { $Pgo = Join-Path $FrameworkDir 'target\pgo-use\release\basic_api.exe' }
if (-not $Loadgen) { $Loadgen = Join-Path $FrameworkDir 'target\release\examples\openloop_loadgen.exe' }
if ($Self) {
    # Both arms are the SAME binary. The delta printed at the end is then what this harness calls
    # zero — the noise floor any real gain has to clear — and it is also how a runtime-shape change
    # (worker count, affinity) is measured without a second build.
    $Pgo = $Base
    if (-not $OutDir) { $OutDir = Join-Path $FrameworkDir 'target\pgo-ab-self' }
}
if (-not $OutDir) { $OutDir = Join-Path $FrameworkDir 'target\pgo-ab' }
# -1 means "not given", so an arm can be left at 0/inherit while the other is set explicitly.
if ($WorkersB -lt 0) { $WorkersB = $Workers }
if ($AffinityB -lt 0) { $AffinityB = $Affinity }

foreach ($pair in @(@('base', $Base), @('pgo', $Pgo), @('loadgen', $Loadgen))) {
    if (-not (Test-Path $pair[1])) {
        Write-Host ("{0} is missing: {1}" -f $pair[0], $pair[1]) -ForegroundColor Red
        Write-Host '  build it, then re-run this comparison'
        exit 2
    }
}
if (-not $Self -and (Get-Item $Base).LastWriteTime -ne (Get-Item $Pgo).LastWriteTime) {
    # Not a defect by itself, but the two arms must come from the same tree, and equal timestamps
    # are the cheap check that they were built in the same pass.
    Write-Host ('note: the two binaries have different build times ({0} vs {1}); confirm they are from the same source' -f
        (Get-Item $Base).LastWriteTime, (Get-Item $Pgo).LastWriteTime) -ForegroundColor Yellow
}
if (-not (Test-LoadShape -Rate $Rate -Connections $Connections)) { exit 2 }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$ExpectFile = Join-Path $OutDir 'expect_body.txt'
# Written byte-for-byte, no trailing newline, because the generator compares the body exactly. It
# travels as a file rather than as argv: PowerShell 5.1 strips the quotes out of a JSON argument on
# its way to a native exe, which made every rep of the first A/B report a body mismatch.
[IO.File]::WriteAllText($ExpectFile, $Expect)

Write-Host ("pgo-ab: base={0}" -f $Base)
Write-Host ("        pgo ={0}" -f $Pgo)
Write-Host ("        load ={0}/s x {1}s x {2} connections, port {3}, reps {4}" -f
    $Rate, $Duration, $Connections, $Port, $Reps)
Write-Host ("        base ={0} workers, affinity {1}   pgo ={2} workers, affinity {3}" -f
    $(if ($Workers -gt 0) { $Workers } else { 'inherit' }),
    $(if ($Affinity -gt 0) { $Affinity } else { 'none' }),
    $(if ($WorkersB -gt 0) { $WorkersB } else { 'inherit' }),
    $(if ($AffinityB -gt 0) { $AffinityB } else { 'none' }))

$rows = @()
for ($rep = 1; $rep -le $Reps; $rep++) {
    # Rotate which arm goes first so an ordering effect cannot masquerade as a PGO effect.
    $order = @('base', 'pgo')
    if ($rep % 2 -eq 0) { $order = @('pgo', 'base') }
    foreach ($arm in $order) {
        $isBase = $arm -eq 'base'
        $exe = if ($isBase) { $Base } else { $Pgo }
        $rows += Invoke-LoadArm -Name $arm -Exe $exe -Loadgen $Loadgen -Port $Port -Path $Path `
            -ExpectFile $ExpectFile -OutDir $OutDir -Rate $Rate -Duration $Duration `
            -Connections $Connections -Rep $Rep `
            -ArmWorkers $(if ($isBase) { $Workers } else { $WorkersB }) `
            -ArmAffinity $(if ($isBase) { $Affinity } else { $AffinityB })
        Start-Sleep -Milliseconds 500
    }
}

$rows | Export-Csv -Path (Join-Path $OutDir 'ab.csv') -NoTypeInformation
Write-Host ''
Write-Host 'per-arm medians (only reps that exited 0 are counted):'
foreach ($arm in @('base', 'pgo')) {
    $good = @($rows | Where-Object { $_.Arm -eq $arm -and $_.Exit -eq 0 })
    $bad = @($rows | Where-Object { $_.Arm -eq $arm -and $_.Exit -ne 0 })
    if ($good.Count -eq 0) {
        Write-Host ("  {0}: no usable rep ({1} rejected)" -f $arm, $bad.Count) -ForegroundColor Red
        continue
    }
    Write-Host ("  {0,-5}: reps={1} [{2}] rps={3} p50={4}us p99={5}us cpu={6}us/req peak_rss={7}MB{8}" -f
        $arm, $good.Count, $good[0].Shape, (Get-ArmMedian $good 'Rps'), (Get-ArmMedian $good 'P50'),
        (Get-ArmMedian $good 'P99'), (Get-ArmMedian $good 'UsCpu'),
        (Get-ArmMedian $good 'PeakRssMb'), $(if ($bad.Count) { "  (rejected $($bad.Count))" } else { '' }))
}
$baseGood = @($rows | Where-Object { $_.Arm -eq 'base' -and $_.Exit -eq 0 })
$pgoGood = @($rows | Where-Object { $_.Arm -eq 'pgo' -and $_.Exit -eq 0 })
if ($baseGood.Count -gt 0 -and $pgoGood.Count -gt 0) {
    $br = ($baseGood | Measure-Object -Property Rps -Average).Average
    $pr = ($pgoGood | Measure-Object -Property Rps -Average).Average
    $bc = ($baseGood | Measure-Object -Property UsCpu -Average).Average
    $pc = ($pgoGood | Measure-Object -Property UsCpu -Average).Average
    # The ratio goes in next to the percentage, because "-204%" under a sign convention nobody
    # memorised is how a threefold regression gets read as a threefold win.
    Write-Host ("  mean us/req: base {0} -> pgo {1} (ratio {2}x, >1 means pgo costs more CPU)" -f
        [math]::Round($bc, 1), [math]::Round($pc, 1), [math]::Round($pc / $bc, 2))
    Write-Host 'Every percentage below: positive = better (more rps, less CPU per request).'
    Write-Host ''
    $sameShape = ($WorkersB -eq $Workers -and $AffinityB -eq $Affinity)
    if ($Self -and $sameShape) {
        # Both arms are the same binary in the same shape, so whatever this line prints is what this
        # harness calls zero. No difference from a real build may be announced as a gain until it
        # clears it.
        Write-Host ("noise floor (same binary, same shape): rps {0:+0.0;-0.0}%   us/request {1:+0.0;-0.0}%" -f
            (($pr - $br) / $br * 100), (($bc - $pc) / $bc * 100))
        Write-Host 'A real gain must be larger than this, not equal to it.'
    }
    elseif ($Self) {
        # Same binary, different runtime shape: this is the worker/affinity effect with the build
        # variable taken out of it. It still has to clear the noise floor measured above.
        Write-Host ("shape delta (same binary): rps {0:+0.0;-0.0}%   us/request {1:+0.0;-0.0}%" -f
            (($pr - $br) / $br * 100), (($bc - $pc) / $bc * 100))
        Write-Host 'Compare this against the -Self same-shape noise floor before calling it an effect.'
    }
    else {
        Write-Host ("build delta: rps {0:+0.0;-0.0}%   us/request {1:+0.0;-0.0}%" -f
            (($pr - $br) / $br * 100), (($bc - $pc) / $bc * 100))
        Write-Host 'Keep PGO only at >=3% better on either, and only if that beats the -Self noise floor.'
    }
}
Write-Host ("raw rows: {0}" -f (Join-Path $OutDir 'ab.csv'))
Write-Host 'These numbers were produced by the agent, in this session, on this box. They are not'
Write-Host 'published results for the project; the owner re-runs them on his own machine.'
exit 0
