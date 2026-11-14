# Fetch ONNX Runtime for the optional forced aligner: onnxruntime.dll, from Microsoft's pinned
# release zip, checked against its SHA-256.
#
#   pwsh crates/autosync-sidecar/fetch-onnxruntime.ps1 -Out <dir>
#
# The sidecar loads it at run time (the `ort` crate's load-dynamic), so it is never linked: the
# engine runs without it, and it is its own download in the app (plans/word-aligner-in-app.md).
# The version must offer the C API the `ort` crate in Cargo.toml is built for (api-31 → 1.31+).
param(
    [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = "Stop"

$Version = "1.31.1"
$Zip = "onnxruntime-win-x64-$Version.zip"
$Sha256 = "5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c"
$Url = "https://github.com/microsoft/onnxruntime/releases/download/v$Version/$Zip"

$work = Join-Path ([IO.Path]::GetTempPath()) "jarlid-onnxruntime"
New-Item -ItemType Directory -Force $work | Out-Null
$zipPath = Join-Path $work $Zip
if (-not (Test-Path $zipPath) -or (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower() -ne $Sha256) {
    Invoke-WebRequest -Uri $Url -OutFile $zipPath
}
$got = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
if ($got -ne $Sha256) { throw "$Zip has SHA-256 $got, expected $Sha256" }

$extract = Join-Path $work "x"
if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
Expand-Archive -Path $zipPath -DestinationPath $extract
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item (Join-Path $extract "onnxruntime-win-x64-$Version\lib\onnxruntime.dll") $Out
Write-Output "onnxruntime.dll $Version -> $Out"
