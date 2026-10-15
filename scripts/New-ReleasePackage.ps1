#Requires -Version 7.0
<#
.SYNOPSIS
  Create a LOCAL DRAFT release package for Toss. Never publishes anything.

.DESCRIPTION
  Dependency-light PowerShell 7 developer tooling. Network is never used:
  the build runs with `cargo build --release --locked --offline --all-features`,
  crate sources are read from the local cargo registry cache, and every other
  input comes from this git repository's working tree.

  A package always lands in a brand-new folder. Existing folders are never
  overwritten or deleted, and all writes stay inside the workspace output
  scope (a path under the repository root; default
  <repo>/docs/release-work/packages).

  Package contents:
    toss.exe                     executable actually built (or explicitly
                                 provided in fixture mode; provenance says which)
    LICENSE                      GPL-3.0-or-later text from the repository
    THIRD_PARTY_NOTICES.md       tracked notices inventory
    licenses/third_party/...     required upstream license files of vendored trees
    licenses/crates/...          license files of every Cargo.lock crate, copied
                                 from the local registry cache
    source/toss-0.1.0-source.zip every inventoried source file (git tracked +
                                 untracked non-ignored, credential denylist apart)
    source/toss-0.1.0-crate-sources.zip
                                 exact Cargo.lock crate sources from the local
                                 registry cache
    SOURCE_MANIFEST.sha256       per-file SHA-256 of the source inventory
    PROVENANCE.json              build/version/size/dev-state record with an
                                 explicit non-claims block
    LOCAL-DRAFT.md               generated draft banner and usage notes
    MANIFEST.sha256              SHA-256 of every packaged file except itself

  Claims this tool never makes: legal clearance, reproducibility or
  byte-identical rebuild, publication. See RELEASING.md.

.OUTPUTS
  Exit codes:
    0  success (a new local-draft package folder was created)
    1  internal/unexpected failure
    2  invalid argument or output path outside the workspace scope
    3  missing or invalid input (LICENSE, notices, source file, crate cache,
       executable, git state)
    4  build failed or the executable is stale against the source inventory
    5  the requested output folder already exists (nothing was touched)
    6  dirty tracked development state refused by the strict policy
#>
[CmdletBinding()]
param(
    # Repository root. Defaults to the parent of this scripts/ folder.
    [string]$RepoRoot = '',

    # Output scope root; must stay inside the repository. Default:
    # <repo>/docs/release-work/packages
    [string]$OutputRoot = '',

    # Optional explicit package folder name (unicode and spaces allowed).
    # When omitted a unique timestamped name is generated.
    [string]$OutputName = '',

    # Permit packaging while tracked files differ from HEAD. The dirty state
    # is then recorded truthfully in PROVENANCE.json; default is to refuse.
    [switch]$AllowDirty,

    # Testing/fixture only: skip the cargo build and use this executable.
    # PROVENANCE.json records mode="provided-executable"; no build is claimed.
    [string]$ExecutablePath = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$script:SchemaId = 'toss-local-draft-package/1'
$script:ToolName = 'scripts/New-ReleasePackage.ps1'
$script:ToolVersion = '0.1.0'

$script:EXIT_OK = 0
$script:EXIT_INTERNAL = 1
$script:EXIT_ARG = 2
$script:EXIT_INPUT = 3
$script:EXIT_BUILD = 4
$script:EXIT_OUTPUT = 5
$script:EXIT_DIRTY = 6

# Untracked files matching these repo-relative paths are treated as local
# credentials: they are excluded from the source inventory, recorded in
# PROVENANCE.json as excluded, and never packaged. A *tracked* match is a
# hard input failure (the owner must decide, this tool will not guess).
$script:CredentialDenyPatterns = @(
    '(?i)(^|/)\.env($|\.)'
    '(?i)(^|/)\.netrc$'
    '(?i)(^|/)id_rsa($|\.pub$)'
    '(?i)(^|/)id_ed25519($|\.pub$)'
    '(?i)(^|/)id_ecdsa($|\.pub$)'
    '(?i)\.(pem|pfx|p12)$'
    '(?i)(^|/)credentials(\.(json|toml|ya?ml|txt))?$'
    '(?i)(^|/)secrets\.(json|toml|ya?ml|txt)$'
)

# Vendored trees that must ship their upstream license file. First existing
# candidate is required; absence of every candidate is exit 3.
$script:VendoredLicenseCandidates = [ordered]@{
    'libarchive' = @('COPYING')
    'zlib'       = @('LICENSE', 'COPYING')
    'xz'         = @('COPYING.0BSD', 'COPYING')
    'ffmpeg'     = @('COPYING.LGPLv2.1', 'COPYING', 'LICENSE.md')
}

function Write-Err([string]$Message) {
    [Console]::Error.WriteLine("Error: $Message")
}

function Fail {
    param([int]$Code, [string]$Message)
    Write-Err $Message
    exit $Code
}

function Get-Sha256Hex([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $fs = [System.IO.File]::OpenRead($Path)
        try {
            $hash = $sha.ComputeHash($fs)
        }
        finally {
            $fs.Dispose()
        }
    }
    finally {
        $sha.Dispose()
    }
    return ([System.BitConverter]::ToString($hash) -replace '-', '').ToLowerInvariant()
}

function Get-Sha256BytesHex([byte[]]$Bytes) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash($Bytes)
    }
    finally {
        $sha.Dispose()
    }
    return ([System.BitConverter]::ToString($hash) -replace '-', '').ToLowerInvariant()
}

