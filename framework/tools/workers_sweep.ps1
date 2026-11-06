#Requires -Version 5.1
<#
.SYNOPSIS
    Measures one binary under N runtime worker-count shapes, in one session, with a null arm.

.DESCRIPTION
    The runtime default is `num_cpus::get().max(1)` (`framework/aether/src/config.rs`), and every
    worker-count number ever quoted here came from a two-arm comparison or from a run on a box whose
    shape the reader did not know. This script prices the whole ladder in one session: the same
    binary, the same offered load, arms `1,2,4,8,default`, with `default` meaning "inherit" so the
    banner's own reported worker count is what gets recorded rather than a guess about num_cpus.

    One shape, one rep:
      1. start the server (its own process), optionally pinned to a core mask,
      2. wait until the port actually accepts — a banner is not proof of readiness,
      3. run framework's open-loop load generator against it at a fixed rate for a fixed time,
      4. read the server process' total CPU seconds before and after, and its peak working set,
      5. stop the server.

    The arm-identity rules (`framework/tools/load_arm.ps1`) are not optional here and are the reason
    any row of this table can be read at all: a port already held by a leftover server makes every
    later arm measure the wrong process while printing plausible numbers.

    What the numbers mean, and do not:
      * achieved_rps and the latency percentiles come from the load generator, which is running on
        the SAME box as the server. A run that exits 2 did not deliver the offered rate, and the
        generator does not claim which side failed; those reps are reported and excluded from the
        medians. Excluded is not the same as meaningless — a server-bound exit 2 still says the
        server could not hold that rate — so read the per-rep rows, not only the medians.
      * A shape whose arm cannot hold the rate at all is a result, not a failed run: on a box with
        fewer real cores than workers, more workers can mean less throughput. That is exactly what
        this table is for, which is why every shape prints its own exit-2 count.
      * us_cpu_per_request is the server process' own CPU time divided by the requests it answered.
        It is the only figure here that a co-located client cannot contaminate.
      * This is a capacity and CPU comparison on one laptop. It is not a published result; the owner
        re-runs it on his own machine.

.PARAMETER Shapes
    AETHER_WORKERS values, comma-separated. 0 means inherit, i.e. the runtime's own default. Each
    value becomes one arm; the first is the baseline every delta is taken against.
    Deliberately a string, not an int array: `powershell -File` passes `1,0` as one argv token and
    an `[int[]]` parameter then reads it through the locale's group separator, so the two shapes
    silently become the single shape 10. Measured: `-Shapes 1,0` under -File produced one arm whose
    banner reported 10 workers. This script splits and validates instead of trusting the binder.

.PARAMETER NullArm
    Add one extra arm identical to the first shape. Whatever delta prints between two arms that are
    the same shape is what this harness calls zero, and it is what every worker-count delta here has
    to beat before it can be called an effect.

.PARAMETER Exe
    The server binary. One binary only: this sweep varies the runtime shape, not the build. For a
    build comparison use framework/tools/pgo_ab.ps1.

.PARAMETER Rate / Duration / Connections
    The offered load. Connections must satisfy connections >= rate x expected latency, and on
    Windows rate / connections must stay <= 500/s (the generator's 1ms timer floor).

.PARAMETER Affinity
    A process mask applied to the SERVER, e.g. 1 = core 0 only, so a worker count can be read
    against a known core budget. The generator then runs on the rest of the box.

.PARAMETER Reps
    Repeats per shape. One rep is an anecdote; the spread across reps is the error bar, and it is
    printed per shape.

.NOTES
    A script file needs an execution policy that permits it. On a host where all five policy scopes
    are Undefined the effective policy for a file is Restricted, and PowerShell refuses to load this
    script before it reads a single argument (measured 2026-11-02). Run it as:
      powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\workers_sweep.ps1 -NullArm
    That scopes the exception to one process; changing the machine policy is a different decision.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\workers_sweep.ps1 -NullArm -Rate 5000 -Duration 10 -Connections 64 -Reps 3
    # the default ladder: 1, 2, 4, 8 and inherit, plus the null arm that says what zero looks like
    # on this box today.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\workers_sweep.ps1 -Shapes 1,0 -NullArm -Affinity 1 -Reps 3
    # the single-core question: an explicit one-worker arm against the runtime default, both pinned
    # to core 0, so "default" is priced against the shape it is assumed to be. The comma is safe
    # here because this script splits -Shapes itself, before any number is read (see its help).

    Every line above carries the policy switch because on this host the bare invocation is refused
    before an argument is read. An example that cannot run is not documentation, it is a defect.
#>
[CmdletBinding()]
param(
    [string]$Exe = '',
    [string]$Loadgen = '',
    [int]$Port = 3199,
    [string]$Path = '/health',
    [string]$Expect = '{"status":"healthy","engine":"Aether Rust"}',
    [double]$Rate = 5000,
    [int]$Duration = 10,
    [int]$Connections = 64,
    [int]$Affinity = 0,
    [string]$Shapes = '1,2,4,8,0',
    [int]$Reps = 3,
    [switch]$NullArm,
    [string]$OutDir = ''
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'load_arm.ps1')

$FrameworkDir = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $FrameworkDir 'target\release\basic_api.exe' }
if (-not $Loadgen) { $Loadgen = Join-Path $FrameworkDir 'target\release\examples\openloop_loadgen.exe' }
if (-not $OutDir) { $OutDir = Join-Path $FrameworkDir 'target\workers-sweep' }
# Split before any numeric conversion, and convert only after the split: a token that still contains
# a comma would reach [int] through the locale group separator and turn "1,0" into 10.
$shapeList = @()
foreach ($token in ($Shapes -split '[,\s]+' | Where-Object { $_ -ne '' })) {
    $value = 0
    if (-not [int]::TryParse($token, [ref]$value)) {
        Write-Host ("-Shapes item '{0}' is not a worker count. Use comma-separated integers;" -f $token) -ForegroundColor Red
        Write-Host '  0 means inherit (the runtime default).'
        exit 2
    }
    if ($value -lt 0) {
        Write-Host ("-Shapes item '{0}' is negative; 0 is the value for inherit" -f $token) -ForegroundColor Red
        exit 2
    }
    $shapeList += $value
}
if ($shapeList.Count -eq 0) {
    Write-Host '-Shapes was empty; there is no arm to run.' -ForegroundColor Red
    exit 2
}

