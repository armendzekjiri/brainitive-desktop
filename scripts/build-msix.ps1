<#
.SYNOPSIS
  Builds the Microsoft Store package (MSIX) of Brainitive Desktop (BRAI-442).

.DESCRIPTION
  1. Builds the Store variant:
       npx tauri build --no-bundle --config src-tauri/tauri.msstore.conf.json --features msstore
  2. Lays out the package folder: Brainitive.exe, AppxManifest.xml (scripts/msix-manifest.mjs,
     version X.Y.Z.0 from tauri.conf.json), Assets\ (scripts/msix-assets.mjs, from
     src-tauri/icons/icon.png) and resources.pri (makepri: the scale / targetsize logos).
  3. makeappx pack (validates the manifest against the schema) ->
     <OutDir>\Brainitive_<X.Y.Z>_x64.msix

  The package is UNSIGNED: Partner Center signs it for the Store. -SignForTest additionally writes
  a sideload copy signed with a throwaway self-signed certificate made on the fly (subject = the
  Store publisher) and that certificate's public part (.cer) to trust on a test PC. The private
  key is deleted right after signing; nothing is committed. See docs/MICROSOFT-STORE.md.

  Needs Windows 10/11 with the Windows SDK (makeappx, makepri, signtool), Node and Rust.

.EXAMPLE
  pwsh scripts/build-msix.ps1
  pwsh scripts/build-msix.ps1 -SignForTest
  pwsh scripts/build-msix.ps1 -SkipBuild    # package the exe a previous Store build left
#>
[CmdletBinding()]
param(
    # where the package folder and the .msix go (relative to the repo)
    [string]$OutDir = "src-tauri\target\msix",
    # don't build: package src-tauri\target\release's exe (it must be the msstore build)
    [switch]$SkipBuild,
    # also write a self-signed sideload copy + its .cer (local testing only, never uploaded)
    [switch]$SignForTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot

function Invoke-Checked {
    param([string]$Exe, [string[]]$Arguments)
    Write-Host "> $([IO.Path]::GetFileName($Exe)) $($Arguments -join ' ')"
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$([IO.Path]::GetFileName($Exe)) failed (exit code $LASTEXITCODE)"
    }
}

# The newest Windows SDK's x64 tool: ...\Windows Kits\10\bin\<10.0.x.y>\x64\<name>.
function Find-SdkTool {
    param([string]$Name)
    $roots = @("${env:ProgramFiles(x86)}\Windows Kits\10\bin", "$env:ProgramFiles\Windows Kits\10\bin")
    $found = @(foreach ($root in $roots) {
        if (Test-Path $root) {
            Get-ChildItem $root -Directory |
                Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
                ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
                Where-Object { Test-Path $_ }
        }
    })
    if ($found.Count -eq 0) { throw "$Name not found: install the Windows SDK" }
    $found | Sort-Object { [version](Split-Path (Split-Path (Split-Path $_ -Parent) -Parent) -Leaf) } -Descending |
        Select-Object -First 1
}

Push-Location $repo
try {
    $version = (Get-Content "src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json).version
    $out = if ([IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $repo $OutDir }
    $pkg = Join-Path $out "package"
    $manifest = Join-Path $pkg "AppxManifest.xml"
    $msix = Join-Path $out "Brainitive_${version}_x64.msix"

    # 1. the Store variant (no updater, Store links, startup task, package toasts)
    if (-not $SkipBuild) {
        Invoke-Checked "npx" @("tauri", "build", "--no-bundle", "--config", "src-tauri/tauri.msstore.conf.json", "--features", "msstore")
    }
    # Tauri may name the binary after the product or the crate: take the newest of the two
    $release = Join-Path $repo "src-tauri\target\release"
    $exe = @("Brainitive.exe", "brainitive-desktop.exe") |
        ForEach-Object { Join-Path $release $_ } |
        Where-Object { Test-Path $_ } |
        Sort-Object { (Get-Item $_).LastWriteTimeUtc } -Descending |
        Select-Object -First 1
    if (-not $exe) { throw "no release exe in $release (run without -SkipBuild)" }
    Write-Host "exe: $exe"

    # 2. the package folder
    if (Test-Path $pkg) { Remove-Item $pkg -Recurse -Force }
    New-Item -ItemType Directory -Path $pkg -Force | Out-Null
    Copy-Item $exe (Join-Path $pkg "Brainitive.exe")
    Invoke-Checked "node" @("scripts/msix-manifest.mjs", $manifest, $version)
    Invoke-Checked "node" @("scripts/msix-assets.mjs", "src-tauri/icons/icon.png", (Join-Path $pkg "Assets"))

    # resources.pri: lets Windows pick Square44x44Logo.targetsize-24_altform-unplated.png etc. for
    # the manifest's plain Assets\Square44x44Logo.png. The config file stays outside the package.
    $makepri = Find-SdkTool "makepri.exe"
    $priConfig = Join-Path $out "priconfig.xml"
    Invoke-Checked $makepri @("createconfig", "/cf", $priConfig, "/dq", "en-US", "/pv", "10.0.0", "/o")
    Invoke-Checked $makepri @("new", "/pr", $pkg, "/cf", $priConfig, "/mn", $manifest, "/of", (Join-Path $pkg "resources.pri"), "/o")

    # 3. the package; makeappx refuses a manifest that doesn't match the schema
    $makeappx = Find-SdkTool "makeappx.exe"
    Invoke-Checked $makeappx @("pack", "/d", $pkg, "/p", $msix, "/o")
    $hash = (Get-FileHash $msix -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "MSIX (unsigned, for Partner Center): $msix"
    Write-Host "SHA-256: $hash"

    $sideload = $null
    $cer = $null
    if ($SignForTest) {
        # a throwaway code-signing certificate whose subject is the Store publisher (Windows only
        # installs a package signed by its Identity/@Publisher)
        $publisher = ([xml](Get-Content $manifest -Raw)).Package.Identity.Publisher
        $cert = New-SelfSignedCertificate -Type Custom -Subject $publisher -KeyUsage DigitalSignature `
            -FriendlyName "Brainitive MSIX sideload test (throwaway)" -CertStoreLocation "Cert:\CurrentUser\My" `
            -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}") -NotAfter (Get-Date).AddDays(30)
        try {
            $sideload = Join-Path $out "Brainitive_${version}_x64_sideload-test.msix"
            $cer = Join-Path $out "Brainitive_sideload-test.cer"
            Copy-Item $msix $sideload -Force
            $signtool = Find-SdkTool "signtool.exe"
            Invoke-Checked $signtool @("sign", "/fd", "SHA256", "/sha1", $cert.Thumbprint, $sideload)
            Export-Certificate -Cert $cert -FilePath $cer -Type CERT | Out-Null
        }
        finally {
            # the private key never outlives this run
            Remove-Item -Path "Cert:\CurrentUser\My\$($cert.Thumbprint)" -DeleteKey -ErrorAction SilentlyContinue
        }
        Write-Host "Sideload test copy (self-signed, NOT for the Store): $sideload"
        Write-Host "Its certificate (trust it in Local Machine > Trusted People on the test PC): $cer"
    }

    if ($env:GITHUB_OUTPUT) {
        Add-Content -Path $env:GITHUB_OUTPUT -Value "msix=$msix"
        Add-Content -Path $env:GITHUB_OUTPUT -Value "version=$version"
        if ($sideload) {
            Add-Content -Path $env:GITHUB_OUTPUT -Value "sideload=$sideload"
            Add-Content -Path $env:GITHUB_OUTPUT -Value "cer=$cer"
        }
    }
}
finally {
    Pop-Location
}
