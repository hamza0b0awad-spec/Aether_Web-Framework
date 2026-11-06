#Requires -Version 5.1
<#
.SYNOPSIS
    The arm-identity rules every measurement arm in this repository has to satisfy, in one place.

.DESCRIPTION
    A measurement arm is: start one server process, prove it is the process answering the port, run
    the open-loop generator against it, and read that process' own CPU and peak working set around
    the run. `pgo_ab.ps1` does that twice; `workers_sweep.ps1` does it N times. The rules below are
    why either answer is worth reading, so they live here rather than being copied — a copied guard
    is a guard that gets left behind when the reason for it is found again.

    What each guard exists for, in the order it fires:
      * A port that already answers before the arm starts means the run would measure whatever is
        holding it. The first A/B here did exactly that: six arms of plausible-looking rows, none of
        them describing the binary named in the row.
      * The banner is the arm's own statement of what it became — the port it bound and the worker
        count the runtime actually has. Checked, not assumed.
      * A server that survives Stop-Process leaves the port held for the next arm, so the comparison
        quietly stops being one. Refusing to continue is the whole point of the check.
      * The expected body travels as a file: PowerShell 5.1 strips the quotes out of a JSON argument
        on its way to a native exe, which made every rep of an otherwise healthy run a body mismatch.

    `us_cpu_per_request` is the server process' own CPU seconds divided by the requests it answered —
    the only figure a co-located generator cannot contaminate.

.PARAMETER Name
    Arm label, used for the row and for the per-arm evidence files under -OutDir.

.PARAMETER ArmWorkers
    AETHER_WORKERS for this arm. 0 means inherit, which is how the runtime's own default is measured
    instead of assumed.

