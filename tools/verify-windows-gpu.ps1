param(
  [Parameter(Mandatory = $true)][string]$ModelDirectory,
  [string]$Image = "tests/fixtures/images/hard-edge-opaque.png"
)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
$previousRuntime = $env:S08_NATIVE_RUNTIME
$previousModels = $env:S08_MODEL_DIR
$previousImage = $env:S08_PERF_IMAGE
$temporary = $null
try {
  if ($env:OS -ne "Windows_NT") { throw "Run this script on Windows x64." }
  Get-Command cargo -ErrorAction Stop | Out-Null
  $metadata = Get-Content "resources/runtime/runtime.json" -Raw | ConvertFrom-Json
  if ($metadata.target -ne "win32-x64" -or $metadata.version -ne "1.24.4") {
    throw "Run npm run installer:prepare on Windows to prepare the pinned DirectML runtime."
  }
  $env:S08_NATIVE_RUNTIME = (Resolve-Path "resources/runtime/onnxruntime.dll").Path
  $env:S08_MODEL_DIR = (Resolve-Path -LiteralPath $ModelDirectory).Path
  $env:S08_PERF_IMAGE = (Resolve-Path -LiteralPath $Image).Path
  New-Item -ItemType Directory -Force "test-output/acceleration-windows" | Out-Null

  function Invoke-Check([string]$Name, [string[]]$Arguments) {
    $previousPreference = $ErrorActionPreference
    try {
      # Windows PowerShell 5.1 wraps native stderr as ErrorRecord even on success.
      $ErrorActionPreference = "Continue"
      & cargo @Arguments 2>&1 | Tee-Object -FilePath "test-output/acceleration-windows/$Name.log"
      $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previousPreference }
    if ($code -ne 0) { throw "$Name failed; inspect its log." }
  }
  Invoke-Check "quality" @("test", "-p", "cutout-core", "--release", "--test", "acceleration_probe",
    "production_gpu_matches_cpu_quality_gate", "--", "--ignored", "--nocapture", "--test-threads=1")
  Invoke-Check "providers" @("test", "-p", "cutout-core", "--release", "--test", "acceleration_probe",
    "windows_profile_contains_directml_nodes", "--", "--ignored", "--nocapture", "--test-threads=1")
  Invoke-Check "execution-recovery" @("test", "-p", "cutout-core", "--release", "--lib",
    "native_accelerator_error_retries_cpu_and_rechecks_model", "--", "--ignored", "--nocapture", "--test-threads=1")

  # A new process and isolated runtime test a missing GPU DLL without changing the installed app.
  $temporary = Join-Path $root ("test-output/windows-cpu-fallback-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory $temporary | Out-Null
  Get-ChildItem "resources/runtime/*.dll" | Where-Object { $_.Name -ne "DirectML.dll" } |
    Copy-Item -Destination $temporary
  $env:S08_NATIVE_RUNTIME = Join-Path $temporary "onnxruntime.dll"
  Invoke-Check "missing-directml" @("test", "-p", "cutout-core", "--release", "--test", "acceleration_probe",
    "windows_missing_directml_falls_back_to_cpu", "--", "--ignored", "--nocapture", "--test-threads=1")
  Write-Host "Passed. Reports: test-output/acceleration-windows/"
} finally {
  $env:S08_NATIVE_RUNTIME = $previousRuntime
  $env:S08_MODEL_DIR = $previousModels
  $env:S08_PERF_IMAGE = $previousImage
  if ($temporary -and (Test-Path -LiteralPath $temporary)) { Remove-Item -LiteralPath $temporary -Recurse -Force }
  Pop-Location
}
