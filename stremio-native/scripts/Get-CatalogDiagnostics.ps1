# Read-only host diagnostics. Run from the user's normal Windows session.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$outputDirectory = Join-Path $repo 'artifacts'
$reportPath = Join-Path $outputDirectory 'catalog-connection-report.json'

function Get-ErrorSummary($errorObject) {
    $chain = @()
    $exception = $errorObject.Exception
    while ($null -ne $exception) {
        $entry = [ordered]@{ type = $exception.GetType().FullName; hresult = $exception.HResult }
        if ($exception -is [Net.Sockets.SocketException]) {
            $entry.socketError = $exception.SocketErrorCode.ToString()
            $entry.nativeError = $exception.NativeErrorCode
        }
        if ($exception -is [Net.WebException]) { $entry.webStatus = $exception.Status.ToString() }
        $chain += $entry
        $exception = $exception.InnerException
    }
    return $chain
}

$report = [ordered]@{
    recordedAt = [DateTime]::UtcNow.ToString('o')
    user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    requests = @()
    appProcesses = @()
    firewall = @()
}
$urls = @(
    'https://v3-cinemeta.strem.io/catalog/movie/top.json',
    'https://v3-cinemeta.strem.io/catalog/series/top.json',
    'https://torrentio.strem.fun/manifest.json'
)
foreach ($url in $urls) {
    $result = [ordered]@{ url = $url }
    $client = New-Object Net.Sockets.TcpClient
    try {
        $connection = $client.ConnectAsync(([uri]$url).Host, 443)
        if (-not $connection.Wait(8000)) { throw 'TCP connection timed out' }
        $connection.GetAwaiter().GetResult()
        $result.tcpConnected = $client.Connected
    } catch {
        $result.tcpConnected = $false
        $result.tcpError = @(Get-ErrorSummary $_)
    } finally { $client.Dispose() }
    try {
        $response = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 12
        $result.httpStatus = [int]$response.StatusCode
        $body = $response.Content | ConvertFrom-Json
        if ($null -ne $body.metas) { $result.catalogItems = @($body.metas).Count }
        if ($null -ne $body.id) { $result.manifestReturned = $true }
    } catch { $result.httpError = @(Get-ErrorSummary $_) }
    $report.requests += $result
}
try {
    $processes = Get-CimInstance Win32_Process -Filter "Name='stremio-native.exe'"
    foreach ($process in $processes) {
        $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwner
        $report.appProcesses += [ordered]@{
            id = $process.ProcessId
            executable = $process.ExecutablePath
            user = "$($owner.Domain)\$($owner.User)"
        }
    }
} catch { $report.processQueryError = @(Get-ErrorSummary $_) }
try {
    $report.firewallProfiles = @(Get-NetFirewallProfile | Select-Object Name, Enabled, DefaultOutboundAction)
    foreach ($rule in (Get-NetFirewallRule -Enabled True -Direction Outbound -Action Block)) {
        $applications = @($rule | Get-NetFirewallApplicationFilter)
        foreach ($application in $applications) {
            if ($application.Program -match 'stremio-native|Pajdflix') {
                $report.firewall += [ordered]@{ name = $rule.DisplayName; program = $application.Program; action = 'Block' }
            }
        }
    }
} catch { $report.firewallQueryError = @(Get-ErrorSummary $_) }
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding UTF8
Write-Host "Report saved: $reportPath"
Write-Host 'No firewall, VPN, application data or system settings were changed.'
