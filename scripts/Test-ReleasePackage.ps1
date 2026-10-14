#Requires -Version 7.0
<#
.SYNOPSIS
  Verifier and bounded self-tests for scripts/New-ReleasePackage.ps1.

.DESCRIPTION
  Three modes (no Pester, no network, PowerShell 7 only):

    -Mode SelfTest   (default) fixture-based self-tests. Small synthetic git
                     fixture repos are created under
                     docs/release-work/fixtures/run-*/; nothing in the real
                     repository history is touched and no real cargo build is
                     needed (fixtures pass an explicitly provided executable).
                     Real failure cases are exercised: missing LICENSE,
                     missing vendored license, missing crate source, overwrite
                     attempts, dirty-tree policy, out-of-workspace output,
                     stale/non-PE executable, tampered package files, tampered
                     or malformed manifests, missing/extra package files,
                     source-zip and crate-zip gaps, unicode/space paths.

    -Mode Verify     -PackageDir <dir> [-Level Quick|Full]
                     Verify an existing local-draft package. Quick: structure,
                     both manifests, provenance cross-checks, zip entry sets.
                     Full: additionally decompresses every source-zip entry and
                     compares its SHA-256 against SOURCE_MANIFEST.sha256.

    -Mode Smoke      Run the packager against the current repository (real
                     locked offline all-features build), then verify the result
                     at Full level; evidence is written under
                     docs/release-work/logs/.

  Exit codes:
    0 success / all tests passed / package verified
    1 verification failed, tests failed, or smoke step failed
    2 invalid arguments
#>
[CmdletBinding()]
param(
    [ValidateSet('SelfTest', 'Verify', 'Smoke')]
    [string]$Mode = 'SelfTest',

    [string]$PackageDir = '',

    [ValidateSet('Quick', 'Full')]
    [string]$Level = 'Full',

    [string]$LogDir = '',

    # Smoke mode only: pass -AllowDirty through to the packager so a working
    # tree with uncommitted tracked edits can still be packaged, with the
    # dirty state recorded truthfully in PROVENANCE.json.
    [switch]$AllowDirty
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$script:SelfPath = $PSCommandPath
$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$script:Packager = Join-Path $PSScriptRoot 'New-ReleasePackage.ps1'
if (-not $LogDir) {
    $LogDir = Join-Path $script:RepoRoot 'docs\release-work\logs'
}

function Write-Err([string]$Message) {
    [Console]::Error.WriteLine("Error: $Message")
}

function Get-Sha256Hex([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $fs = [System.IO.File]::OpenRead($Path)
        try { $hash = $sha.ComputeHash($fs) }
        finally { $fs.Dispose() }
    }
    finally { $sha.Dispose() }
    return ([System.BitConverter]::ToString($hash) -replace '-', '').ToLowerInvariant()
}

function Get-StreamSha256Hex([System.IO.Stream]$Stream) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { $hash = $sha.ComputeHash($Stream) }
    finally { $sha.Dispose() }
    return ([System.BitConverter]::ToString($hash) -replace '-', '').ToLowerInvariant()
}

