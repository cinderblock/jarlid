# Build the speech engine the autosync sidecar loads: whisper.cpp's Parakeet runtime and ggml,
# as DLLs, from a pinned whisper.cpp release.
#
#   pwsh crates/autosync-sidecar/build-engine.ps1 [-Out <dir>] [-Work <dir>] [-Jobs 4]
#
# ggml is built with its backends as separately loaded DLLs (GGML_BACKEND_DL): one CPU backend per
# instruction-set level, chosen at run time for the machine it lands on, and a Vulkan backend
# that is simply ignored where there is no usable GPU. One bundle then runs everywhere, which a
# single statically linked build tuned for the build machine would not.
#
# Needs CMake, the MSVC build tools and the Vulkan SDK ($env:VULKAN_SDK). The SDK ships
# SPIRV-HeadersConfig.cmake flat in Lib/cmake, where CMake's package search does not look, so it
# is pointed at directly (see plans/auto-synced-lyrics.md, "Building whisper.cpp with Vulkan").
param(
    [string]$Out = (Join-Path $PSScriptRoot "engine"),
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) "jarlid-engine-build"),
    [int]$Jobs = 4
)
$ErrorActionPreference = "Stop"

# Pinned by tag *and* commit: a tag can be moved, a commit cannot.
$Tag = "v1.9.5"
$Commit = "d1be6fde11ac6e0407606b4e42fe72d34add8037"

if (-not $env:VULKAN_SDK) { throw "VULKAN_SDK is not set; install the Vulkan SDK first." }

$src = Join-Path $Work "whisper.cpp"
$build = Join-Path $Work "build"
if (-not (Test-Path $src)) {
    git clone --quiet --depth 1 --branch $Tag https://github.com/ggml-org/whisper.cpp $src
    if ($LASTEXITCODE) { throw "clone failed" }
}
$head = (git -C $src rev-parse HEAD).Trim()
if ($head -ne $Commit) { throw "whisper.cpp at $head, expected $Commit for $Tag" }

cmake -S $src -B $build `
    -DBUILD_SHARED_LIBS=ON `
    -DGGML_BACKEND_DL=ON `
    -DGGML_CPU_ALL_VARIANTS=ON `
    -DGGML_NATIVE=OFF `
    -DGGML_VULKAN=ON `
    "-DSPIRV-Headers_DIR=$env:VULKAN_SDK\Lib\cmake" `
    -DWHISPER_BUILD_EXAMPLES=OFF `
    -DWHISPER_BUILD_TESTS=OFF
if ($LASTEXITCODE) { throw "configure failed" }

cmake --build $build --config Release -j $Jobs
if ($LASTEXITCODE) { throw "build failed" }

New-Item -ItemType Directory -Force $Out | Out-Null
Get-ChildItem $Out -Filter *.dll | Remove-Item
$dlls = Get-ChildItem (Join-Path $build "bin\Release") -Filter *.dll |
    Where-Object { $_.Name -like "ggml*.dll" -or $_.Name -eq "parakeet.dll" }
$dlls | Copy-Item -Destination $Out
Write-Output "engine: $($dlls.Count) DLLs in $Out"
$dlls | ForEach-Object { Write-Output ("  {0,-28} {1,10:N0}" -f $_.Name, $_.Length) }