foreach ($pair in @(@('server', $Exe), @('loadgen', $Loadgen))) {
    if (-not (Test-Path $pair[1])) {
        Write-Host ("{0} is missing: {1}" -f $pair[0], $pair[1]) -ForegroundColor Red
        Write-Host '  build it, then re-run this sweep'
        exit 2
    }
}
if (-not (Test-LoadShape -Rate $Rate -Connections $Connections)) { exit 2 }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$ExpectFile = Join-Path $OutDir 'expect_body.txt'
# Written byte-for-byte, no trailing newline, because the generator compares the body exactly, and
# it travels as a file rather than as argv (see load_arm.ps1).
[IO.File]::WriteAllText($ExpectFile, $Expect)

# Two shapes with the same worker count would collide on the arm label, and the label is also the
# evidence filename, so a collision silently overwrites one arm's rows with the other's.
$seen = @{}
$arms = @()
foreach ($shape in $shapeList) {
    if ($seen.ContainsKey($shape)) {
        Write-Host ("-Shapes repeats {0}; a repeated shape needs -NullArm, not a duplicate entry" -f $shape) -ForegroundColor Red
        exit 2
    }
    $seen[$shape] = $true
    $arms += [pscustomobject]@{
        Name = $(if ($shape -gt 0) { "w$shape" } else { 'default' })
        Workers = $shape
        Null = $false
    }
}
if ($NullArm) {
    # The same shape twice: the delta between these two arms is measurement, not effect.
    $arms += [pscustomobject]@{
        Name = "null_$(if ($shapeList[0] -gt 0) { "w$($shapeList[0])" } else { 'default' })"
        Workers = $shapeList[0]
        Null = $true
    }
}

Write-Host ("workers-sweep: server={0}" -f $Exe)
Write-Host ("             load={0}/s x {1}s x {2} connections, port {3}, affinity {4}, reps {5}" -f
    $Rate, $Duration, $Connections, $Port, $(if ($Affinity -gt 0) { $Affinity } else { 'none' }), $Reps)
Write-Host ("             arms={0}" -f (($arms | ForEach-Object { $_.Name }) -join ', '))

$rows = @()
for ($rep = 1; $rep -le $Reps; $rep++) {
    # Rotate which shape runs first so an ordering effect (a warming box, a background task) cannot
    # land on the same arm every rep and look like that shape's cost.
    $order = @($arms)
    $offset = ($rep - 1) % $arms.Count
    if ($offset -gt 0) { $order = @($order[$offset..($order.Count - 1)]) + @($order[0..($offset - 1)]) }
    Write-Host ("  rep {0} order: {1}" -f $rep, (($order | ForEach-Object { $_.Name }) -join ', '))
    foreach ($arm in $order) {
        $row = Invoke-LoadArm -Name $arm.Name -Exe $Exe -Loadgen $Loadgen -Port $Port -Path $Path `
            -ExpectFile $ExpectFile -OutDir $OutDir -Rate $Rate -Duration $Duration `
            -Connections $Connections -ArmWorkers $arm.Workers -ArmAffinity $Affinity -Rep $rep
        # The banner's worker count is the sweep's identity column: it is what says the arm ran with
        # the shape its row is labelled with, and for the inherit arm it is the only reading of the
        # runtime default. An empty one means the banner changed shape, not that the box is idle.
        if (-not $row.BannerWorkers) {
            throw "$($arm.Name) rep $rep reported no worker count in its banner - refusing to sweep"
        }
        $rows += $row
        Start-Sleep -Milliseconds 500
    }
}