function Write-Utf8NoBom([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Test-ZipEntryName([string]$Name) {
    if ([string]::IsNullOrEmpty($Name)) { return $false }
    if ($Name.StartsWith('/')) { return $false }
    if ($Name -match '\\') { return $false }
    if ($Name -match '^[A-Za-z]:') { return $false }
    if ($Name -match '(^|/)\.\.(/|$)') { return $false }
    return $true
}

# ---------------------------------------------------------------------------
# Package verification core (shared by -Mode Verify, -Mode Smoke and the
# fixture self-tests).
# ---------------------------------------------------------------------------

function Test-ReleasePackageCore {
    param(
        [Parameter(Mandatory)][string]$Dir,
        [ValidateSet('Quick', 'Full')][string]$CheckLevel = 'Full'
    )
    $reasons = [System.Collections.Generic.List[string]]::new()
    if (-not (Test-Path -LiteralPath $Dir -PathType Container)) {
        return [pscustomobject]@{ Ok = $false; Reasons = @("package directory not found: $Dir") }
    }
    $root = (Get-Item -LiteralPath $Dir).FullName

    $exeName = $null
    if (Test-Path -LiteralPath (Join-Path $root 'toss.exe') -PathType Leaf) { $exeName = 'toss.exe' }
    elseif (Test-Path -LiteralPath (Join-Path $root 'toss') -PathType Leaf) { $exeName = 'toss' }
    else { [void]$reasons.Add('missing executable (toss.exe/toss)') }

    $requiredFiles = @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'SOURCE_MANIFEST.sha256', 'MANIFEST.sha256', 'PROVENANCE.json', 'LOCAL-DRAFT.md', 'source/toss-0.1.0-source.zip', 'source/toss-0.1.0-crate-sources.zip')
    foreach ($rel in $requiredFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $root ($rel -replace '/', '\')) -PathType Leaf)) {
            [void]$reasons.Add("missing required package file: $rel")
        }
    }
    if ($reasons.Count -gt 0) {
        return [pscustomobject]@{ Ok = $false; Reasons = @($reasons) }
    }

    # --- MANIFEST.sha256: strict format, safe paths, full coverage ---------
    $manifestPath = Join-Path $root 'MANIFEST.sha256'
    $manifestMap = [System.Collections.Generic.Dictionary[string, string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $manifestRaw = [System.IO.File]::ReadAllText($manifestPath)
    $manifestLines = $manifestRaw -split "`n"
    if ($manifestLines.Count -gt 0 -and $manifestLines[-1] -eq '') { $manifestLines = $manifestLines[0..($manifestLines.Count - 2)] }
    elseif ($manifestLines.Count -gt 0) { [void]$reasons.Add('MANIFEST.sha256 does not end with a newline') }
    $lineNo = 0
    foreach ($line in $manifestLines) {
        $lineNo++
        if ($line -notmatch '^(?<hash>[0-9a-f]{64})  (?<path>.+)$') {
            [void]$reasons.Add("MANIFEST.sha256 line $lineNo is malformed: '$line'")
            continue
        }
        $p = $Matches['path']
        if ($p.StartsWith('/') -or $p -match '\\' -or $p -match '(^|/)\.\.(/|$)' -or $p -match '^[A-Za-z]:') {
            [void]$reasons.Add("MANIFEST.sha256 line $lineNo has an unsafe path: '$p'")
            continue
        }
        if ($manifestMap.ContainsKey($p)) {
            [void]$reasons.Add("MANIFEST.sha256 lists '$p' more than once")
            continue
        }
        $manifestMap[$p] = $Matches['hash']
    }

    $diskFiles = [System.Collections.Generic.List[string]]::new()
    foreach ($f in (Get-ChildItem -LiteralPath $root -File -Recurse)) {
        $rel = $f.FullName.Substring($root.Length).TrimStart('\').Replace('\', '/')
        if ($rel -eq 'MANIFEST.sha256') { continue }
        [void]$diskFiles.Add($rel)
    }
    foreach ($rel in $diskFiles) {
        if (-not $manifestMap.ContainsKey($rel)) {
            [void]$reasons.Add("file present in package but not recorded in MANIFEST.sha256 (tamper or extra file): $rel")
        }
    }
    foreach ($key in @($manifestMap.Keys)) {
        if ($diskFiles -notcontains $key) {
            [void]$reasons.Add("MANIFEST.sha256 records a file that is missing from the package: $key")
        }
    }
    foreach ($kv in $manifestMap.GetEnumerator()) {
        $full = Join-Path $root ($kv.Key -replace '/', '\')
        if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { continue }
        $actual = Get-Sha256Hex $full
        if ($actual -ne $kv.Value) {
            [void]$reasons.Add("hash mismatch (tampered file): $($kv.Key) manifest=$($kv.Value) actual=$actual")
        }
    }

    # --- SOURCE_MANIFEST.sha256: strict format ----------------------------
    $srcManifestPath = Join-Path $root 'SOURCE_MANIFEST.sha256'
    $srcManifestBytes = [System.IO.File]::ReadAllBytes($srcManifestPath)
    $srcManifestHex = ([System.BitConverter]::ToString(
            [System.Security.Cryptography.SHA256]::Create().ComputeHash($srcManifestBytes)) -replace '-', '').ToLowerInvariant()
    $srcMap = [System.Collections.Generic.Dictionary[string, string]]::new([System.StringComparer]::Ordinal)
    $srcLines = [System.Text.Encoding]::UTF8.GetString($srcManifestBytes) -split "`n"
    if ($srcLines.Count -gt 0 -and $srcLines[-1] -eq '') { $srcLines = $srcLines[0..($srcLines.Count - 2)] }
    $srcLineNo = 0
    foreach ($line in $srcLines) {
        $srcLineNo++
        if ($line -notmatch '^(?<hash>[0-9a-f]{64})  (?<path>.+)$') {
            [void]$reasons.Add("SOURCE_MANIFEST.sha256 line $srcLineNo is malformed: '$line'")
            continue
        }
        $p = $Matches['path']
        if ($p.StartsWith('/') -or $p -match '\\' -or $p -match '(^|/)\.\.(/|$)') {
            [void]$reasons.Add("SOURCE_MANIFEST.sha256 line $srcLineNo has an unsafe path: '$p'")
            continue
        }
        if ($srcMap.ContainsKey($p)) {
            [void]$reasons.Add("SOURCE_MANIFEST.sha256 lists '$p' more than once")
            continue
        }
        $srcMap[$p] = $Matches['hash']
    }

    # --- PROVENANCE.json ---------------------------------------------------
    $prov = $null
    try {
        $prov = Get-Content -LiteralPath (Join-Path $root 'PROVENANCE.json') -Raw | ConvertFrom-Json
    }
    catch {
        [void]$reasons.Add("PROVENANCE.json is not valid JSON: $($_.Exception.Message)")
    }
    if ($null -ne $prov) {
        if ($prov.schema -ne 'toss-local-draft-package/1') {
            [void]$reasons.Add("unexpected provenance schema: $($prov.schema)")
        }
        $claims = $prov.claims
        if ($null -eq $claims) {
            [void]$reasons.Add('PROVENANCE.json is missing the claims block')
        }
        else {
            if ($claims.local_draft -ne $true) { [void]$reasons.Add('claims.local_draft must be true') }
            if ($claims.legal_clearance -ne $false) { [void]$reasons.Add('claims.legal_clearance must be false (no fake legal clearance)') }
            if ($claims.reproducibility_guarantee -ne $false) { [void]$reasons.Add('claims.reproducibility_guarantee must be false') }
            if ($claims.byte_identical_rebuild_claimed -ne $false) { [void]$reasons.Add('claims.byte_identical_rebuild_claimed must be false') }
            if ($claims.testing_checklists_asserted -ne $false) { [void]$reasons.Add('claims.testing_checklists_asserted must be false') }
        }
        $conditions = @($prov.open_decision_conditions)
        $conditionText = $conditions -join ' | '
        if ($conditionText -notmatch 'R1' -or $conditionText -notmatch 'M1') {
            [void]$reasons.Add('open_decision_conditions must keep the R1 and M1 static-CRT questions')
        }
        if ($prov.repo.head_commit -notmatch '^[0-9a-f]{7,40}$') {
            [void]$reasons.Add("repo.head_commit is not a commit id: $($prov.repo.head_commit)")
        }
        $policy = $prov.repo.dev_state_policy
        if ($policy -notin @('strict', 'allow-dirty')) {
            [void]$reasons.Add("repo.dev_state_policy must be strict or allow-dirty: $policy")
        }
        if ($prov.repo.git_dirty -eq $true -and $policy -ne 'allow-dirty') {
            [void]$reasons.Add('inconsistent provenance: git_dirty=true under the strict policy')
        }
        if ($prov.build.mode -notin @('cargo', 'provided-executable')) {
            [void]$reasons.Add("unknown build.mode: $($prov.build.mode)")
        }
        if ($prov.build.mode -eq 'cargo' -and $prov.build.exit_code -ne 0) {
            [void]$reasons.Add("cargo build provenance claims exit $($prov.build.exit_code), not 0")
        }
        if ($prov.freshness.passed -ne $true) {
            [void]$reasons.Add('freshness.passed must be true in a packaged draft')
        }
        if ($exeName -and $prov.executable.path_in_package -ne $exeName) {
            [void]$reasons.Add("provenance executable name mismatch: $($prov.executable.path_in_package) vs $exeName")
        }
        if ($exeName) {
            $exeActual = Get-Sha256Hex (Join-Path $root $exeName)
            if ($prov.executable.sha256 -ne $exeActual) {
                [void]$reasons.Add('provenance executable sha256 does not match the packaged executable')
            }
            if ($prov.executable.size_bytes -ne (Get-Item -LiteralPath (Join-Path $root $exeName)).Length) {
                [void]$reasons.Add('provenance executable size does not match the packaged executable')
            }
        }
        if ($prov.source.file_count -ne $srcMap.Count) {
            [void]$reasons.Add("provenance source.file_count $($prov.source.file_count) != SOURCE_MANIFEST entries $($srcMap.Count)")
        }
        if ($prov.source.content_sha256 -ne $srcManifestHex) {
            [void]$reasons.Add('provenance source.content_sha256 does not match SOURCE_MANIFEST.sha256')
        }
        foreach ($art in @($prov.artifacts)) {
            $artFull = Join-Path $root ($art.path -replace '/', '\')
            if (-not (Test-Path -LiteralPath $artFull -PathType Leaf)) {
                [void]$reasons.Add("provenance artifact missing from package: $($art.path)")
                continue
            }
            if ((Get-Item -LiteralPath $artFull).Length -ne $art.size_bytes) {
                [void]$reasons.Add("provenance artifact size mismatch: $($art.path)")
            }
            if ((Get-Sha256Hex $artFull) -ne $art.sha256) {
                [void]$reasons.Add("provenance artifact hash mismatch: $($art.path)")
            }
        }
    }

    # --- license text sanity ----------------------------------------------
    $licenseText = [System.IO.File]::ReadAllText((Join-Path $root 'LICENSE'))
    if ($licenseText -notmatch 'GNU GENERAL PUBLIC LICENSE' -or $licenseText -notmatch 'Version 3') {
        [void]$reasons.Add('LICENSE is not recognizable GPL-3.0 text')
    }
    $noticesText = [System.IO.File]::ReadAllText((Join-Path $root 'THIRD_PARTY_NOTICES.md'))
    if ($noticesText -notmatch 'GPL-3\.0-or-later') {
        [void]$reasons.Add('THIRD_PARTY_NOTICES.md does not declare GPL-3.0-or-later')
    }
    if ($exeName) {
        $fs = [System.IO.File]::OpenRead((Join-Path $root $exeName))
        try {
            $b = [byte[]]::new(2)
            [void]$fs.Read($b, 0, 2)
            if (-not ($b[0] -eq 0x4D -and $b[1] -eq 0x5A)) {
                [void]$reasons.Add('packaged executable does not start with the MZ signature')
            }
        }
        finally { $fs.Dispose() }
    }

    # --- source zip vs SOURCE_MANIFEST ------------------------------------
    $srcZipPath = Join-Path $root 'source\toss-0.1.0-source.zip'
    try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($srcZipPath)
        try {
            $matched = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
            foreach ($entry in $zip.Entries) {
                if (-not (Test-ZipEntryName $entry.FullName)) {
                    [void]$reasons.Add("source zip has an unsafe entry name: '$($entry.FullName)'")
                    continue
                }
                if (-not $entry.FullName.StartsWith('toss-source/')) {
                    [void]$reasons.Add("source zip entry outside toss-source/: '$($entry.FullName)'")
                    continue
                }
                $rel = $entry.FullName.Substring('toss-source/'.Length)
                if ($rel -eq '' -or $rel.EndsWith('/')) { continue }
                if (-not $srcMap.ContainsKey($rel)) {
                    [void]$reasons.Add("source zip contains a file not in SOURCE_MANIFEST.sha256: $rel")
                    continue
                }
                if (-not $matched.Add($rel)) {
                    [void]$reasons.Add("source zip contains a duplicate entry: $rel")
                    continue
                }
                if ($CheckLevel -eq 'Full') {
                    $es = $entry.Open()
                    try { $entryHash = Get-StreamSha256Hex $es }
                    finally { $es.Dispose() }
                    if ($entryHash -ne $srcMap[$rel]) {
                        [void]$reasons.Add("source zip entry hash mismatch: $rel")
                    }
                }
            }
            foreach ($key in @($srcMap.Keys)) {
                if (-not $matched.Contains($key)) {
                    [void]$reasons.Add("SOURCE_MANIFEST entry missing from source zip: $key")
                }
            }
        }
        finally { $zip.Dispose() }
    }
    catch {
        [void]$reasons.Add("source zip unreadable: $($_.Exception.Message)")
    }

    # --- crate zip vs provenance ------------------------------------------
    $crateZipPath = Join-Path $root 'source\toss-0.1.0-crate-sources.zip'
    $expectedCrates = @{}
    if ($null -ne $prov) {
        foreach ($c in @($prov.crates)) {
            if ($c.name -and $c.version) { $expectedCrates[$c.dir] = @($c.license_files) }
        }
    }
    try {
        $czip = [System.IO.Compression.ZipFile]::OpenRead($crateZipPath)
        try {
            $foundCrates = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
            $foundLicenseEntries = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
            foreach ($entry in $czip.Entries) {
                if (-not (Test-ZipEntryName $entry.FullName)) {
                    [void]$reasons.Add("crate zip has an unsafe entry name: '$($entry.FullName)'")
                    continue
                }
                if (-not $entry.FullName.StartsWith('crate-sources/')) {
                    [void]$reasons.Add("crate zip entry outside crate-sources/: '$($entry.FullName)'")
                    continue
                }
                $rest = $entry.FullName.Substring('crate-sources/'.Length)
                $parts = $rest -split '/', 2
                if ($parts.Count -lt 2 -or $parts[1] -eq '') { continue }
                [void]$foundCrates.Add($parts[0])
                $leaf = ($parts[1] -split '/')[-1]
                if ($leaf -match '^(?i)(LICENSE|COPYING)') {
                    [void]$foundLicenseEntries.Add("$($parts[0])/$leaf")
                }
            }
            foreach ($crateDir in $expectedCrates.Keys) {
                if (-not $foundCrates.Contains($crateDir)) {
                    [void]$reasons.Add("crate sources zip is missing crate dir: $crateDir")
                }
                foreach ($lic in $expectedCrates[$crateDir]) {
                    if (-not $foundLicenseEntries.Contains("$crateDir/$lic")) {
                        [void]$reasons.Add("crate sources zip is missing license file: $crateDir/$lic")
                    }
                }
            }
            foreach ($found in $foundCrates) {
                if (-not $expectedCrates.ContainsKey($found)) {
                    [void]$reasons.Add("crate sources zip contains an unexpected crate dir: $found")
                }
            }
        }
        finally { $czip.Dispose() }
    }
    catch {
        [void]$reasons.Add("crate sources zip unreadable: $($_.Exception.Message)")
    }

    return [pscustomobject]@{ Ok = ($reasons.Count -eq 0); Reasons = @($reasons) }
}

function Invoke-VerifyAndReport {
    param([string]$Dir, [string]$CheckLevel)
    $result = Test-ReleasePackageCore -Dir $Dir -CheckLevel $CheckLevel
    if ($result.Ok) {
        Write-Output "verified: $Dir (level=$CheckLevel)"
        return 0
    }
    foreach ($r in $result.Reasons) { Write-Err $r }
    Write-Err "verification FAILED for $Dir ($($result.Reasons.Count) reason(s), level=$CheckLevel)"
    return 1
}

# ---------------------------------------------------------------------------
# Smoke mode: real repository, real locked offline build, real verification.
# ---------------------------------------------------------------------------

if ($Mode -eq 'Verify') {
    if (-not $PackageDir) {
        Write-Err '-Mode Verify requires -PackageDir'
        exit 2
    }
    exit (Invoke-VerifyAndReport -Dir $PackageDir -CheckLevel $Level)
}

if ($Mode -eq 'Smoke') {
    [void](New-Item -ItemType Directory -Force -Path $LogDir)
    $stamp = [System.DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
    $packLog = Join-Path $LogDir "smoke-pack-$stamp.log"
    $verLog = Join-Path $LogDir "smoke-verify-$stamp.log"
    $sumLog = Join-Path $LogDir "smoke-summary-$stamp.log"
    $lines = [System.Collections.Generic.List[string]]::new()

    Write-Output "smoke: packaging current repository via $script:Packager"
    $packArgs = @('-NoProfile', '-File', $script:Packager)
    if ($AllowDirty) { $packArgs += '-AllowDirty' }
    $packOut = & pwsh @packArgs 2>&1
    $packExit = $LASTEXITCODE
    $packText = (($packOut | ForEach-Object { $_.ToString() }) -join "`n")
    Write-Utf8NoBom -Path $packLog -Text ($packText + "`n")
    foreach ($l in ($packText -split "`r?`n")) { if ($l -ne '') { Write-Output $l } }

    $pkgDirLine = @($packText -split "`r?`n" | Where-Object { $_ -like 'package_dir=*' }) | Select-Object -First 1
    $pkgDir = if ($pkgDirLine) { ($pkgDirLine -replace '^package_dir=', '') } else { '' }

    $verExit = 1
    $verText = 'not run: packaging failed or produced no package_dir'
    if ($packExit -eq 0 -and $pkgDir) {
        Write-Output "smoke: verifying $pkgDir at level Full"
        $verResult = Test-ReleasePackageCore -Dir $pkgDir -CheckLevel 'Full'
        if ($verResult.Ok) { $verExit = 0 } else { $verExit = 1 }
        $verText = if ($verResult.Ok) { "verified OK: $pkgDir" } else { ($verResult.Reasons -join "`n") }
    }
    Write-Utf8NoBom -Path $verLog -Text ($verText + "`n")

    $manifestSha = ''
    $exeSha = ''
    if ($pkgDir -and (Test-Path -LiteralPath (Join-Path $pkgDir 'MANIFEST.sha256'))) {
        $manifestSha = Get-Sha256Hex (Join-Path $pkgDir 'MANIFEST.sha256')
        if (Test-Path -LiteralPath (Join-Path $pkgDir 'toss.exe')) {
            $exeSha = Get-Sha256Hex (Join-Path $pkgDir 'toss.exe')
        }
    }
    $summary = @"
smoke_utc=$stamp
packager=$script:Packager
pack_exit=$packExit
package_dir=$pkgDir
verify_level=Full
verify_exit=$verExit
manifest_sha256=$manifestSha
exe_sha256=$exeSha
pack_log=$packLog
verify_log=$verLog
note=local draft only; no legal clearance, no reproducibility guarantee; nothing published
"@
    Write-Utf8NoBom -Path $sumLog -Text ($summary + "`n")
    Write-Output $summary
    if ($packExit -eq 0 -and $verExit -eq 0) { exit 0 }
    exit 1
}

# ---------------------------------------------------------------------------
# SelfTest mode
# ---------------------------------------------------------------------------

[void](New-Item -ItemType Directory -Force -Path $LogDir)
$runStamp = [System.DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$randHex = -join ((1..3) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
$runRoot = Join-Path $script:RepoRoot "docs\release-work\fixtures\run-$runStamp-$randHex"
[void](New-Item -ItemType Directory -Force -Path $runRoot)
$detailLog = Join-Path $LogDir "selftest-$runStamp-detail.log"
$summaryLog = Join-Path $LogDir "selftest-$runStamp.log"
$script:DetailLines = [System.Collections.Generic.List[string]]::new()

function Add-Detail([string]$Text) {
    [void]$script:DetailLines.Add($Text)
}

function Invoke-Capture {
    param([string]$FilePath, [string[]]$ArgumentList = @(), [string]$WorkingDirectory = '')
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $FilePath
    foreach ($a in $ArgumentList) { [void]$psi.ArgumentList.Add($a) }
    if ($WorkingDirectory) { $psi.WorkingDirectory = $WorkingDirectory }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    $proc = [System.Diagnostics.Process]::Start($psi)
    $errTask = $proc.StandardError.ReadToEndAsync()
    $out = $proc.StandardOutput.ReadToEnd()
    $proc.WaitForExit()
    $err = $errTask.GetAwaiter().GetResult()
    return [pscustomobject]@{ Exit = $proc.ExitCode; Out = $out; Err = $err }
}

function Invoke-Packager {
    param([string[]]$PackArgs)
    Add-Detail "=== packager $($PackArgs -join ' ')"
    $res = Invoke-Capture -FilePath 'pwsh' -ArgumentList (@('-NoProfile', '-File', $script:Packager) + $PackArgs)
    Add-Detail $res.Out
    if ($res.Err) { Add-Detail "STDERR:`n$($res.Err)" }
    Add-Detail "exit=$($res.Exit)"
    return [pscustomobject]@{ Exit = $res.Exit; Out = $res.Out; Err = $res.Err; Text = ($res.Out + "`n" + $res.Err) }
}

function Assert {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "assertion failed: $Message" }
}

function Assert-Exit {
    param([int]$Got, [int]$Want, [string]$Text, [string]$What)
    if ($Got -ne $Want) {
        $tail = (($Text -split "`r?`n" | Where-Object { $_ -ne '' } | Select-Object -Last 8) -join ' | ')
        throw "$What : expected exit $Want, got $Got. tail: $tail"
    }
}

function New-FixtureFile {
    param([string]$Base, [string]$Rel, [string]$Text)
    $p = Join-Path $Base ($Rel -replace '/', '\')
    $dir = Split-Path -Parent $p
    if (-not (Test-Path -LiteralPath $dir)) { [void](New-Item -ItemType Directory -Force -Path $dir) }
    [System.IO.File]::WriteAllText($p, $Text, [System.Text.UTF8Encoding]::new($false))
}

function New-FixtureRepo {
    param([string]$Dir, [ValidateSet('Basic', 'NoLicense', 'NoVendoredLicense', 'MissingCrate')][string]$Variant = 'Basic')
    [void](New-Item -ItemType Directory -Force -Path $Dir)
    New-FixtureFile -Base $Dir -Rel '.gitignore' -Text "/target/`ndocs/`n*.log`n"
    if ($Variant -ne 'NoLicense') {
        New-FixtureFile -Base $Dir -Rel 'LICENSE' -Text "GNU GENERAL PUBLIC LICENSE`nVersion 3, 17 June 2007`nfixture placeholder text`n"
    }
    New-FixtureFile -Base $Dir -Rel 'THIRD_PARTY_NOTICES.md' -Text "# Notices`nFixture inventory. Licensed GPL-3.0-or-later.`n"
    New-FixtureFile -Base $Dir -Rel 'Cargo.toml' -Text "[package]`nname = `"toss`"`nversion = `"0.1.0`"`n"
    $crateName = 'unicode-ident'
    $crateVer = '1.0.26'
    if ($Variant -eq 'MissingCrate') {
        $crateName = 'toss-fixture-missing-crate'
        $crateVer = '9.9.9'
    }
    New-FixtureFile -Base $Dir -Rel 'Cargo.lock' -Text @"
version = 4

[[package]]
name = "toss"
version = "0.1.0"

[[package]]
name = "$crateName"
version = "$crateVer"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"
"@
    New-FixtureFile -Base $Dir -Rel 'src/main.rs' -Text "fn main() {}`n"
    New-FixtureFile -Base $Dir -Rel 'src/lib.rs' -Text "// fixture lib`n"
    New-FixtureFile -Base $Dir -Rel 'third_party/libarchive/README.md' -Text "fixture libarchive`n"
    if ($Variant -ne 'NoVendoredLicense') {
        New-FixtureFile -Base $Dir -Rel 'third_party/libarchive/COPYING' -Text "BSD-2-Clause fixture notice`n"
    }
    New-FixtureFile -Base $Dir -Rel 'third_party/zlib/LICENSE' -Text "zlib fixture license`n"
    New-FixtureFile -Base $Dir -Rel 'third_party/xz/COPYING.0BSD' -Text "0BSD fixture`n"
    New-FixtureFile -Base $Dir -Rel 'third_party/ffmpeg/COPYING.LGPLv2.1' -Text "LGPL-2.1 fixture`n"
    New-FixtureFile -Base $Dir -Rel 'docs/private-notes.md' -Text "private, must never be inventoried`n"
    New-FixtureFile -Base $Dir -Rel 'target/junk.txt' -Text "ignored build junk`n"

    $init = Invoke-Capture -FilePath 'git' -ArgumentList @('-C', $Dir, '-c', 'init.defaultBranch=main', 'init', '-q')
    if ($init.Exit -ne 0) { throw "git init failed: $($init.Err)" }
    $add = Invoke-Capture -FilePath 'git' -ArgumentList @('-C', $Dir, 'add', '-A')
    if ($add.Exit -ne 0) { throw "git add failed: $($add.Err)" }
    $commit = Invoke-Capture -FilePath 'git' -ArgumentList @('-C', $Dir, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-q', '-m', 'fixture baseline')
    if ($commit.Exit -ne 0) { throw "git commit failed: $($commit.Out) $($commit.Err)" }
}

function New-FakeExe {
    param([string]$Path, [datetime]$LastWrite = [datetime]::UtcNow)
    $bytes = [System.Collections.Generic.List[byte]]::new()
    $bytes.Add(0x4D); $bytes.Add(0x5A)
    foreach ($b in [System.Text.Encoding]::ASCII.GetBytes('fixture executable payload - not a real binary')) { $bytes.Add($b) }
    for ($i = 0; $i -lt 96; $i++) { $bytes.Add([byte](Get-Random -Maximum 256)) }
    $dir = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $dir)) { [void](New-Item -ItemType Directory -Force -Path $dir) }
    [System.IO.File]::WriteAllBytes($Path, $bytes.ToArray())
    (Get-Item -LiteralPath $Path).LastWriteTimeUtc = [datetime]::SpecifyKind($LastWrite, [System.DateTimeKind]::Utc)
}

function New-BasicPackage {
    param([string]$Tag)
    $repo = Join-Path $runRoot "$Tag repo"
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot "$Tag exe\toss.exe"
    New-FakeExe -Path $exe
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe, '-OutputName', 'pkg')
    Assert-Exit -Got $res.Exit -Want 0 -Text $res.Text -What 'baseline packaging'
    $pkg = Join-Path $repo 'docs\release-work\packages\pkg'
    Assert (Test-Path -LiteralPath $pkg) 'baseline package folder exists'
    return [pscustomobject]@{ Repo = $repo; Exe = $exe; Pkg = $pkg }
}

$results = [System.Collections.Generic.List[object]]::new()

function Invoke-Test {
    param([string]$Name, [scriptblock]$Action)
    try {
        & $Action | Out-Null
        [void]$results.Add([pscustomobject]@{ Name = $Name; Ok = $true; Detail = '' })
        Write-Output "[PASS] $Name"
    }
    catch {
        [void]$results.Add([pscustomobject]@{ Name = $Name; Ok = $false; Detail = $_.Exception.Message })
        Write-Output "[FAIL] $Name : $($_.Exception.Message)"
    }
}

# T01: both scripts parse cleanly.
Invoke-Test 'scripts_parse_cleanly' {
    foreach ($f in @($script:Packager, $script:SelfPath)) {
        $tokens = $null
        $errors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$tokens, [ref]$errors)
        Assert ($errors.Count -eq 0) "$f has $($errors.Count) parse error(s): $(($errors | ForEach-Object Message) -join '; ')"
    }
}

# T02: happy path with unicode/space paths, untracked new input, credential exclusion.
Invoke-Test 'package_success_unicode_and_exclusions' {
    $repo = Join-Path $runRoot '装包 test 😊 repo'
    New-FixtureRepo -Dir $repo
    New-FixtureFile -Base $repo -Rel '.env' -Text "SECRET=must-not-be-packaged`n"
    New-FixtureFile -Base $repo -Rel 'src/extra.rs' -Text "// added after commit, part of real build inputs`n"
    $exe = Join-Path $runRoot '装包 test 😊 exe\toss.exe'
    New-FakeExe -Path $exe
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe, '-OutputName', 'draft 包 😊')
    Assert-Exit -Got $res.Exit -Want 0 -Text $res.Text -What 'unicode packaging'
    $pkg = Join-Path $repo 'docs\release-work\packages\draft 包 😊'
    Assert (Test-Path -LiteralPath $pkg) 'unicode package folder exists'
    $sm = [System.IO.File]::ReadAllText((Join-Path $pkg 'SOURCE_MANIFEST.sha256'))
    Assert ($sm -match '(?m)  src/main\.rs$') 'source manifest lists src/main.rs'
    Assert ($sm -match '(?m)  src/extra\.rs$') 'source manifest lists the untracked new input src/extra.rs'
    Assert ($sm -notmatch '(?m)  docs/private-notes\.md$') 'gitignored docs are excluded'
    Assert ($sm -notmatch '(?m)  target/junk\.txt$') 'gitignored target is excluded'
    Assert ($sm -notmatch '(?m)  \.env$') 'credential denylist excludes .env from the source inventory'
    $prov = Get-Content -LiteralPath (Join-Path $pkg 'PROVENANCE.json') -Raw | ConvertFrom-Json
    Assert (@($prov.source.excluded_credential_files) -contains '.env') 'provenance records the excluded credential file'
    Assert (@($prov.repo.untracked_new_files) -contains 'src/extra.rs') 'provenance records the untracked new input'
    Assert ($prov.claims.legal_clearance -eq $false) 'provenance refuses legal clearance'
    Assert ($prov.repo.dev_state_policy -eq 'strict') 'clean fixture stays under the strict policy'
    $v = Test-ReleasePackageCore -Dir $pkg -CheckLevel Full
    Assert $v.Ok "verifier accepts the fresh package: $($v.Reasons -join ' | ')"
}

# T03: missing LICENSE in the fixture repo is a hard input failure.
Invoke-Test 'packager_missing_license_fails' {
    $repo = Join-Path $runRoot 'nolicense repo'
    New-FixtureRepo -Dir $repo -Variant NoLicense
    $exe = Join-Path $runRoot 'nolicense exe\toss.exe'
    New-FakeExe -Path $exe
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
    Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'missing LICENSE'
    Assert ($res.Text -match 'LICENSE') 'diagnostic names LICENSE'
}

# T04: vendored tree without its upstream license file is a hard failure.
Invoke-Test 'packager_missing_vendored_license_fails' {
    $repo = Join-Path $runRoot 'novendored repo'
    New-FixtureRepo -Dir $repo -Variant NoVendoredLicense
    $exe = Join-Path $runRoot 'novendored exe\toss.exe'
    New-FakeExe -Path $exe
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
    Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'missing vendored license'
    Assert ($res.Text -match 'libarchive') 'diagnostic names the vendored tree'
}

# T05: Cargo.lock crate absent from the local registry cache is a hard failure.
Invoke-Test 'packager_missing_crate_source_fails' {
    $repo = Join-Path $runRoot 'nocrate repo'
    New-FixtureRepo -Dir $repo -Variant MissingCrate
    $exe = Join-Path $runRoot 'nocrate exe\toss.exe'
    New-FakeExe -Path $exe
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
    Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'missing crate source'
    Assert ($res.Text -match 'toss-fixture-missing-crate') 'diagnostic names the missing crate'
}

# T06: an existing output folder is never overwritten or deleted.
Invoke-Test 'packager_refuses_overwrite' {
    $base = New-BasicPackage -Tag 'overwrite'
    $manifestPath = Join-Path $base.Pkg 'MANIFEST.sha256'
    $before = Get-Sha256Hex $manifestPath
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $base.Repo, '-ExecutablePath', $base.Exe, '-OutputName', 'pkg')
    Assert-Exit -Got $res.Exit -Want 5 -Text $res.Text -What 'second packaging into an existing folder'
    Assert ((Get-Sha256Hex $manifestPath) -eq $before) 'existing package manifest is untouched'
}

# T07: dirty tracked state is refused by default and only packaged with -AllowDirty.
Invoke-Test 'packager_dirty_policy' {
    $base = New-BasicPackage -Tag 'dirty'
    New-FixtureFile -Base $base.Repo -Rel 'src/main.rs' -Text "fn main() { /* dirty edit */ }`n"
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $base.Repo, '-ExecutablePath', $base.Exe, '-OutputName', 'dirty-strict')
    Assert-Exit -Got $res.Exit -Want 6 -Text $res.Text -What 'dirty tree under the strict policy'
    New-FakeExe -Path $base.Exe   # refresh mtime so freshness is not the failing check
    $res2 = Invoke-Packager -PackArgs @('-RepoRoot', $base.Repo, '-ExecutablePath', $base.Exe, '-OutputName', 'dirty-allowed', '-AllowDirty')
    Assert-Exit -Got $res2.Exit -Want 0 -Text $res2.Text -What 'dirty tree with -AllowDirty'
    $pkg = Join-Path $base.Repo 'docs\release-work\packages\dirty-allowed'
    $prov = Get-Content -LiteralPath (Join-Path $pkg 'PROVENANCE.json') -Raw | ConvertFrom-Json
    Assert ($prov.repo.git_dirty -eq $true) 'provenance records the dirty tree'
    Assert ($prov.repo.dev_state_policy -eq 'allow-dirty') 'provenance records the allow-dirty policy'
    Assert (@($prov.repo.dirty_tracked_files) -contains 'src/main.rs') 'provenance names the dirty file'
    $v = Test-ReleasePackageCore -Dir $pkg -CheckLevel Quick
    Assert $v.Ok "verifier accepts the honest dirty package: $($v.Reasons -join ' | ')"
}

# T08: output outside the workspace scope is refused without creating anything.
Invoke-Test 'packager_refuses_output_outside_workspace' {
    $repo = Join-Path $runRoot 'scope repo'
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot 'scope exe\toss.exe'
    New-FakeExe -Path $exe
    $outside = Join-Path ([System.IO.Path]::GetTempPath()) ("toss-outside-scope-" + ([System.Guid]::NewGuid().ToString('N')).Substring(0, 8))
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe, '-OutputRoot', $outside)
    Assert-Exit -Got $res.Exit -Want 2 -Text $res.Text -What 'output root outside the workspace'
    Assert (-not (Test-Path -LiteralPath $outside)) 'nothing was created outside the workspace'
}

# T09: a provided executable path that does not exist is an input failure.
Invoke-Test 'packager_missing_provided_exe_fails' {
    $repo = Join-Path $runRoot 'noexe repo'
    New-FixtureRepo -Dir $repo
    $missing = Join-Path $runRoot 'no such exe\toss.exe'
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $missing)
    Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'missing provided executable'
}

