<#
.SYNOPSIS
  Runs CSpell across the zerho repository using root cspell.json.
.EXAMPLE
  ./scripts/spellcheck.ps1
  ./scripts/spellcheck.ps1 -ListUnknown
#>
param(
    [switch]$ListUnknown
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

Push-Location $RepoRoot
try {
    $argsList = @("--yes", "cspell", "--config", "cspell.json", "--no-progress")
    if ($ListUnknown) {
        $argsList += @("--words-only", "--unique")
    } else {
        $argsList += @("--show-suggestions")
    }
    $argsList += "**"

    & npx @argsList
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
