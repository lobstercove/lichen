Param(
    [ValidateSet('start','start-reset','stop','status')]
    [string]$Command = 'status'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Root = Resolve-Path (Join-Path $PSScriptRoot '..')
$ArtDir = Join-Path $Root 'tests/artifacts/local_cluster'
$PidFile = Join-Path $ArtDir 'pids.txt'
$Log1 = Join-Path $ArtDir 'v1.log'
$Log2 = Join-Path $ArtDir 'v2.log'
$Log3 = Join-Path $ArtDir 'v3.log'
$Bin = Join-Path $Root 'target/release/moltchain-validator.exe'
$Stagger = [int]($env:MOLT_LOCAL_STAGGER_SECS ?? '15')

New-Item -ItemType Directory -Path $ArtDir -Force | Out-Null

function Test-RpcPort {
    Param([int]$Port)
    try {
        $body = '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'
        $resp = Invoke-RestMethod -Uri ("http://127.0.0.1:{0}" -f $Port) -Method Post -ContentType 'application/json' -Body $body -TimeoutSec 4
        return ($null -ne $resp.result)
    } catch {
        return $false
    }
}

function Wait-RpcPort {
    Param([int]$Port, [int]$Attempts = 90, [int]$DelaySecs = 1)
    for ($i=0; $i -lt $Attempts; $i++) {
        if (Test-RpcPort -Port $Port) { return $true }
        Start-Sleep -Seconds $DelaySecs
    }
    return $false
}

function Stop-Cluster {
    if (Test-Path $PidFile) {
        $pids = Get-Content $PidFile | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^\d+$' }
        foreach ($pid in $pids) {
            try { Stop-Process -Id ([int]$pid) -ErrorAction SilentlyContinue } catch {}
        }
        Start-Sleep -Seconds 1
        foreach ($pid in $pids) {
            try { Stop-Process -Id ([int]$pid) -Force -ErrorAction SilentlyContinue } catch {}
        }
        Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
    }

    foreach ($port in 8899,8901,8903,7001,7002,7003) {
        $listeners = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
        foreach ($l in $listeners) {
            try { Stop-Process -Id $l.OwningProcess -Force -ErrorAction SilentlyContinue } catch {}
        }
    }
}

function Ensure-Binary {
    if (-not (Test-Path $Bin)) {
        Push-Location $Root
        try {
            cargo build --release --bin moltchain-validator | Out-Null
        } finally {
            Pop-Location
        }
    }
}

function Start-Validator {
    Param(
        [int]$Number,
        [string]$LogPath
    )

    $p2p = 7001 + ($Number - 1)
    $rpc = 8899 + (2 * ($Number - 1))
    $ws = 8900 + (2 * ($Number - 1))
    $db = Join-Path $Root ("data/state-{0}" -f $p2p)
    $signerBind = "0.0.0.0:{0}" -f (9300 + $Number)

    $args = @(
        '--network','testnet',
        '--dev-mode',
        '--p2p-port',$p2p,
        '--rpc-port',$rpc,
        '--ws-port',$ws,
        '--db-path',$db
    )
    if ($Number -gt 1) {
        $args += @('--bootstrap-peers','127.0.0.1:7001')
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Bin
    $psi.WorkingDirectory = [string]$Root
    $psi.Arguments = ($args -join ' ')
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['MOLTCHAIN_SIGNER_BIND'] = $signerBind
    $psi.EnvironmentVariables['RUST_LOG'] = 'warn'

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    $null = $proc.Start()

    $outWriter = [System.IO.StreamWriter]::new($LogPath, $true)
    Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action {
        if ($Event.SourceEventArgs.Data) { $outWriter.WriteLine($Event.SourceEventArgs.Data); $outWriter.Flush() }
    } | Out-Null
    Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action {
        if ($Event.SourceEventArgs.Data) { $outWriter.WriteLine($Event.SourceEventArgs.Data); $outWriter.Flush() }
    } | Out-Null
    $proc.BeginOutputReadLine()
    $proc.BeginErrorReadLine()

    return $proc.Id
}

function Show-Status {
    $up = 0
    foreach ($port in 8899,8901,8903) {
        if (Test-RpcPort -Port $port) { $up++ }
    }
    if ($up -eq 3) {
        Write-Host '[local-3validators] status=up rpc=8899,8901,8903 p2p=7001,7002,7003 data=data/state-{7001,7002,7003}'
        return $true
    }
    Write-Host ("[local-3validators] status=down reachable_rpc={0}/3" -f $up)
    return $false
}

switch ($Command) {
    'stop' {
        Stop-Cluster
        Write-Host '[local-3validators] stopped'
    }
    'status' {
        if (-not (Show-Status)) { exit 1 }
    }
    'start' {
        Stop-Cluster
        Ensure-Binary
        $v1 = Start-Validator -Number 1 -LogPath $Log1
        Start-Sleep -Seconds $Stagger
        $v2 = Start-Validator -Number 2 -LogPath $Log2
        Start-Sleep -Seconds $Stagger
        $v3 = Start-Validator -Number 3 -LogPath $Log3

        if (-not (Wait-RpcPort -Port 8899) -or -not (Wait-RpcPort -Port 8901) -or -not (Wait-RpcPort -Port 8903)) {
            Stop-Cluster
            throw '[local-3validators] cluster failed to become healthy'
        }

        Set-Content -Path $PidFile -Value "${v1}`n${v2}`n${v3}`n" -Encoding UTF8
        Write-Host ("[local-3validators] ready pids={0},{1},{2}" -f $v1,$v2,$v3)
    }
    'start-reset' {
        Push-Location $Root
        try {
            bash ./reset-blockchain.sh testnet | Out-Null
        } finally {
            Pop-Location
        }
        & $PSCommandPath -Command start
    }
}