# T10: an executable older than the newest inventoried input is stale.
Invoke-Test 'packager_rejects_stale_executable' {
    $repo = Join-Path $runRoot 'stale repo'
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot 'stale exe\toss.exe'
    New-FakeExe -Path $exe -LastWrite ([datetime]::new(2000, 1, 1, 0, 0, 0, [System.DateTimeKind]::Utc))
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
    Assert-Exit -Got $res.Exit -Want 4 -Text $res.Text -What 'stale executable'
    Assert ($res.Text -match 'stale') 'diagnostic says the executable is stale'
}

# T11: a non-PE executable is rejected.
Invoke-Test 'packager_rejects_non_pe_executable' {
    $repo = Join-Path $runRoot 'badpe repo'
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot 'badpe exe\toss.exe'
    $dir = Split-Path -Parent $exe
    [void](New-Item -ItemType Directory -Force -Path $dir)
    [System.IO.File]::WriteAllText($exe, 'hello, not an executable', [System.Text.UTF8Encoding]::new($false))
    $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
    Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'non-PE executable'
    Assert ($res.Text -match 'MZ') 'diagnostic mentions the MZ signature'
}

# T12: verifier rejects a missing package directory (function and CLI wiring).
Invoke-Test 'verify_rejects_missing_package_dir' {
    $missing = Join-Path $runRoot 'no such package'
    $v = Test-ReleasePackageCore -Dir $missing -CheckLevel Quick
    Assert (-not $v.Ok) 'core verifier rejects a missing directory'
    $res = Invoke-Capture -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', $script:SelfPath, '-Mode', 'Verify', '-PackageDir', $missing)
    Assert ($res.Exit -ne 0) "CLI verify must fail for a missing directory, got $($res.Exit)"
}

