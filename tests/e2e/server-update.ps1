# Server updates from burrow admin through the Windows service manager, with the health check and rollback. Run as administrator.
# Builds with the admin-test-hooks feature, so the binaries can pretend to be other versions.
# $env:BURROW_E2E_BIN = DIR uses burrow.exe, admin_probe.exe and release_server.exe from DIR instead of building them.
$ErrorActionPreference = "Stop"
$Root = Resolve-Path "$PSScriptRoot/../.."
if ($env:BURROW_E2E_BIN) {
  $Out = $env:BURROW_E2E_BIN
  $Probe = Join-Path $Out "admin_probe.exe"
  $Serve = Join-Path $Out "release_server.exe"
} else {
  cargo build --manifest-path "$Root/Cargo.toml" --features admin-test-hooks --bin burrow --example admin_probe --example release_server -q
  if ($LASTEXITCODE -ne 0) { throw "build failed" }
  $Out = Join-Path $Root "target/debug"
  $Probe = Join-Path $Out "examples/admin_probe.exe"
  $Serve = Join-Path $Out "examples/release_server.exe"
}
$Name = "burrow-e2e-srv"
$Mate = "burrow-e2e-srvmate"
$Updates = "$env:ProgramData\burrow\updates\e2e-srv"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("burrow-srv-" + [guid]::NewGuid())
New-Item -ItemType Directory $Work | Out-Null
$Procs = @()

function Check($msg) { Write-Host "  ok: $msg" }
function Fail($msg) {
  Write-Host "FAIL: $msg"
  & "$Work/s1.exe" service logs $Name -n 40 2>&1 | Out-String | Write-Host
  Get-Content "$Updates\updater.log" -ErrorAction SilentlyContinue | Write-Host
  Get-Content "$Updates\result.json" -ErrorAction SilentlyContinue | Write-Host
  throw $msg
}
function Mark($file, [string[]]$marks) {
  $bytes = [IO.File]::ReadAllBytes((Join-Path $Out "burrow.exe"))
  $tail = "`n" + (($marks | ForEach-Object { "BURROW-TEST-$_`n" }) -join "")
  [IO.File]::WriteAllBytes((Join-Path $Work $file), $bytes + [Text.Encoding]::ASCII.GetBytes($tail))
}
function Own {
  $s = & $Probe ops.yaml status 2>$null | Out-String
  if (-not $s) { return $null }
  try { ConvertFrom-Json $s } catch { $null }
}
function WaitFor($what, [scriptblock]$ok) {
  for ($i = 0; $i -lt 400; $i++) { $s = Own; if ($s -and (& $ok $s)) { return $s }; Start-Sleep -Milliseconds 500 }
  Fail "$what : $(Own | ConvertTo-Json -Compress -Depth 6)"
}
function Latest($v) {
  $Ctl.WriteLine("latest $v"); $Ctl.Flush()
  WaitFor "the server to see $v" { param($s) $s.latest.version -eq $v } | Out-Null
}
function Ask { $r = & $Probe ops.yaml call update_server '{}' 2>&1 | Out-String; return @($LASTEXITCODE, $r) }
function Manifest { Get-Content "$env:ProgramData\burrow\services\$Name.json" | ConvertFrom-Json }
function Installed { & (Manifest).binary version }
function ServerYaml($extra) {
  "listen: 127.0.0.1:27985`nclients:`n  home:`n    secret: `${E2E_HOME}`nvisitors:`n  ops:`n    secret: `${E2E_OPS}`n    admin: write`nservices:`n  app: { client: home, bind: 127.0.0.1:27986, target: 127.0.0.1:27987 }`nweb:`n  https: 127.0.0.1:27988`n  request_headers:`n    X-Releases: `${BURROW_UPDATE_URL}`n    X-Window: `${BURROW_ADMIN_TEST_UPDATE_WINDOW}`n    X-Latest: `${BURROW_ADMIN_TEST_LATEST_SECS}`n$extra"
}

