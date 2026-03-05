param(
  [Parameter(Mandatory = $true)][string]$Tag,
  [Parameter(Mandatory = $true)][string]$TargetTriple,
  [Parameter(Mandatory = $true)][string]$BinaryPath,
  [Parameter(Mandatory = $true)][string]$OutputDir
)

$ErrorActionPreference = 'Stop'
$AppName = 'Arbor'
$StagingDir = Join-Path $OutputDir "$AppName-$Tag-$TargetTriple"
$ArchivePath = Join-Path $OutputDir "$AppName-$Tag-$TargetTriple.zip"

New-Item -Path $StagingDir -ItemType Directory -Force | Out-Null
Copy-Item -Path $BinaryPath -Destination (Join-Path $StagingDir "$AppName.exe") -Force
Copy-Item -Path README.md -Destination (Join-Path $StagingDir 'README.md') -Force

if (Test-Path $ArchivePath) {
  Remove-Item -Path $ArchivePath -Force
}
Compress-Archive -Path (Join-Path $StagingDir '*') -DestinationPath $ArchivePath

Write-Output $ArchivePath
