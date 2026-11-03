# Remote client updates through the Windows service manager, with rollback. Run as administrator.
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
$Name = "burrow-e2e-upd"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("burrow-upd-" + [guid]::NewGuid())
New-Item -ItemType Directory $Work | Out-Null
$Procs = @()

function Check($msg) { Write-Host "  ok: $msg" }
function Fail($msg) {
  Write-Host "FAIL: $msg"
  & "$Work/v1.exe" service logs $Name -n 40 2>&1 | Out-String | Write-Host
  Get-Content "$env:ProgramData\burrow\updates\e2e-upd\updater.log" -ErrorAction SilentlyContinue | Write-Host
  throw $msg
}
function Mark($file, [string[]]$marks) {
  $bytes = [IO.File]::ReadAllBytes((Join-Path $Out "burrow.exe"))
  $tail = "`n" + (($marks | ForEach-Object { "BURROW-TEST-$_`n" }) -join "")
  [IO.File]::WriteAllBytes((Join-Path $Work $file), $bytes + [Text.Encoding]::ASCII.GetBytes($tail))
}
function HomeClient {
  $s = & $Probe ops.yaml status 2>$null | Out-String
  if (-not $s) { return $null }
  (ConvertFrom-Json $s).clients | Where-Object { $_.name -eq "home" }
}
function WaitFor($what, [scriptblock]$ok) {
  for ($i = 0; $i -lt 300; $i++) { $c = HomeClient; if ($c -and (& $ok $c)) { return $c }; Start-Sleep -Milliseconds 500 }
  Fail "$what : $(HomeClient | ConvertTo-Json -Compress)"
}
function Ask($v) {
  $Ctl.WriteLine("latest $v"); $Ctl.Flush()
  WaitFor "the server to see $v" { param($c) $c.available -eq $v } | Out-Null
  & $Probe ops.yaml call update_clients '{"clients":["home"]}' | Out-Null
  if ($LASTEXITCODE -ne 0) { Fail "ask $v" }
}
function Installed { & "$env:ProgramFiles\burrow\burrow.exe" version }