# T13: verifier accepts a fresh package through the CLI at both levels.
Invoke-Test 'verify_cli_accepts_fresh_package' {
    $base = New-BasicPackage -Tag 'cli-verify'
    $quick = Invoke-Capture -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', $PSCommandPath, '-Mode', 'Verify', '-PackageDir', $base.Pkg, '-Level', 'Quick')
    Assert ($quick.Exit -eq 0) "CLI verify Quick failed: $($quick.Err)"
    $full = Invoke-Capture -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', $PSCommandPath, '-Mode', 'Verify', '-PackageDir', $base.Pkg, '-Level', 'Full')
    Assert ($full.Exit -eq 0) "CLI verify Full failed: $($full.Err)"
}

# T14: verifier detects a tampered executable.
Invoke-Test 'verify_detects_tampered_executable' {
    $base = New-BasicPackage -Tag 'tamper-exe'
    $exePath = Join-Path $base.Pkg 'toss.exe'
    [System.IO.File]::AppendAllText($exePath, 'tampered')
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier rejects a tampered executable'
    Assert (($v.Reasons -join ' ') -match 'toss\.exe') 'reason names the tampered file'
}

# T15: verifier detects a manifest whose hash was rewritten to match tampering.
Invoke-Test 'verify_detects_tampered_manifest_entry' {
    $base = New-BasicPackage -Tag 'tamper-manifest'
    $exePath = Join-Path $base.Pkg 'toss.exe'
    [System.IO.File]::AppendAllText($exePath, 'tampered')
    $manifestPath = Join-Path $base.Pkg 'MANIFEST.sha256'
    $text = [System.IO.File]::ReadAllText($manifestPath)
    $text = $text -replace '(?m)^[0-9a-f]{64}  toss\.exe$', (('f' * 64) + '  toss.exe')
    Write-Utf8NoBom -Path $manifestPath -Text $text
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier rejects a rewritten manifest entry'
    Assert (($v.Reasons -join ' ') -match 'PROVENANCE|executable') 'provenance cross-check catches the rewrite'
}