function Write-Utf8NoBom([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Test-ReparsePath([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    try {
        $item = [System.IO.FileInfo]::new($Path)
        if (-not $item.Exists) {
            $item = [System.IO.DirectoryInfo]::new($Path)
        }
        if ($item.Exists -and ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            return $true
        }
    }
    catch { }
    return $false
}

function Test-ReparseHierarchy([string]$Path, [string]$StopAt) {
    $curr = $Path
    try { $curr = [System.IO.Path]::GetFullPath($curr) } catch { return $false }
    $stop = $StopAt
    if ($stop) { try { $stop = [System.IO.Path]::GetFullPath($stop) } catch { } }

    while ($curr) {
        if (Test-ReparsePath $curr) { return $true }
        if ($stop -and $curr.TrimEnd('\').Equals($stop.TrimEnd('\'), [System.StringComparison]::OrdinalIgnoreCase)) {
            break
        }
        $parent = [System.IO.Path]::GetDirectoryName($curr)
        if ($parent -eq $curr -or [string]::IsNullOrEmpty($parent)) { break }
        $curr = $parent
    }
    return $false
}

# Run an external program, capturing stdout/stderr as UTF-8 text without
# letting stderr or exit codes interact with $ErrorActionPreference.
function Invoke-Capture {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = ''
    )
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

function Invoke-Git {
    param([string]$Root, [string[]]$GitArgs)
    $res = Invoke-Capture -FilePath 'git' -ArgumentList (@('--no-optional-locks', '-C', $Root) + $GitArgs)
    if ($res.Exit -ne 0) {
        Fail -Code $script:EXIT_INPUT -Message ("git {0} failed (exit {1}): {2}" -f ($GitArgs -join ' '), $res.Exit, $res.Err.Trim())
    }
    return $res
}

function Get-NulSplit([string]$Text) {
    if ([string]::IsNullOrEmpty($Text)) { return @() }
    return @($Text.TrimEnd("`0") -split "`0" | Where-Object { $_ -ne '' })
}

function Test-CredentialPath([string]$RelPath) {
    foreach ($p in $script:CredentialDenyPatterns) {
        if ($RelPath -match $p) { return $true }
    }
    return $false
}

function New-ZipFromEntries {
    param(
        [Parameter(Mandatory)][string]$ZipPath,
        [Parameter(Mandatory)][object[]]$Entries  # each: @{ Arc = 'forward/slash/path'; Src = 'absolute source path' }
    )
    $zipEpoch = [System.DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero)
    $fs = [System.IO.File]::Create($ZipPath)
    try {
        $zip = [System.IO.Compression.ZipArchive]::new($fs, [System.IO.Compression.ZipArchiveMode]::Create, $false)
        try {
            foreach ($e in $Entries) {
                $entry = $zip.CreateEntry($e.Arc, [System.IO.Compression.CompressionLevel]::Fastest)
                $mtime = [System.DateTime]::SpecifyKind([System.IO.File]::GetLastWriteTimeUtc($e.Src), [System.DateTimeKind]::Utc)
                $dto = [System.DateTimeOffset]::new($mtime, [System.TimeSpan]::Zero)
                if ($dto -lt $zipEpoch) { $dto = $zipEpoch }
                $entry.LastWriteTime = $dto
                $in = [System.IO.File]::OpenRead($e.Src)
                try {
                    $entryStream = $entry.Open()
                    try { $in.CopyTo($entryStream) }
                    finally { $entryStream.Dispose() }
                }
                finally {
                    $in.Dispose()
                }
            }
        }
        finally {
            $zip.Dispose()
        }
    }
    finally {
        $fs.Dispose()
    }
}

# ---------------------------------------------------------------------------

if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}
try {
    $RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).ProviderPath
}
catch {
    Fail -Code $script:EXIT_ARG -Message "repository root not found: $RepoRoot"
}

# The repository must be usable as a source authority.
if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot '.git'))) {
    Fail -Code $script:EXIT_ARG -Message "not a git repository root (no .git): $RepoRoot"
}
$topRes = Invoke-Git -Root $RepoRoot -GitArgs @('rev-parse', '--show-toplevel')
$top = ($topRes.Out.Trim() -replace '[/]', '\')
try { $top = [System.IO.Path]::GetFullPath($top) } catch { }
if (-not $top.TrimEnd('\').Equals($RepoRoot.TrimEnd('\'), [System.StringComparison]::OrdinalIgnoreCase)) {
    Fail -Code $script:EXIT_ARG -Message "RepoRoot must be the git toplevel; git says '$top', given '$RepoRoot'"
}

# Output scope: always inside the workspace, always a new folder.
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $RepoRoot 'docs\release-work\packages'
}
try {
    $OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
}
catch {
    Fail -Code $script:EXIT_ARG -Message "invalid -OutputRoot: $OutputRoot"
}
$rootPrefix = $RepoRoot.TrimEnd('\') + '\'
if (-not $OutputRoot.TrimEnd('\').StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    Fail -Code $script:EXIT_ARG -Message "output scope rejected: -OutputRoot must stay inside the repository ($RepoRoot); got '$OutputRoot'"
}
if ($OutputRoot.TrimEnd('\') -ieq $RepoRoot) {
    Fail -Code $script:EXIT_ARG -Message "output scope rejected: -OutputRoot may not be the repository root itself"
}
if (Test-ReparseHierarchy -Path $OutputRoot -StopAt $RepoRoot) {
    Fail -Code $script:EXIT_ARG -Message "output scope rejected: -OutputRoot contains a reparse point/junction/symlink component: '$OutputRoot'"
}

if ($OutputName) {
    if ($OutputName -match '[<>:"/\\|?*]' -or $OutputName -match '[\x00-\x1F]') {
        Fail -Code $script:EXIT_ARG -Message "invalid -OutputName (path separators or reserved characters): '$OutputName'"
    }
    if ($OutputName -in @('.', '..') -or $OutputName -match '\.\.$' -or $OutputName -match '[ .]$') {
        Fail -Code $script:EXIT_ARG -Message "invalid -OutputName: '$OutputName'"
    }
    $packageName = $OutputName
}
else {
    $stamp = [System.DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
    $rand = -join ((1..4) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
    $packageName = "toss-local-draft-$stamp-$rand"
}
$targetDir = Join-Path $OutputRoot $packageName
if (Test-Path -LiteralPath $targetDir) {
    Fail -Code $script:EXIT_OUTPUT -Message "output folder already exists, refusing to overwrite or delete: $targetDir"
}

foreach ($required in @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'Cargo.lock', 'Cargo.toml')) {
    if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $required))) {
        Fail -Code $script:EXIT_INPUT -Message "required repository input missing: $required"
    }
}

# --- git identity and development-state policy -----------------------------

$headRes = Invoke-Git -Root $RepoRoot -GitArgs @('rev-parse', 'HEAD')
$headCommit = $headRes.Out.Trim()
$branchRes = Invoke-Git -Root $RepoRoot -GitArgs @('rev-parse', '--abbrev-ref', 'HEAD')
$branch = $branchRes.Out.Trim()

$unstaged = @(Get-NulSplit (Invoke-Git -Root $RepoRoot -GitArgs @('diff', '--name-only', '-z')).Out)
$staged = @(Get-NulSplit (Invoke-Git -Root $RepoRoot -GitArgs @('diff', '--cached', '--name-only', '-z')).Out)
$deleted = @(Get-NulSplit (Invoke-Git -Root $RepoRoot -GitArgs @('ls-files', '--deleted', '-z')).Out)
$trackedDirty = @($unstaged + $staged + $deleted | Where-Object { $_ -ne '' } | Sort-Object -Unique)
$untracked = @(Get-NulSplit (Invoke-Git -Root $RepoRoot -GitArgs @('ls-files', '--others', '--exclude-standard', '-z')).Out)

$gitDirty = ($trackedDirty.Count -gt 0)
$devPolicy = 'strict'
if ($gitDirty) {
    if (-not $AllowDirty) {
        $shown = ($trackedDirty | Select-Object -First 10) -join ', '
        Fail -Code $script:EXIT_DIRTY -Message ("tracked development state is dirty ({0} file(s)): {1}. Commit or stash first, or pass -AllowDirty to package the working tree with the dirty state recorded in PROVENANCE.json." -f $trackedDirty.Count, $shown)
    }
    $devPolicy = 'allow-dirty'
}

# --- source inventory ------------------------------------------------------

$tracked = @(Get-NulSplit (Invoke-Git -Root $RepoRoot -GitArgs @('ls-files', '-z')).Out)
$allPaths = [System.Collections.Generic.List[string]]::new()
$seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($p in ($tracked + $untracked)) { if ($seen.Add($p)) { [void]$allPaths.Add($p) } }
$pathArray = $allPaths.ToArray()
[System.Array]::Sort($pathArray, [System.StringComparer]::Ordinal)

$excludedCredentials = [System.Collections.Generic.List[string]]::new()
$inventory = [System.Collections.Generic.List[string]]::new()
foreach ($rel in $pathArray) {
    if (Test-CredentialPath $rel) {
        $isTracked = $seen.Contains($rel) -and ($tracked -contains $rel)
        if ($isTracked) {
            Fail -Code $script:EXIT_INPUT -Message "tracked file matches the credential denylist and cannot be packaged silently: $rel"
        }
        [void]$excludedCredentials.Add($rel)
        continue
    }
    $fullSrc = Join-Path $RepoRoot $rel
    if (-not (Test-Path -LiteralPath $fullSrc -PathType Leaf)) {
        Fail -Code $script:EXIT_INPUT -Message "listed by git but missing from the working tree: $rel"
    }
    if (Test-ReparseHierarchy -Path $fullSrc -StopAt $RepoRoot) {
        Fail -Code $script:EXIT_INPUT -Message "source file or component is a reparse point/symlink: $rel"
    }
    [void]$inventory.Add($rel)
}
$inventoryArray = $inventory.ToArray()
[System.Array]::Sort($inventoryArray, [System.StringComparer]::Ordinal)

$manifestLines = [System.Text.StringBuilder]::new()
$sourceHashes = [System.Collections.Generic.Dictionary[string, string]]::new([System.StringComparer]::Ordinal)
# Freshness is measured against real cargo build inputs only: code,
# manifests, vendored native trees, cargo config. A doc edit (README-style
# *.md, logs) does not invalidate the executable; a source edit does.
$buildInputPattern = '^(src|tests|benches|examples|third_party|\.cargo)/|^(build\.rs|Cargo\.toml|Cargo\.lock|rust-toolchain|rust-toolchain\.toml)$'
$newestInputUtc = [System.DateTime]::MinValue
$newestInputPath = ''
$newestBuildInputUtc = [System.DateTime]::MinValue
$newestBuildInputPath = ''
foreach ($rel in $inventoryArray) {
    $full = Join-Path $RepoRoot $rel
    $hash = Get-Sha256Hex $full
    $sourceHashes[$rel] = $hash
    [void]$manifestLines.Append($hash).Append('  ').Append($rel.Replace('\', '/')).Append("`n")
    $mtime = [System.IO.File]::GetLastWriteTimeUtc($full)
    if ($mtime -gt $newestInputUtc) { $newestInputUtc = $mtime; $newestInputPath = $rel }
    if ($rel -match $buildInputPattern -and $mtime -gt $newestBuildInputUtc) {
        $newestBuildInputUtc = $mtime
        $newestBuildInputPath = $rel
    }
}
$sourceManifestText = $manifestLines.ToString()
$sourceManifestBytes = [System.Text.Encoding]::UTF8.GetBytes($sourceManifestText)
$sourceContentHash = Get-Sha256BytesHex $sourceManifestBytes

if ($inventoryArray.Count -eq 0) {
    Fail -Code $script:EXIT_INPUT -Message "source inventory is empty; refusing to package nothing"
}

# --- required license / notice inputs --------------------------------------

$licensePath = Join-Path $RepoRoot 'LICENSE'
$licenseText = [System.IO.File]::ReadAllText($licensePath)
if ($licenseText -notmatch 'GNU GENERAL PUBLIC LICENSE' -or $licenseText -notmatch 'Version 3') {
    Fail -Code $script:EXIT_INPUT -Message "LICENSE is not the GPL-3.0 text (missing 'GNU GENERAL PUBLIC LICENSE' / 'Version 3')"
}
$noticesPath = Join-Path $RepoRoot 'THIRD_PARTY_NOTICES.md'
$noticesText = [System.IO.File]::ReadAllText($noticesPath)
if ($noticesText -notmatch 'GPL-3\.0-or-later') {
    Fail -Code $script:EXIT_INPUT -Message "THIRD_PARTY_NOTICES.md does not declare GPL-3.0-or-later"
}

$vendoredLicenseFiles = [System.Collections.Generic.List[string]]::new()
$vendoredLibs = @($inventoryArray | Where-Object { $_ -like 'third_party/*/*' } | ForEach-Object { ($_ -split '/')[1] } | Sort-Object -Unique)
foreach ($lib in $vendoredLibs) {
    $rootFiles = @($inventoryArray | Where-Object { $_ -like "third_party/$lib/*" -and $_ -notlike "third_party/$lib/*/*" })
    $candidates = $script:VendoredLicenseCandidates[$lib]
    if ($null -eq $candidates) {
        $candidates = @($rootFiles | Where-Object { $_ -match '(?i)/(COPYING|LICENSE)' } | ForEach-Object { ($_ -split '/')[-1] })
        if ($candidates.Count -eq 0) {
            Fail -Code $script:EXIT_INPUT -Message "vendored tree third_party/$lib has no recognizable license file (COPYING*/LICENSE*) in its root; add the upstream license text before packaging"
        }
    }
    $hit = @($candidates | Where-Object { $rootFiles -contains "third_party/$lib/$_" } | Select-Object -First 1)
    if ($hit.Count -eq 0) {
        Fail -Code $script:EXIT_INPUT -Message "vendored tree third_party/$lib is missing its required upstream license file ($($candidates -join ' / ')); source/notices completeness cannot be claimed"
    }
    [void]$vendoredLicenseFiles.Add("third_party/$lib/$($hit[0])")
}

# --- Cargo.lock crate sources from the local registry cache ----------------

$lockText = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'Cargo.lock'))
$registryPackages = [System.Collections.Generic.List[object]]::new()
$pathPackages = [System.Collections.Generic.List[string]]::new()
foreach ($block in ($lockText -split '\[\[package\]\]')) {
    $nm = [regex]::Match($block, '(?m)^name = "([^"]+)"')
    $vm = [regex]::Match($block, '(?m)^version = "([^"]+)"')
    $sm = [regex]::Match($block, '(?m)^source = "([^"]+)"')
    if (-not $nm.Success -or -not $vm.Success) { continue }
    $name = $nm.Groups[1].Value
    $version = $vm.Groups[1].Value
    if ($name -eq 'toss') { continue }
    if ($sm.Success) {
        if ($sm.Groups[1].Value -like 'registry+*') {
            [void]$registryPackages.Add([pscustomobject]@{ Name = $name; Version = $version })
        }
        else {
            Fail -Code $script:EXIT_INPUT -Message "Cargo.lock package $name@$version has a non-registry source ($($sm.Groups[1].Value)); vendored/path dependency sources must be inventoried by hand, this tool does not guess"
        }
    }
    else {
        [void]$pathPackages.Add("$name@$version")
    }
}

$regSrcRoot = Join-Path $HOME '.cargo\registry\src'
$regRoots = @()
if (Test-Path -LiteralPath $regSrcRoot) {
    $regRoots = @(Get-ChildItem -LiteralPath $regSrcRoot -Directory | ForEach-Object { $_.FullName })
}
$crateRecords = [System.Collections.Generic.List[object]]::new()
$missingCrates = [System.Collections.Generic.List[string]]::new()
foreach ($pkg in $registryPackages) {
    $dir = $null
    foreach ($r in $regRoots) {
        $cand = Join-Path $r ("{0}-{1}" -f $pkg.Name, $pkg.Version)
        if (Test-Path -LiteralPath $cand -PathType Container) { $dir = $cand; break }
    }
    if (-not $dir) {
        [void]$missingCrates.Add("$($pkg.Name)@$($pkg.Version)")
        continue
    }
    $licFiles = @(Get-ChildItem -LiteralPath $dir -File | Where-Object { $_.Name -match '^(?i)(LICENSE|COPYING)' } | ForEach-Object { $_.Name } | Sort-Object)
    if ($licFiles.Count -eq 0) {
        Fail -Code $script:EXIT_INPUT -Message "crate $($pkg.Name)@$($pkg.Version) in the local registry cache carries no LICENSE/COPYING file; notices cannot be claimed complete"
    }
    [void]$crateRecords.Add([pscustomobject]@{
            Name          = $pkg.Name
            Version       = $pkg.Version
            Dir           = ("{0}-{1}" -f $pkg.Name, $pkg.Version)
            RegistryPath  = $dir
            LicenseFiles  = $licFiles
        })
}
if ($missingCrates.Count -gt 0) {
    Fail -Code $script:EXIT_INPUT -Message ("Cargo.lock crate sources missing from the local registry cache ({0}): {1}. Corresponding source cannot be claimed; populate the cache offline or adjust Cargo.lock." -f $missingCrates.Count, (($missingCrates | Select-Object -First 20) -join ', '))
}

# --- executable: real locked offline build or explicit provided input ------

$buildRecord = [ordered]@{
    mode           = 'cargo'
    command        = 'cargo build --release --locked --offline --all-features'
    exit_code      = -1
    started_utc    = ''
    ended_utc      = ''
    duration_s     = 0.0
    cargo_version  = ''
    rustc_version  = ''
    output_tail    = ''
}
$buildMode = 'cargo'
$exeSource = ''

if ($ExecutablePath) {
    $buildMode = 'provided-executable'
    $exeSource = $ExecutablePath
    if (-not (Test-Path -LiteralPath $ExecutablePath -PathType Leaf)) {
        Fail -Code $script:EXIT_INPUT -Message "provided executable not found: $ExecutablePath"
    }
    $buildRecord.mode = 'provided-executable'
    $buildRecord.command = '(cargo build skipped: -ExecutablePath given; fixture/testing input)'
    $buildRecord.exit_code = 0
}
else {
    $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargoCmd) {
        Fail -Code $script:EXIT_BUILD -Message "cargo not found on PATH; cannot perform the locked offline build"
    }
    $ver = Invoke-Capture -FilePath $cargoCmd.Source -ArgumentList @('--version')
    $buildRecord.cargo_version = $ver.Out.Trim()
    $rv = Invoke-Capture -FilePath 'rustc' -ArgumentList @('--version')
    $buildRecord.rustc_version = $rv.Out.Trim()

    $started = [System.DateTime]::UtcNow
    $buildRecord.started_utc = $started.ToString('o')
    Write-Output "building: $($buildRecord.command)"
    $res = Invoke-Capture -FilePath $cargoCmd.Source -ArgumentList @('build', '--release', '--locked', '--offline', '--all-features') -WorkingDirectory $RepoRoot
    $ended = [System.DateTime]::UtcNow
    $buildRecord.ended_utc = $ended.ToString('o')
    $buildRecord.duration_s = [math]::Round(($ended - $started).TotalSeconds, 3)
    $buildRecord.exit_code = $res.Exit
    foreach ($line in ($res.Out -split "`r?`n")) { if ($line -ne '') { Write-Output $line } }
    if ($res.Exit -ne 0) {
        foreach ($line in ($res.Err -split "`r?`n")) { if ($line -ne '') { [Console]::Error.WriteLine($line) } }
        Fail -Code $script:EXIT_BUILD -Message "cargo build failed with exit $($res.Exit); no package was created"
    }
    $combined = (@($res.Out -split "`r?`n") + @($res.Err -split "`r?`n") | Where-Object { $_ -ne '' })
    $buildRecord.output_tail = (($combined | Select-Object -Last 40) -join "`n")

    $exeName = if ($IsWindows) { 'toss.exe' } else { 'toss' }
    $exeSource = Join-Path $RepoRoot "target\release\$exeName"
    if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
        Fail -Code $script:EXIT_BUILD -Message "cargo reported success but the release executable is missing: $exeSource"
    }
}