try {
  Set-Location $Work
  Mark "v1.exe" @("VERSION=9.0.0-test.1")
  Mark "v2.exe" @("VERSION=9.0.0-test.2")
  Mark "v3.exe" @("VERSION=9.0.0-test.3")
  Mark "v4.exe" @("VERSION=9.0.0-test.4", "BREAK=connect")
  Mark "v5.exe" @("VERSION=9.0.0-test.5", "BREAK=start")
  Mark "srv.exe" @("VERSION=9.9.9")
  $psi = New-Object Diagnostics.ProcessStartInfo $Serve
  $psi.Arguments = "9.0.0-test.2 v2.exe good 9.0.0-test.3 v3.exe tampered 9.0.0-test.4 v4.exe good 9.0.0-test.5 v5.exe good"
  $psi.WorkingDirectory = $Work
  $psi.UseShellExecute = $false
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $server = [Diagnostics.Process]::Start($psi)
  $Procs += $server
  $Ctl = $server.StandardInput
  $Url = $server.StandardOutput.ReadLine()
  if (-not $Url) { Fail "the release server did not start" }

  $HomeSecret = (& "$Work/v1.exe" gensecret).Trim()
  $OpsSecret = (& "$Work/v1.exe" gensecret).Trim()
  Set-Content server.yaml "listen: 127.0.0.1:27975`nclients:`n  home:`n    secret: $HomeSecret`nvisitors:`n  ops:`n    secret: $OpsSecret`n    admin: write"
  Set-Content ops.yaml "server: 127.0.0.1:27975`nname: ops`nsecret: $OpsSecret"
  Set-Content e2e-upd.yaml "server: 127.0.0.1:27975`nname: home`nsecret: `${E2E_SECRET}`nallow: [127.0.0.1/32]`nheaders:`n  X-Releases: `${BURROW_UPDATE_URL}`n  X-Window: `${BURROW_ADMIN_TEST_UPDATE_WINDOW}`nupdate:`n  remote: true"
  $env:BURROW_UPDATE_URL = $Url
  $env:BURROW_ADMIN_TEST_LATEST_SECS = "0"
  $Procs += Start-Process -FilePath (Join-Path $Work "srv.exe") -ArgumentList "server","-c","server.yaml" -PassThru -WindowStyle Hidden -RedirectStandardError server.err

  Write-Host "== install the client as a service that allows remote updates"
  $env:E2E_SECRET = $HomeSecret
  $env:BURROW_ADMIN_TEST_UPDATE_WINDOW = "8"
  $out = & "$Work/v1.exe" service install -c e2e-upd.yaml 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { Fail "install: $out" }
  if ($out -notmatch "the server's admin may ask it to update") { Fail "install does not mention the updater: $out" }
  $c = WaitFor "the old version reports" { param($c) $c.build.version -eq "9.0.0-test.1" }
  if ($c.build.remote -ne "on" -or $c.build.run -ne "service") { Fail "not a service with remote updates: $($c | ConvertTo-Json -Compress)" }
  $acl = icacls "$env:ProgramData\burrow\updates\e2e-upd" | Out-String
  if ($acl -match "e2e-upd:\(OI\)\(CI\)\((M|F|W)" -or $acl -match "e2e-upd:\((M|F|W)") { Fail "the client account may change the updater's folder: $acl" }
  $own = icacls "$env:ProgramData\burrow\updates\e2e-upd\own" | Out-String
  $mine = ($own -split "`n") | Where-Object { $_ -match "e2e-upd:" }
  if (-not ($mine -match "\(RX,WD,AD\)") -or -not ($mine -match "\(OI\)\(CI\)\(IO\)\(M\)")) { Fail "unexpected rights on its own folder: $own" }
  if ($mine | Where-Object { $_ -notmatch "\(IO\)" -and $_ -match "\((M|F|DE)[,)]" }) { Fail "the client account may replace its own folder: $own" }
  Check "the client reports 9.0.0-test.1 as a service; it may only read the updater's folder and fill, not replace, its own"

  Write-Host "== update to 9.0.0-test.2"
  Ask "9.0.0-test.2"
  WaitFor "the update" { param($c) $c.update.state -eq "done" } | Out-Null
  if ((Installed) -notmatch "9.0.0-test.2") { Fail "installed: $(Installed)" }
  Check "updated to 9.0.0-test.2 through the service manager"

  Write-Host "== a download that does not match its checksum changes nothing"
  Ask "9.0.0-test.3"
  $c = WaitFor "the failure" { param($c) $c.update.state -eq "failed" }
  if ($c.update.detail -notmatch "SHA-256") { Fail "no checksum reason: $($c | ConvertTo-Json -Compress)" }
  if ((Installed) -notmatch "9.0.0-test.2") { Fail "the binary changed: $(Installed)" }
  Check "checksum mismatch: nothing changed"

  Write-Host "== a version that does not connect is rolled back"
  Ask "9.0.0-test.4"
  WaitFor "the rollback" { param($c) $c.update.state -eq "rolled-back" } | Out-Null
  WaitFor "the old version back" { param($c) $c.build.version -eq "9.0.0-test.2" } | Out-Null
  Check "no connection: rolled back to 9.0.0-test.2"

  Write-Host "== a version that keeps crashing is rolled back"
  Ask "9.0.0-test.5"
  WaitFor "the rollback" { param($c) $c.update.state -eq "rolled-back" } | Out-Null
  WaitFor "the old version back" { param($c) $c.build.version -eq "9.0.0-test.2" } | Out-Null
  Check "crash loop: rolled back to 9.0.0-test.2"

  Write-Host "== uninstall removes the updater"
  & "$Work/v1.exe" service uninstall $Name --purge | Out-Null
  sc.exe query "$Name-update" | Out-Null
  if ($LASTEXITCODE -eq 0) { Fail "the updater service is left" }
  if (Test-Path "$env:ProgramData\burrow\updates\e2e-upd") { Fail "the updater's folder is left" }
  Check "updater removed"
  Write-Host "remote-update e2e: all ok"
} finally {
  & "$Work/v1.exe" service uninstall $Name --purge 2>&1 | Out-Null
  foreach ($p in $Procs) { try { $p.Kill() } catch {} }
  Set-Location $env:TEMP
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
