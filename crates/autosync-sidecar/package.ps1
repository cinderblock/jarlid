# Write the manifest Jarlid downloads the auto-sync engine from.
#
#   pwsh crates/autosync-sidecar/package.ps1 -Dist <dir> -BaseUrl <url> [-Out <file>]
#
# <dir> holds jarlid-autosync.exe and the engine DLLs (build-engine.ps1). Every file is listed with
# its size and SHA-256 under <url>/<name>, plus the speech model from model.json, which is pinned
# to a Hugging Face commit rather than a branch so the bytes behind its URL can never change.
#
# The app pins only this manifest's own SHA-256 (app/src-tauri/src/autosync/component.rs). Every
# other file is then trusted through the hash the manifest gives for it, so one reviewed constant
# covers the whole download.
param(
    [Parameter(Mandatory = $true)][string]$Dist,
    [Parameter(Mandatory = $true)][string]$BaseUrl,
    [string]$Out = (Join-Path $Dist "autosync-manifest.json")
)
$ErrorActionPreference = "Stop"

$cargo = Get-Content (Join-Path $PSScriptRoot "Cargo.toml") -Raw
$version = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$protocol = [regex]::Match((Get-Content (Join-Path $PSScriptRoot "src\protocol.rs") -Raw), 'pub const PROTOCOL: u32 = (\d+);').Groups[1].Value
if (-not $version -or -not $protocol) { throw "could not read version or protocol" }

$files = Get-ChildItem $Dist -File |
    Where-Object { $_.Extension -in ".exe", ".dll" } |
    Sort-Object Name |
    ForEach-Object {
        [ordered]@{
            name   = $_.Name
            url    = "$($BaseUrl.TrimEnd('/'))/$($_.Name)"
            sha256 = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
            size   = $_.Length
        }
    }
if (-not ($files | Where-Object { $_.name -eq "jarlid-autosync.exe" })) { throw "jarlid-autosync.exe missing from $Dist" }

$manifest = [ordered]@{
    version  = $version
    protocol = [int]$protocol
    files    = @($files)
    model    = Get-Content (Join-Path $PSScriptRoot "model.json") -Raw | ConvertFrom-Json
}
# LF, no BOM: the hash the app pins is of these exact bytes.
$json = ($manifest | ConvertTo-Json -Depth 5) -replace "`r`n", "`n"
[IO.File]::WriteAllText($Out, $json + "`n", [Text.UTF8Encoding]::new($false))
$hash = (Get-FileHash $Out -Algorithm SHA256).Hash.ToLower()
Write-Output "manifest: $Out"
Write-Output "  version $version, protocol $protocol, $($files.Count) files"
Write-Output "  sha256 $hash"