try {
  Set-Location $Work
  Mark "s1.exe" @("VERSION=9.0.0-test.1")
  Mark "s2.exe" @("VERSION=9.0.0-test.2")
  Mark "s3.exe" @("VERSION=9.0.0-test.3", "BREAK=config")
  Mark "s4.exe" @("VERSION=9.0.0-test.4", "BREAK=bind")
  Mark "s5.exe" @("VERSION=9.0.0-test.5", "BREAK=start")
  Mark "s6.exe" @("VERSION=9.0.0-test.6", "BREAK=status")
  Mark "c1.exe" @("VERSION=9.0.0-test.1")
  $psi = New-Object Diagnostics.ProcessStartInfo $Serve
  $psi.Arguments = "9.0.0-test.2 s2.exe good 9.0.0-test.3 s3.exe good 9.0.0-test.4 s4.exe good 9.0.0-test.5 s5.exe good 9.0.0-test.6 s6.exe good"
  $psi.WorkingDirectory = $Work
  $psi.UseShellExecute = $false
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $rel = [Diagnostics.Process]::Start($psi)
  $Procs += $rel
  $Ctl = $rel.StandardInput
  $Url = $rel.StandardOutput.ReadLine()
  if (-not $Url) { Fail "the release server did not start" }
  $Ctl.WriteLine("latest 9.0.0-test.1"); $Ctl.Flush()

  $HomeSecret = (& "$Work/s1.exe" gensecret).Trim()
  $OpsSecret = (& "$Work/s1.exe" gensecret).Trim()
  Set-Content e2e-srv.yaml (ServerYaml "")
  Set-Content ops.yaml "server: 127.0.0.1:27985`nname: ops`nsecret: $OpsSecret"
  Set-Content home.yaml "server: 127.0.0.1:27985`nname: home`nsecret: $HomeSecret`nallow: [127.0.0.1/32]"
  $env:E2E_HOME = $HomeSecret
  $env:E2E_OPS = $OpsSecret
  $env:BURROW_UPDATE_URL = $Url
  $env:BURROW_ADMIN_TEST_UPDATE_WINDOW = "20"
  $env:BURROW_ADMIN_TEST_LATEST_SECS = "0"

  Write-Host "== a server installed without the opt-in refuses, with how to turn it on"
  $out = & "$Work/s1.exe" service install -c e2e-srv.yaml 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { Fail "install: $out" }
  $Procs += Start-Process -FilePath (Join-Path $Work "c1.exe") -ArgumentList "client","-c","home.yaml" -PassThru -WindowStyle Hidden -RedirectStandardError home.err
  WaitFor "the server reports itself" { param($s) $s.version -eq "9.0.0-test.1" } | Out-Null
  Latest "9.0.0-test.2"
  $s = Own
  if ($s.server_update.remote -ne "off" -or $s.server_update.why_not -notmatch "update.remote: true") { Fail "not off with the hint: $($s.server_update | ConvertTo-Json -Compress)" }
  $rc, $r = Ask
  if ($rc -eq 0) { Fail "an update was asked without the opt-in: $r" }
  Check "no opt-in: refused with the hint"

  Write-Host "== update.remote: true written by the server account alone counts for nothing"
  Add-Content (Manifest).config "update:`n  remote: true"
  & "$Work/s1.exe" service restart $Name | Out-Null
  WaitFor "the server back" { param($s) $s.server_update.why_not -match "counts only once root installs" } | Out-Null
  $rc, $r = Ask
  if ($rc -eq 0) { Fail "an update was asked on the file alone: $r" }
  & "$Work/s1.exe" service remote-update $Name 2>&1 | Out-Null
  if ((Get-Content "$Updates\result.json" -Raw) -notmatch "installed without remote updates") { Fail "the root updater trusted server.yaml" }
  if ((Installed) -notmatch "9.0.0-test.1") { Fail "the binary changed: $(Installed)" }
  Check "server.yaml alone: refused by the server and by the root updater"

  Write-Host "== install with update.remote: true adds the updater"
  Set-Content e2e-srv.yaml (ServerYaml "update: { remote: true }")
  $out = & "$Work/s1.exe" service install -c e2e-srv.yaml 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { Fail "install: $out" }
  if ($out -notmatch "burrow mcp may update it") { Fail "install does not mention the updater: $out" }
  WaitFor "remote updates on" { param($s) $s.server_update.remote -eq "on" } | Out-Null
  Check "remote updates on after an administrator install"

  Write-Host "== a binary shared with a service that did not opt in is never replaced"
  Set-Content e2e-srvmate.yaml "server: 127.0.0.1:27985`nname: home`nsecret: $HomeSecret`nallow: [127.0.0.1/32]"
  $out = & "$Work/s1.exe" service install -c e2e-srvmate.yaml --no-start 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { Fail "install the mate: $out" }
  $rc, $r = Ask
  if ($rc -ne 0) { Fail "ask: $r" }
  $s = WaitFor "the refusal" { param($s) $s.server_update.last.state -eq "failed" }
  if ($s.server_update.last.detail -notmatch "did not allow remote updates") { Fail "no reason: $($s.server_update | ConvertTo-Json -Compress)" }
  if ((Installed) -notmatch "9.0.0-test.1") { Fail "the binary changed: $(Installed)" }
  & "$Work/s1.exe" service uninstall $Mate --purge | Out-Null
  Check "shared with a service without the opt-in: refused, nothing changed"

  Write-Host "== update to 9.0.0-test.2"
  $rc, $r = Ask
  if ($rc -ne 0 -or $r -notmatch '"asked":true') { Fail "not asked: $r" }
  $rc2, $r2 = Ask
  if ($rc2 -eq 0) { Fail "a second job was accepted: $r2" }
  $s = WaitFor "the update" { param($s) $s.server_update.last.state -eq "done" -and $s.version -eq "9.0.0-test.2" }
  if ($s.server_update.last.by -notmatch "^ops \(") { Fail "who asked is not recorded" }
  if ($s.server_update.last.detail -notmatch "1 of 1 clients connected again") { Fail "no reconnect count: $($s.server_update.last.detail)" }
  if ((Installed) -notmatch "9.0.0-test.2") { Fail "installed: $(Installed)" }
  Check "updated to 9.0.0-test.2, asked by ops, the client came back"

  foreach ($case in @(@("9.0.0-test.3", "config refused"), @("9.0.0-test.4", "a port that cannot bind"), @("9.0.0-test.5", "a crash at start"), @("9.0.0-test.6", "a server that runs but fails the health check"))) {
    $v = $case[0]; $what = $case[1]
    Write-Host "== $what is rolled back"
    Latest $v
    $rc, $r = Ask
    if ($rc -ne 0) { Fail "ask $v : $r" }
    $s = WaitFor "the rollback" { param($s) $s.server_update.last.state -eq "rolled-back" -and $s.version -eq "9.0.0-test.2" }
    if ($s.server_update.last.detail -notmatch "restored 9.0.0-test.2") { Fail "no reason: $($s.server_update.last.detail)" }
    if ((Installed) -notmatch "9.0.0-test.2") { Fail "the binary was not restored: $(Installed)" }
    Check "$what : rolled back to 9.0.0-test.2 ($($s.server_update.last.detail))"
  }

  Write-Host "== uninstall removes the updater"
  & "$Work/s1.exe" service uninstall $Name --purge | Out-Null
  sc.exe query "$Name-update" | Out-Null
  if ($LASTEXITCODE -eq 0) { Fail "the updater service is left" }
  if (Test-Path $Updates) { Fail "the updater's folder is left" }
  Check "updater removed"
  Write-Host "server-update e2e: all ok"
} finally {
  & "$Work/s1.exe" service uninstall $Mate --purge 2>&1 | Out-Null
  & "$Work/s1.exe" service uninstall $Name --purge 2>&1 | Out-Null
  foreach ($p in $Procs) { try { $p.Kill() } catch {} }
  Set-Location $env:TEMP
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
