$ErrorActionPreference = "Stop"
$Root = Resolve-Path "$PSScriptRoot/../.."
$BuildProfile = if ($env:BURROW_E2E_PROFILE) { $env:BURROW_E2E_PROFILE } else { "release" }
cargo build --profile $BuildProfile --manifest-path "$Root/Cargo.toml" --bin burrow --example loadgen -q
if ($LASTEXITCODE -ne 0) { throw "build failed" }
$Out = Join-Path $Root "target/$(if ($BuildProfile -eq 'dev') { 'debug' } else { $BuildProfile })"
$B = Join-Path $Out "burrow.exe"
$Loadgen = Join-Path $Out "examples/loadgen.exe"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("burrow-e2e-" + [guid]::NewGuid())
New-Item -ItemType Directory $Work | Out-Null
$Echo = $null
$Passed = $false

function Burrow { $ErrorActionPreference = "Continue"; & $B @args 2>&1 | Out-String }
function Check($msg) { Write-Host "  ok: $msg" }
function Fail($msg) {
  Write-Host "FAIL: $msg"
  if ($env:GITHUB_ACTIONS) {
    Write-Host "::error title=service e2e::$($msg -replace "`r?`n", ' ')"
    foreach ($n in "burrow-e2e-server", "burrow-e2e-client") {
      Write-Host "::error title=$n logs::$((Burrow service logs $n -n 15) -replace "`r?`n", '|')"
    }
  }
  Burrow service logs burrow-e2e-server -n 30 | Write-Host
  Burrow service logs burrow-e2e-client -n 30 | Write-Host
  throw $msg
}
function Traffic($count) {
  for ($i = 0; $i -lt 100; $i++) {
    & $Loadgen latency --addr 127.0.0.1:27940 --count 10 *> $null
    if ($LASTEXITCODE -eq 0) { break }
    Start-Sleep -Milliseconds 200
  }
  & $Loadgen latency --addr 127.0.0.1:27940 --count $count *> $null
  return $LASTEXITCODE -eq 0
}
function Running($name) {
  for ($i = 0; $i -lt 50; $i++) {
    if ((Burrow service status $name) -match "(?m)^${name}: running") { return $true }
    Start-Sleep -Milliseconds 200
  }
  return $false
}