.EXAMPLE
    . framework\tools\load_arm.ps1
    $row = Invoke-LoadArm -Name w1 -Exe $exe -Loadgen $gen -Port 3199 -Path '/health' `
        -ExpectFile $expect -OutDir $dir -Rate 5000 -Duration 10 -Connections 64 -ArmWorkers 1 -Rep 1
#>

function Test-PortFree {
    param([int]$Port)
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $client.Connect('127.0.0.1', $Port)
        $client.Close()
        return $false
    }
    catch { return $true }
}

function Wait-Port {
    param([int]$Port, [int]$TimeoutSec)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $client = New-Object System.Net.Sockets.TcpClient
            $client.Connect('127.0.0.1', $Port)
            $client.Close()
            return $true
        }
        catch { Start-Sleep -Milliseconds 100 }
    }
    return $false
}

function Stop-Server {
    param($Server, [string]$Label)
    Stop-Process -Id $Server.Id -Force -ErrorAction SilentlyContinue
    try { $Server.WaitForExit(5000) | Out-Null } catch { }
    $Server.Refresh()
    # A survivor here is not a leak to ignore: the next arm would find the port already answering
    # and measure the wrong binary while reporting a clean comparison.
    if (-not $Server.HasExited) {
        throw "$Label server (pid $($Server.Id)) survived Stop-Process; refusing to continue"
    }
}

function Invoke-LoadArm {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][string]$Loadgen,
        [Parameter(Mandatory)][int]$Port,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ExpectFile,
        [Parameter(Mandatory)][string]$OutDir,
        [Parameter(Mandatory)][double]$Rate,
        [Parameter(Mandatory)][int]$Duration,
        [Parameter(Mandatory)][int]$Connections,
        [int]$ArmWorkers = 0,
        [int]$ArmAffinity = 0,
        [int]$Rep = 1
    )

    if ($ArmWorkers -gt 0) { $env:AETHER_WORKERS = "$ArmWorkers" } else { Remove-Item Env:AETHER_WORKERS -ErrorAction SilentlyContinue }
    $env:AETHER_PORT = "$Port"
    $env:AETHER_KEEP_ALIVE_TIMEOUT_SECS = '30'

    # Arm identity is the first thing to prove. The first A/B run here measured a leftover server
    # from an earlier run for all six arms, because the port was already answering when the next one
    # started: every row looked plausible and none of them described the binary named in it.
    if (-not (Test-PortFree -Port $Port)) {
        throw "port $Port is already answering before the $Name arm started - stop whatever holds it"
    }
    $bannerFile = Join-Path $OutDir "$Name`_$Rep`_banner.txt"
    $errFile = Join-Path $OutDir "$Name`_$Rep`_stderr.txt"
    $server = Start-Process -FilePath $Exe -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $bannerFile -RedirectStandardError $errFile
    if (-not (Wait-Port -Port $Port -TimeoutSec 20)) {
        Stop-Server -Server $server -Label $Name
        Write-Host ("  server banner was: {0}" -f (Get-Content $bannerFile -Raw -ErrorAction SilentlyContinue))
        Write-Host ("  server stderr was: {0}" -f (Get-Content $errFile -Raw -ErrorAction SilentlyContinue))
        throw "$Name server (pid $($server.Id)) never listened on $Port"
    }
    if ($server.HasExited) {
        throw "our $Name server (pid $($server.Id)) is gone but $Port answers - something else holds the port"
    }
    # The banner is the arm's own statement of what it became: the port it bound and the worker
    # count the runtime actually has. Checked rather than assumed, because the whole comparison is
    # void if an arm is not the shape it is labelled with.
    $banner = Get-Content $bannerFile -Raw -ErrorAction SilentlyContinue
    if ($banner -notmatch "http://0\.0\.0\.0:$Port") {
        throw "$Name banner does not mention port ${Port}:`n$banner"
    }
    if ($ArmWorkers -gt 0 -and $banner -notmatch "Workers     : $ArmWorkers \(") {
        throw "$Name banner does not report $ArmWorkers active workers:`n$banner"
    }
    # For an inherit arm the count is the answer, not a check: this is what the runtime default
    # actually resolved to on this box, read from the arm's own banner rather than guessed from
    # num_cpus.
    $bannerWorkers = ''
    if ($banner -match 'Workers     : (\d+) \(') { $bannerWorkers = $Matches[1] }
    if ($ArmAffinity -gt 0) {
        $server.ProcessorAffinity = [IntPtr]$ArmAffinity
        $server.Refresh()
    }
    $cpuBefore = $server.CPU
    $loadFile = Join-Path $OutDir "$Name`_$Rep`_load.txt"
    Write-Host ("  {0} rep {1} [workers {2}, affinity {3}]: {4}/s x {5}s x {6} conns ..." -f
        $Name, $Rep, $(if ($ArmWorkers -gt 0) { $ArmWorkers } else { 'inherit' }),
        $(if ($ArmAffinity -gt 0) { $ArmAffinity } else { 'none' }), $Rate, $Duration, $Connections) -NoNewline
    $genArgs = @('--url', ("http://127.0.0.1:{0}{1}" -f $Port, $Path), '--rate', "$Rate",
        '--duration', "$Duration", '--connections', "$Connections", '--expect-file', $ExpectFile)
    # Captured and written explicitly: `$out = & exe | Out-String` is the redirect here, because a
    # '>' inside the argument list would just be another argv item the generator then rejects.
    $gen = & $Loadgen @genArgs 2>&1 | Out-String
    $exit = $LASTEXITCODE
    Set-Content -Path $loadFile -Value $gen -Encoding UTF8

    $server.Refresh()
    $cpuAfter = $server.CPU
    $peakRss = $server.PeakWorkingSet64
    Stop-Server -Server $server -Label $Name

    $text = Get-Content $loadFile -Raw -ErrorAction SilentlyContinue
    $row = [pscustomobject]@{
        Arm = $Name; Rep = $Rep; Exit = $exit
        Shape = ("w{0}/a{1}" -f $(if ($ArmWorkers -gt 0) { $ArmWorkers } else { 'inherit' }),
                                  $(if ($ArmAffinity -gt 0) { $ArmAffinity } else { 'none' }))
        Rps = 0.0; P50 = 0; P99 = 0; Completed = 0
        UsCpu = 0.0; PeakRssMb = 0.0; Result = 'unreadable'; BannerWorkers = $bannerWorkers
    }
    if ($text) {
        if ($text -match 'wall_s=[0-9.]+ achieved_rps=([0-9.]+)') { $row.Rps = [double]$Matches[1] }
        if ($text -match 'completed=(\d+)') { $row.Completed = [int]$Matches[1] }
        if ($text -match 'latency_us: p50=(\d+) p95=\d+ p99=(\d+)') {
            $row.P50 = [int]$Matches[1]; $row.P99 = [int]$Matches[2]
        }
        if ($text -match 'result: (\w[\w-]*)') { $row.Result = $Matches[1] }
        $cpuSeconds = [double]$cpuAfter - [double]$cpuBefore
        if ($row.Completed -gt 0) { $row.UsCpu = [math]::Round($cpuSeconds * 1e6 / $row.Completed, 1) }
        $row.PeakRssMb = [math]::Round($peakRss / 1MB, 1)
    }
    # Exit 0 with unparsed output is a parse failure, not a measurement: the row would carry rps=0 and
    # a mean of zero turns every delta printed from it into Infinity. Callers filter on this.
    $row | Add-Member -NotePropertyName Usable -NotePropertyValue ($exit -eq 0 -and $row.Result -ne 'unreadable')
    Write-Host (" rps={0} p50={1}us p99={2}us cpu={3}us/req rss={4}MB exit={5} {6}" -f
        $row.Rps, $row.P50, $row.P99, $row.UsCpu, $row.PeakRssMb, $row.Exit, $row.Result)
    return $row
}

function Test-LoadShape {
    param([double]$Rate, [int]$Connections)
    # The generator's per-connection interval floors at 1 ms on Windows, so a rate that needs more
    # than that per connection cannot be offered at all — the run would under-deliver and look like
    # a server that cannot hold the rate.
    $perConnection = $Rate / $Connections
    if ($perConnection -gt 500) {
        Write-Host ("{0}/s over {1} connections is {2}/s per connection, above the generator's 500/s floor" -f
            $Rate, $Connections, [math]::Round($perConnection, 1)) -ForegroundColor Red
        Write-Host 'Raise -Connections, or lower -Rate.'
        return $false
    }
    return $true
}

function Get-ArmMedian {
    param($Rows, [string]$Property)
    $vals = @($Rows | ForEach-Object { $_.$Property } | Sort-Object)
    [math]::Round($vals[[int][math]::Floor(($vals.Count - 1) / 2)], 1)
}