# T16: verifier detects a malformed manifest.
Invoke-Test 'verify_detects_malformed_manifest' {
    $base = New-BasicPackage -Tag 'malformed'
    Write-Utf8NoBom -Path (Join-Path $base.Pkg 'MANIFEST.sha256') -Text "garbage line without a hash`n"
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier rejects a malformed manifest'
    Assert (($v.Reasons -join ' ') -match 'malformed') 'reason says the manifest is malformed'
}

# T17: verifier detects an extra file that the manifest does not record.
Invoke-Test 'verify_detects_extra_file' {
    $base = New-BasicPackage -Tag 'extra'
    [System.IO.File]::WriteAllText((Join-Path $base.Pkg 'rogue.txt'), 'not in the manifest', [System.Text.UTF8Encoding]::new($false))
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier rejects an extra file'
    Assert (($v.Reasons -join ' ') -match 'rogue\.txt') 'reason names the extra file'
}

# T18: verifier detects a missing packaged file.
Invoke-Test 'verify_detects_missing_packaged_file' {
    $base = New-BasicPackage -Tag 'missing-file'
    Remove-Item -LiteralPath (Join-Path $base.Pkg 'LICENSE')
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier rejects a package with a missing LICENSE'
    Assert (($v.Reasons -join ' ') -match 'LICENSE') 'reason names LICENSE'
}