$exeItem = Get-Item -LiteralPath $exeSource
if ($exeItem.Length -lt 4) {
    Fail -Code $script:EXIT_INPUT -Message "executable is empty or truncated: $exeSource"
}
if ($IsWindows) {
    $head4 = [byte[]]::new(4)
    $fs = [System.IO.File]::OpenRead($exeSource)
    try { [void]$fs.Read($head4, 0, 4) }
    finally { $fs.Dispose() }
    if (-not ($head4[0] -eq 0x4D -and $head4[1] -eq 0x5A)) {
        Fail -Code $script:EXIT_INPUT -Message "executable does not start with the MZ signature, refusing to package it: $exeSource"
    }
}

# Freshness: the executable must not be older than the newest inventoried
# build input. A changed input therefore cannot silently ship a stale binary.
$exeUtc = $exeItem.LastWriteTimeUtc
$freshness = [ordered]@{
    check                     = 'exe_mtime_gte_newest_cargo_build_input'
    newest_build_input_path   = $newestBuildInputPath
    newest_build_input_utc    = if ($newestBuildInputPath) { $newestBuildInputUtc.ToString('o') } else { '' }
    newest_inventoried_input_path = $newestInputPath
    newest_inventoried_input_utc  = $newestInputUtc.ToString('o')
    exe_utc                   = $exeUtc.ToString('o')
    passed                    = $true
}
if ($newestBuildInputPath -and $exeUtc -lt $newestBuildInputUtc) {
    $freshness.passed = $false
    Fail -Code $script:EXIT_BUILD -Message ("stale executable: {0} (mtime {1}) is older than build input {2} (mtime {3}). Rebuild or re-provide a current executable." -f $exeSource, $exeUtc.ToString('o'), $newestBuildInputPath, $newestBuildInputUtc.ToString('o'))
}