try {
  Set-Location $Work
  $Echo = Start-Process -FilePath $Loadgen -ArgumentList "echo","--listen","127.0.0.1:19100" -PassThru -WindowStyle Hidden

  & $B init server -o e2e-server.yaml | Out-Null
  (Get-Content e2e-server.yaml) -replace '0.0.0.0:7835','127.0.0.1:27935' -replace '0.0.0.0:7840','127.0.0.1:27940' -replace '127.0.0.1:3000','127.0.0.1:19100' | Set-Content e2e-server.yaml
  & $B client-config home -c e2e-server.yaml --server 127.0.0.1:27935 | Set-Content e2e-client.yaml
  $Secret = ((Get-Content e2e-client.yaml | Select-String '^secret:').Line -split '\s+')[1]
  (Get-Content e2e-client.yaml) -replace '^secret: .*','secret: $${E2E_SECRET}' | Set-Content e2e-client.yaml

  Write-Host "== a busy port stops the install before anything changes"
  $busy = [System.Net.Sockets.TcpListener]::new([ipaddress]"127.0.0.1", 27940)
  $busy.Start()
  $out = Burrow service install -c e2e-server.yaml
  $busy.Stop()
  if ($out -notmatch 'port 27940 \(service "web"\) is already in use') { Fail "unexpected: $out" }
  if ((Burrow service list) -match "burrow-e2e-server") { Fail "a failed install left a service behind" }
  Check "busy port reported"

  Write-Host "== an unset variable is explained"
  Remove-Item Env:E2E_SECRET -ErrorAction SilentlyContinue
  $out = Burrow service install -c e2e-client.yaml
  if ($out -notmatch 'E2E_SECRET') { Fail "missing hint: $out" }
  Check "env hint shown"

  Write-Host "== install server and client"
  $out = Burrow service install -c e2e-server.yaml
  Write-Host $out
  if ($out -notmatch "installed Windows service") { Fail "server install: $out" }
  $env:E2E_SECRET = $Secret
  $out = Burrow service install -c e2e-client.yaml
  Remove-Item Env:E2E_SECRET
  Write-Host $out
  if ($out -notmatch "installed Windows service") { Fail "client install: $out" }
  if (-not (Traffic 500)) { Fail "no traffic through the installed services" }
  Check "traffic flows through the installed server and client"
  if (-not (Running burrow-e2e-server)) { Fail "server not running" }
  $log = ((Burrow service status burrow-e2e-server) -split "`n" | Select-String 'logs:').Line.Trim() -replace '^logs:\s+',''
  if (-not (Test-Path $log)) { Fail "no log file at $log" }
  if ((Burrow service logs burrow-e2e-server -n 20) -notmatch "listening for clients") { Fail "service logs show nothing" }
  if (@((Burrow service list) -split "`n" | Select-String "running").Count -ne 2) { Fail "list does not show both services" }
  Check "log file $log, status and list"

  $users = (New-Object Security.Principal.SecurityIdentifier "S-1-5-32-545").Translate([Security.Principal.NTAccount]).Value
  $write = [Security.AccessControl.FileSystemRights]"WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, DeleteSubdirectoriesAndFiles, Delete, ChangePermissions, TakeOwnership"
  foreach ($dir in "services", "logs", "") {
    $path = Join-Path $env:ProgramData "burrow\$dir"
    $open = (Get-Acl $path).Access | Where-Object {
      $_.IdentityReference.Value -eq $users -and $_.AccessControlType -eq "Allow" -and ($_.FileSystemRights -band $write)
    }
    if ($open) { Fail "ordinary users can write to $path" }
  }
  Check "ordinary users cannot write to burrow's folders"

  Write-Host "== the same tunnel cannot also run by hand"
  $env:E2E_SECRET = $Secret
  $out = Burrow client -c e2e-client.yaml
  Remove-Item Env:E2E_SECRET
  if ($out -notmatch "is already running") { Fail "unexpected: $out" }
  Check "manual duplicate refused"

  Write-Host "== restart and reload keep it working"
  Burrow service restart burrow-e2e-server | Out-Null
  Burrow service reload burrow-e2e-client | Out-Null
  if (-not (Traffic 200)) { Fail "no traffic after restart" }
  Check "restart and reload"

  Write-Host "== update: same version is left alone, --force replaces and restarts"
  $out = Burrow service update
  if ($out -notmatch "nothing to do") { Fail "same-version update did something: $out" }
  $out = Burrow service update --force
  if ($out -notmatch "restarted burrow-e2e-client, burrow-e2e-server") { Fail "forced update: $out" }
  if (-not (Traffic 200)) { Fail "no traffic after update" }
  Check "update"

  Write-Host "== a stopped service stays stopped"
  Burrow service stop burrow-e2e-client | Out-Null
  Start-Sleep -Seconds 7
  if ((Burrow service status burrow-e2e-client) -notmatch "(?m)^burrow-e2e-client: stopped") { Fail "service came back after stop" }
  Burrow service start burrow-e2e-client | Out-Null
  if (-not (Running burrow-e2e-client)) { Fail "not running after start" }
  Check "stop and start"

  Write-Host "== uninstall removes everything it created"
  Burrow service uninstall burrow-e2e-client --purge | Write-Host
  Burrow service uninstall burrow-e2e-server --purge | Write-Host
  Start-Sleep -Seconds 2
  if (Get-Service burrow-e2e-* -ErrorAction SilentlyContinue) { Fail "services still registered" }
  $left = @(
    "$env:ProgramData\burrow\e2e-server.yaml",
    "$env:ProgramData\burrow\e2e-client.yaml",
    "$env:ProgramData\burrow\e2e-client.env",
    "$env:ProgramFiles\burrow\burrow.exe",
    $log
  ) | Where-Object { Test-Path $_ }
  if ($left) { Fail "still there: $left" }
  Check "nothing left behind"
  Write-Host "service end-to-end checks passed"
  $Passed = $true
}
catch {
  if ($env:GITHUB_ACTIONS) {
    Write-Host "::error title=service e2e exception::$(($_ | Out-String) -replace "`r?`n", ' ') at line $($_.InvocationInfo.ScriptLineNumber)"
  }
  throw
}
finally {
  $ErrorActionPreference = "Continue"
  & $B service uninstall burrow-e2e-client --purge *> $null
  & $B service uninstall burrow-e2e-server --purge *> $null
  if ($Echo) { Stop-Process -Id $Echo.Id -Force -ErrorAction SilentlyContinue }
  Set-Location $Root
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
if ($Passed) { exit 0 } else { exit 1 }