# T19: verifier detects a source-zip entry that no longer matches SOURCE_MANIFEST.
Invoke-Test 'verify_detects_source_zip_content_tamper' {
    $base = New-BasicPackage -Tag 'src-tamper'
    $zipPath = Join-Path $base.Pkg 'source\toss-0.1.0-source.zip'
    $zip = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Update)
    try {
        $entry = $zip.GetEntry('toss-source/src/lib.rs')
        Assert ($null -ne $entry) 'fixture source zip contains src/lib.rs'
        $entry.Delete()
        $newEntry = $zip.CreateEntry('toss-source/src/lib.rs')
        $stream = $newEntry.Open()
        try {
            $bytes = [System.Text.Encoding]::UTF8.GetBytes("// tampered content`n")
            $stream.Write($bytes, 0, $bytes.Length)
        }
        finally { $stream.Dispose() }
    }
    finally { $zip.Dispose() }
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Full
    Assert (-not $v.Ok) 'verifier detects the tampered source zip entry'
    Assert (($v.Reasons -join ' ') -match 'src/lib\.rs') 'reason names the tampered source file'
}

# T20: verifier detects a crate-sources zip missing a crate.
Invoke-Test 'verify_detects_crate_zip_gap' {
    $base = New-BasicPackage -Tag 'crate-gap'
    $zipPath = Join-Path $base.Pkg 'source\toss-0.1.0-crate-sources.zip'
    $zip = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Update)
    try {
        $toRemove = @($zip.Entries | Where-Object { $_.FullName -like 'crate-sources/unicode-ident-1.0.26/*' })
        Assert ($toRemove.Count -gt 0) 'fixture crate zip contains unicode-ident entries'
        foreach ($e in $toRemove) { $e.Delete() }
    }
    finally {
        $zip.Dispose()
    }
    $v = Test-ReleasePackageCore -Dir $base.Pkg -CheckLevel Quick
    Assert (-not $v.Ok) 'verifier detects the missing crate dir'
    Assert (($v.Reasons -join ' ') -match 'unicode-ident-1\.0\.26') 'reason names the missing crate'
}