# --- stage the package -----------------------------------------------------

[void](New-Item -ItemType Directory -Force -Path $OutputRoot)
$partialDir = "$targetDir.partial-$(([System.Guid]::NewGuid().ToString('N')).Substring(0, 8))"
if (Test-Path -LiteralPath $partialDir) {
    Fail -Code $script:EXIT_OUTPUT -Message "temporary output folder unexpectedly exists: $partialDir"
}
[void](New-Item -ItemType Directory -Path $partialDir)

try {
    $pkgFiles = [ordered]@{}
    $exeTargetName = if ($IsWindows) { 'toss.exe' } else { 'toss' }
    Copy-Item -LiteralPath $exeSource -Destination (Join-Path $partialDir $exeTargetName)
    $pkgFiles['executable'] = $exeTargetName
    Copy-Item -LiteralPath $licensePath -Destination (Join-Path $partialDir 'LICENSE')
    $pkgFiles['license'] = 'LICENSE'
    Copy-Item -LiteralPath $noticesPath -Destination (Join-Path $partialDir 'THIRD_PARTY_NOTICES.md')
    $pkgFiles['notices'] = 'THIRD_PARTY_NOTICES.md'

    foreach ($rel in $vendoredLicenseFiles) {
        $dest = Join-Path $partialDir ("licenses\{0}" -f ($rel -replace '/', '\'))
        [void](New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest))
        Copy-Item -LiteralPath (Join-Path $RepoRoot $rel) -Destination $dest
    }
    foreach ($cr in $crateRecords) {
        if (Test-ReparseHierarchy -Path $cr.RegistryPath) {
            Fail -Code $script:EXIT_INPUT -Message "crate directory is or contains a reparse point/symlink: $($cr.Dir)"
        }
        foreach ($lic in $cr.LicenseFiles) {
            $rel = "licenses/crates/$($cr.Dir)/$lic"
            $licSrc = Join-Path $cr.RegistryPath $lic
            if (Test-ReparsePath $licSrc) {
                Fail -Code $script:EXIT_INPUT -Message "crate license file is a reparse point/symlink: $rel"
            }
            $dest = Join-Path $partialDir ("licenses\crates\{0}\{1}" -f $cr.Dir, $lic)
            [void](New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest))
            Copy-Item -LiteralPath $licSrc -Destination $dest
        }
    }

    $srcZipName = 'source/toss-0.1.0-source.zip'
    $srcZipPath = Join-Path $partialDir ($srcZipName -replace '/', '\')
    [void](New-Item -ItemType Directory -Force -Path (Split-Path -Parent $srcZipPath))
    $srcEntries = foreach ($rel in $inventoryArray) {
        @{ Arc = "toss-source/$($rel.Replace('\', '/'))"; Src = (Join-Path $RepoRoot $rel) }
    }
    Write-Output "source inventory: $($inventoryArray.Count) files; writing source zip..."
    New-ZipFromEntries -ZipPath $srcZipPath -Entries $srcEntries

    $crateZipName = 'source/toss-0.1.0-crate-sources.zip'
    $crateZipPath = Join-Path $partialDir ($crateZipName -replace '/', '\')
    $crateEntries = [System.Collections.Generic.List[object]]::new()
    foreach ($cr in $crateRecords) {
        $base = $cr.RegistryPath
        $files = Get-ChildItem -LiteralPath $base -File -Recurse | Where-Object {
            $relPart = $_.FullName.Substring($base.Length).TrimStart('\')
            $relPart -notmatch '(?i)^(target|\.git)(\\|$)'
        }
        foreach ($f in $files) {
            if (Test-ReparseHierarchy -Path $f.FullName -StopAt $base) {
                Fail -Code $script:EXIT_INPUT -Message "crate file or component is a reparse point/symlink: $($f.FullName)"
            }
            $relPart = $f.FullName.Substring($base.Length).TrimStart('\').Replace('\', '/')
            [void]$crateEntries.Add(@{ Arc = "crate-sources/$($cr.Dir)/$relPart"; Src = $f.FullName })
        }
    }
    Write-Output "crate sources: $($crateRecords.Count) crates; writing crate zip..."
    New-ZipFromEntries -ZipPath $crateZipPath -Entries $crateEntries.ToArray()

    [System.IO.File]::WriteAllBytes((Join-Path $partialDir 'SOURCE_MANIFEST.sha256'), $sourceManifestBytes)

    $artifactList = [System.Collections.Generic.List[object]]::new()
    function Add-Artifact([string]$RelPath) {
        $full = Join-Path $partialDir ($RelPath -replace '/', '\')
        $script:artifactList.Add([ordered]@{
                path       = $RelPath
                size_bytes = (Get-Item -LiteralPath $full).Length
                sha256     = Get-Sha256Hex $full
            })
    }

    $localDraftText = @"
# LOCAL DRAFT PACKAGE - NOT PUBLISHED, NOT LEGALLY CLEARED

Generated by $script:ToolName v$script:ToolVersion at $([System.DateTime]::UtcNow.ToString('o')).

This folder is a local review draft assembled on this machine. It has not
been uploaded, published or conveyed anywhere. Verify it with:

    pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode Verify -PackageDir <this folder> -Level Full

## Non-claims (read before quoting anything from this folder)

* No legal clearance is claimed. The static-CRT questions **R1** (GPL-3.0
  System Library applicability) and **M1** (Microsoft Distributable Code
  authorization) have a documented project interpretation; see RELEASING.md.
  This tool does not attest the current builder's license or replace the
  applicable agreement. Reassess when the toolchain or license basis changes.
* No reproducibility or byte-identical rebuild is claimed.
* TESTING.md checkboxes are neither required nor asserted by this package.
* This package is not a release artifact; publishing is a separate,
  owner-decided step.

## Contents

* ``$exeTargetName`` - the executable this run actually built (or an
  explicitly provided test executable; PROVENANCE.json says which)
* ``LICENSE`` - GPL-3.0-or-later
* ``THIRD_PARTY_NOTICES.md`` + ``licenses/`` - notices and upstream texts
* ``source/`` - full source tree and the exact Cargo.lock crate sources
* ``SOURCE_MANIFEST.sha256`` / ``MANIFEST.sha256`` / ``PROVENANCE.json``
"@
    Write-Utf8NoBom -Path (Join-Path $partialDir 'LOCAL-DRAFT.md') -Text $localDraftText

    foreach ($rel in @($exeTargetName, 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'SOURCE_MANIFEST.sha256', $srcZipName, $crateZipName, 'LOCAL-DRAFT.md')) {
        Add-Artifact $rel
    }

    $provenance = [ordered]@{
        schema        = $script:SchemaId
        tool          = [ordered]@{ name = $script:ToolName; version = $script:ToolVersion; powershell = $PSVersionTable.PSVersion.ToString() }
        package_name  = $packageName
        created_utc   = [System.DateTime]::UtcNow.ToString('o')
        repo          = [ordered]@{
            root                 = $RepoRoot
            head_commit          = $headCommit
            branch               = $branch
            git_dirty            = $gitDirty
            dirty_tracked_files  = $trackedDirty
            untracked_new_files  = @($untracked | Where-Object { -not (Test-CredentialPath $_) })
            untracked_new_count  = $untracked.Count
            dev_state_policy     = $devPolicy
            inventory_basis      = 'git ls-files (tracked) + git ls-files --others --exclude-standard, minus the credential denylist'
        }
        build         = $buildRecord
        executable    = [ordered]@{
            path_in_package = $exeTargetName
            source_path     = $exeSource
            size_bytes      = $exeItem.Length
            sha256          = Get-Sha256Hex (Join-Path $partialDir $exeTargetName)
            mtime_utc       = $exeUtc.ToString('o')
        }
        freshness     = $freshness
        source        = [ordered]@{
            file_count                   = $inventoryArray.Count
            content_sha256               = $sourceContentHash
            manifest_bytes               = $sourceManifestBytes.Length
            excluded_credential_files    = @($excludedCredentials)
            vendored_license_files       = @($vendoredLicenseFiles)
            path_packages_skipped        = @($pathPackages)
        }
        crates        = @($crateRecords | ForEach-Object {
                [ordered]@{
                    name         = $_.Name
                    version      = $_.Version
                    dir          = $_.Dir
                    license_files = @($_.LicenseFiles)
                }
            })
        artifacts     = @($artifactList)
        claims        = [ordered]@{
            local_draft                   = $true
            legal_clearance               = $false
            reproducibility_guarantee      = $false
            byte_identical_rebuild_claimed = $false
            testing_checklists_asserted    = $false
        }
        open_decision_conditions = @(
            'R1: normal compiler CRT treated as GPL-3.0 System Library - DOCUMENTED PROJECT INTERPRETATION (RELEASING.md); reassess for toolchain changes'
            'M1: static application deployment relies on applicable Microsoft product license and deployment guidance - DOCUMENTED PROJECT INTERPRETATION (RELEASING.md); builder license not attested by this tool'
        )
    }
    $provenanceText = $provenance | ConvertTo-Json -Depth 8
    Write-Utf8NoBom -Path (Join-Path $partialDir 'PROVENANCE.json') -Text ($provenanceText + "`n")

    # Final per-file manifest: every packaged file except the manifest itself.
    $pkgRoot = (Get-Item -LiteralPath $partialDir).FullName
    $finalLines = [System.Collections.Generic.List[string]]::new()
    $pkgFileInfos = Get-ChildItem -LiteralPath $pkgRoot -File -Recurse
    foreach ($f in $pkgFileInfos) {
        $rel = $f.FullName.Substring($pkgRoot.Length).TrimStart('\').Replace('\', '/')
        if ($rel -eq 'MANIFEST.sha256') { continue }
        $finalLines.Add((Get-Sha256Hex $f.FullName) + '  ' + $rel)
    }
    $sortedLines = $finalLines.ToArray()
    [System.Array]::Sort($sortedLines, [System.StringComparer]::Ordinal)
    $manifestBody = (($sortedLines -join "`n") + "`n")
    Write-Utf8NoBom -Path (Join-Path $pkgRoot 'MANIFEST.sha256') -Text $manifestBody
    $manifestHash = Get-Sha256Hex (Join-Path $pkgRoot 'MANIFEST.sha256')

    if (Test-Path -LiteralPath $targetDir) {
        Fail -Code $script:EXIT_OUTPUT -Message "output folder appeared during packaging, refusing to overwrite: $targetDir"
    }
    Move-Item -LiteralPath $pkgRoot -Destination $targetDir
}
catch {
    Write-Err ("packaging failed after partial output was written to '{0}'; the folder is left in place as evidence and is NOT a valid package: {1}" -f $partialDir, $_.Exception.Message)
    exit $script:EXIT_INTERNAL
}

Write-Output ''
Write-Output "LOCAL DRAFT PACKAGE ONLY - no legal clearance, no reproducibility guarantee, nothing published."
Write-Output "package_dir=$targetDir"
Write-Output "package_name=$packageName"
Write-Output "head_commit=$headCommit branch=$branch dirty=$gitDirty policy=$devPolicy"
Write-Output "build_mode=$buildMode build_exit=$($buildRecord.exit_code)"
Write-Output "executable=$exeTargetName size=$($exeItem.Length) sha256=$(Get-Sha256Hex (Join-Path $targetDir $exeTargetName))"
Write-Output "source_files=$($inventoryArray.Count) source_content_sha256=$sourceContentHash"
Write-Output "crates=$($crateRecords.Count) excluded_credentials=$($excludedCredentials.Count)"
Write-Output "manifest_sha256=$manifestHash"
exit $script:EXIT_OK