$rows | Export-Csv -Path (Join-Path $OutDir 'sweep.csv') -NoTypeInformation
Write-Host ''
Write-Host 'per-shape medians (only reps that exited 0 and parsed are counted; the spread is over them):'
$summaries = @()
foreach ($arm in $arms) {
    $good = @($rows | Where-Object { $_.Arm -eq $arm.Name -and $_.Usable })
    $bad = @($rows | Where-Object { $_.Arm -eq $arm.Name -and -not $_.Usable })
    if ($good.Count -eq 0) {
        Write-Host ("  {0,-9}: no usable rep ({1} rejected)" -f $arm.Name, $bad.Count) -ForegroundColor Red
        $summaries += [pscustomobject]@{ Arm = $arm.Name; Workers = $arm.Workers; Null = $arm.Null
                                         Rps = 0.0; UsCpu = 0.0; Good = 0; Bad = $bad.Count }
        continue
    }
    $rpsVals = @($good | ForEach-Object { $_.Rps })
    $min = ($rpsVals | Measure-Object -Minimum).Minimum
    $max = ($rpsVals | Measure-Object -Maximum).Maximum
    $bannerWorkers = ($good | ForEach-Object { $_.BannerWorkers } | Select-Object -First 1)
    Write-Host ("  {0,-9}: reps={1}/{2} banner_workers={3} rps={4} [{5}..{6}] p50={7}us p99={8}us cpu={9}us/req peak_rss={10}MB{11}" -f
        $arm.Name, $good.Count, $Reps, $bannerWorkers,
        (Get-ArmMedian $good 'Rps'), $min, $max, (Get-ArmMedian $good 'P50'), (Get-ArmMedian $good 'P99'),
        (Get-ArmMedian $good 'UsCpu'), (Get-ArmMedian $good 'PeakRssMb'),
        $(if ($bad.Count) { "  (rejected $($bad.Count))" } else { '' }))
    $summaries += [pscustomobject]@{ Arm = $arm.Name; Workers = $arm.Workers; Null = $arm.Null
                                     Rps = (Get-ArmMedian $good 'Rps'); UsCpu = (Get-ArmMedian $good 'UsCpu')
                                     Good = $good.Count; Bad = $bad.Count }
}

$usable = @($summaries | Where-Object { $_.Good -gt 0 })
if ($usable.Count -ge 2) {
    # Means, not medians, for the percentages: a median is one rep and the medians of two shapes can
    # sit on the same rep's luck. The printed medians above are what to read; these ratios are the
    # shape-to-shape comparison.
    $mean = {
        param($arm, $prop)
        ((@($rows | Where-Object { $_.Arm -eq $arm -and $_.Usable }) |
            Measure-Object -Property $prop -Average).Average)
    }
    $base = $usable[0]
    Write-Host ''
    Write-Host ("Deltas are against the first arm ({0}). Positive = better (more rps, less CPU per" -f $base.Arm)
    Write-Host 'request). A delta has to be larger than the null arm below, and only shapes that'
    Write-Host 'delivered the offered rate in at least one rep appear here.'
    Write-Host ''
    foreach ($s in ($usable | Where-Object { $_.Arm -ne $base.Arm })) {
        $br = & $mean $base.Arm 'Rps'; $sr = & $mean $s.Arm 'Rps'
        $bc = & $mean $base.Arm 'UsCpu'; $sc = & $mean $s.Arm 'UsCpu'
        $label = if ($s.Null) { "$($s.Arm) (null arm)" } else { $s.Arm }
        Write-Host ("  {0,-16}: rps {1:+0.0;-0.0}%   us/request {2:+0.0;-0.0}%   cpu ratio {3}x" -f
            $label, (($sr - $br) / $br * 100), (($bc - $sc) / $bc * 100), [math]::Round($sc / $bc, 2))
    }
    # Named $nullArms, not $nullArm: parameter variables keep their declared type, and assigning an
    # array to the [switch] $NullArm parameter throws — and the throw lands after the whole sweep has
    # already run, so the table prints and the exit code is 1.
    $nullArms = @($usable | Where-Object { $_.Null })
    if ($nullArms.Count -gt 0) {
        Write-Host ''
        Write-Host 'The null arm is the same shape as the baseline measured a second time, so the'
        Write-Host 'percentages printed for it are what this harness calls zero on this box today.'
        Write-Host 'No worker-count effect may be announced until it is larger than that.'
    } else {
        Write-Host ''
        Write-Host 'No null arm was requested (-NullArm), so nothing printed above is calibrated' -ForegroundColor Yellow
        Write-Host 'against this harness''s own zero. Re-run with -NullArm before quoting a delta.' -ForegroundColor Yellow
    }
}
Write-Host ("raw rows: {0}" -f (Join-Path $OutDir 'sweep.csv'))
Write-Host 'These numbers were produced by the agent, in this session, on this box. They are not'
Write-Host 'published results for the project; the owner re-runs them on his own machine.'
exit 0