# T21: packager refuses an OutputRoot containing a directory junction.
Invoke-Test 'packager_refuses_junction_output' {
    $repo = Join-Path $runRoot 'junction repo'
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot 'junction exe\toss.exe'
    New-FakeExe -Path $exe
    $targetDir = Join-Path $runRoot 'junction-external-target'
    [void](New-Item -ItemType Directory -Force -Path $targetDir)
    New-FixtureFile -Base $targetDir -Rel 'canary.txt' -Text "canary data before pack`n"
    $junctionDir = Join-Path $repo 'docs\release-work\junction-packages'
    [void](New-Item -ItemType Directory -Force -Path (Split-Path -Parent $junctionDir))

    $linkCreated = $false
    try {
        [void][System.IO.Directory]::CreateSymbolicLink($junctionDir, $targetDir)
        $linkCreated = $true
    }
    catch {
        # Fallback to cmd /c mklink /J if CreateSymbolicLink fails or requires privilege
        $cmdRes = Invoke-Capture -FilePath 'cmd.exe' -ArgumentList @('/c', 'mklink', '/J', $junctionDir, $targetDir)
        if ($cmdRes.Exit -eq 0 -and (Test-Path -LiteralPath $junctionDir)) {
            $linkCreated = $true
        }
    }

    if ($linkCreated) {
        $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe, '-OutputRoot', $junctionDir)
        Assert-Exit -Got $res.Exit -Want 2 -Text $res.Text -What 'output root via directory junction'
        Assert ($res.Text -match 'reparse point|junction|symlink') 'diagnostic mentions reparse/junction/symlink'
        Assert (Test-Path -LiteralPath (Join-Path $targetDir 'canary.txt')) 'target canary file remains intact'
        $targetChildren = @(Get-ChildItem -LiteralPath $targetDir | ForEach-Object Name)
        Assert ($targetChildren.Count -eq 1 -and $targetChildren[0] -eq 'canary.txt') 'target directory remains completely untouched'
    }
    else {
        Add-Detail "NOTICE: environment does not permit creating directory junction; skipping junction test"
    }
}

# T22: packager refuses source inventory files that are reparse points / symlinks.
Invoke-Test 'packager_refuses_reparse_source_input' {
    $repo = Join-Path $runRoot 'linksrc repo'
    New-FixtureRepo -Dir $repo
    $exe = Join-Path $runRoot 'linksrc exe\toss.exe'
    New-FakeExe -Path $exe
    $outside = Join-Path $runRoot 'linksrc-external-file.rs'
    [System.IO.File]::WriteAllText($outside, "// external source file`n", [System.Text.UTF8Encoding]::new($false))
    $symlinkSrc = Join-Path $repo 'src\symlink.rs'

    $linkCreated = $false
    try {
        [void][System.IO.File]::CreateSymbolicLink($symlinkSrc, $outside)
        $linkCreated = $true
    }
    catch { }

    if ($linkCreated) {
        $res = Invoke-Packager -PackArgs @('-RepoRoot', $repo, '-ExecutablePath', $exe)
        Assert-Exit -Got $res.Exit -Want 3 -Text $res.Text -What 'source file as symlink'
        Assert ($res.Text -match 'reparse point|symlink') 'diagnostic mentions reparse/symlink'
    }
    else {
        Add-Detail "NOTICE: unprivileged environment does not permit file symlink creation; skipped file symlink test"
    }
}

# --- summary ---------------------------------------------------------------

$passed = @($results | Where-Object Ok).Count
$failed = @($results | Where-Object { -not $_.Ok }).Count
Write-Output '---'
Write-Output "selftest_run_root=$runRoot"
Write-Output "passed=$passed failed=$failed total=$($results.Count)"
foreach ($r in $results) {
    if ($r.Ok) { Write-Output ("  PASS  {0}" -f $r.Name) }
    else { Write-Output ("  FAIL  {0} :: {1}" -f $r.Name, $r.Detail) }
}

$summaryText = @"
selftest_utc=$runStamp
run_root=$runRoot
detail_log=$detailLog
passed=$passed
failed=$failed
total=$($results.Count)
$((($results | ForEach-Object { '{0}={1}' -f $_.Name, $(if ($_.Ok) { 'PASS' } else { "FAIL: $($_.Detail)" }) }) -join "`n"))
"@
Write-Utf8NoBom -Path $summaryLog -Text ($summaryText + "`n")
Write-Utf8NoBom -Path $detailLog -Text (($script:DetailLines -join "`n") + "`n")
Write-Output "summary_log=$summaryLog"

if ($failed -gt 0) { exit 1 }
exit 0
