# PhiTeX's installer for Windows.
#
#   irm https://phitex.org/install.ps1 | iex
#
# Downloads phitex.exe for this machine from PhiTeX's GitHub release
# (phitex-x86_64-windows.zip), checks it against the release's SHA256SUMS
# (and that file's minisign signature when minisign is installed), puts
# it in %LOCALAPPDATA%\Programs\phitex and adds that folder to the user's
# PATH. Nothing else on the machine is touched, and nothing is sent but
# the downloads themselves. As phitex.org/install.sh does on Linux and
# macOS, with the same settings.
#
# Settings (environment):
#   $env:PHITEX_VERSION = "1.0"      a version instead of the latest release
#   $env:PHITEX_CHANNEL = "nightly"  the nightly (dev's, rebuilt each night)
#                                    instead; `phitex upgrade` then follows it
#   $env:PHITEX_PREFIX = "DIR"       install into DIR (default
#                                    %LOCALAPPDATA%\Programs\phitex)
#   $env:PHITEX_BASE_URL = "URL"     the release's files from URL instead of
#                                    GitHub (a copy of a release; CI's test)
#
# `phitex upgrade` updates an install made by this script. To remove it,
# delete %LOCALAPPDATA%\Programs\phitex (and %LOCALAPPDATA%\phitex, its
# cache and record) and take the folder off the user's PATH.

# (a script block run at once: `irm … | iex` runs the text in the
# caller's session, whose variables and preferences this leaves alone)
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    # (Windows PowerShell 5.1 may not offer TLS 1.2 by default)
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $Repo = 'accl-kaust/PhiTeX'
    # The releases' signing key (minisign; PhiTeX's ci/minisign.pub). With
    # minisign installed, SHA256SUMS's signature is checked against it.
    $PubKey = 'RWS/tPWBRVVf0Y9h/ODPiO2EgZi1labeCog3siaCfIdaC9HJkDJZ8k4u'
    # The nightly's own key (PhiTeX's ci/nightly.pub): a nightly is never
    # checked against the releases' key.
    $NightlyPubKey = 'RWT5jA+G82gT+bMpGqolUeaWPm6OYkLC4m6lI8G5HbUnyL3yY5UpGCYD'

    function Say([string] $m) { [Console]::Error.WriteLine("phitex: $m") }
    function Die([string] $m) { Say $m; throw "phitex: $m" }

    $version = "$env:PHITEX_VERSION"
    $channel = if ($env:PHITEX_CHANNEL) { "$env:PHITEX_CHANNEL" } else { 'stable' }
    $prefix = if ($env:PHITEX_PREFIX) { "$env:PHITEX_PREFIX" } else { Join-Path $env:LOCALAPPDATA 'Programs\phitex' }

    # (the machine's, not this PowerShell's: a 32-bit PowerShell on 64-bit
    # Windows says x86 in PROCESSOR_ARCHITECTURE)
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { $arch = 'x86_64' }
        default { Die "no build for $arch Windows: build from source (https://github.com/$Repo)" }
    }
    $asset = "phitex-$arch-windows.zip"

    switch ($channel) {
        'stable' { }
        'nightly' { if ($version) { Die 'PHITEX_VERSION and PHITEX_CHANNEL=nightly: one or the other' } }
        default { Die "PHITEX_CHANNEL is stable or nightly, not $channel" }
    }
    if ($channel -eq 'nightly') {
        $base = "https://github.com/$Repo/releases/download/nightly"
        $PubKey = $NightlyPubKey
    } elseif ($version) {
        $base = "https://github.com/$Repo/releases/download/v$($version.TrimStart('v'))"
    } else {
        $base = "https://github.com/$Repo/releases/latest/download"
    }
    if ($env:PHITEX_BASE_URL) { $base = "$env:PHITEX_BASE_URL".TrimEnd('/') }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("phitex-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        function Get([string] $url, [string] $to) {
            Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $to
        }
        Say "downloading $(if ($version) { "v$($version.TrimStart('v')) " })$asset"
        try { Get "$base/SHA256SUMS" "$tmp\SHA256SUMS" }
        catch { Die "no release found at $base (is there a release yet? https://github.com/$Repo/releases)" }
        try { Get "$base/$asset" "$tmp\$asset" }
        catch { Die "the release has no $asset" }

        if (Get-Command minisign -ErrorAction SilentlyContinue) {
            try { Get "$base/SHA256SUMS.minisig" "$tmp\SHA256SUMS.minisig" }
            catch { Die 'the release has no signature' }
            & minisign -Vqm "$tmp\SHA256SUMS" -P $PubKey
            if ($LASTEXITCODE -ne 0) { Die "SHA256SUMS's signature does not verify: nothing installed" }
            Say 'signature verified'
        } else {
            Say 'minisign not installed: checked by SHA-256 over HTTPS only'
        }

        $want = $null
        foreach ($l in Get-Content "$tmp\SHA256SUMS") {
            $f = $l -split '\s+', 2
            if ($f.Count -eq 2 -and ($f[1] -eq $asset -or $f[1] -eq "*$asset")) { $want = $f[0].ToLowerInvariant() }
        }
        if (-not $want) { Die "SHA256SUMS does not list $asset" }
        $have = (Get-FileHash -Algorithm SHA256 "$tmp\$asset").Hash.ToLowerInvariant()
        if ($have -ne $want) { Die "$asset does not match SHA256SUMS: nothing installed" }

        Expand-Archive -Path "$tmp\$asset" -DestinationPath "$tmp\x" -Force
        if (-not (Test-Path "$tmp\x\phitex.exe")) { Die "$asset has no phitex.exe" }
        New-Item -ItemType Directory -Force $prefix | Out-Null
        $exe = Join-Path $prefix 'phitex.exe'
        # (beside it, then renamed: a running phitex.exe can be renamed
        # aside but not replaced, so the old one steps out of the way)
        Copy-Item "$tmp\x\phitex.exe" (Join-Path $prefix '.phitex.new') -Force
        Get-ChildItem $prefix -Filter '.phitex-*.old' -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        if (Test-Path $exe) { Move-Item $exe (Join-Path $prefix ".phitex-$PID.old") -Force }
        Move-Item (Join-Path $prefix '.phitex.new') $exe -Force

        $installed = try { (& $exe --version 2>$null | Select-Object -First 1) } catch { $null }
        $data = Join-Path $env:LOCALAPPDATA 'phitex'
        if ($env:XDG_DATA_HOME) { $data = Join-Path $env:XDG_DATA_HOME 'phitex' }
        New-Item -ItemType Directory -Force $data | Out-Null
        [ordered]@{ method = 'script'; prefix = $prefix; asset = $asset; channel = $channel } |
            ConvertTo-Json -Compress | Set-Content -Encoding ascii (Join-Path $data 'install.json')

        Say "installed $(if ($installed) { $installed } else { 'phitex' }) in $prefix"
        # (the user's PATH as the registry keeps it, its %VARIABLES%
        # unexpanded and its kind kept; then the change announced, as
        # SetEnvironmentVariable announces one, by a variable set and
        # removed, so a new terminal sees it)
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            $userPath = [string] $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $parts = @($userPath -split ';' | Where-Object { $_ })
            if ($parts -notcontains $prefix -and $parts -notcontains "$prefix\") {
                $kind = if ($userPath) { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
                $key.SetValue('Path', ((@($parts) + $prefix) -join ';'), $kind)
                [Environment]::SetEnvironmentVariable('PHITEX_INSTALL_PATH_CHANGED', '1', 'User')
                [Environment]::SetEnvironmentVariable('PHITEX_INSTALL_PATH_CHANGED', $null, 'User')
                Say "added $prefix to your PATH: open a new terminal to use phitex"
            }
        } finally {
            $key.Close()
        }
        if (($env:Path -split ';') -notcontains $prefix) { $env:Path = "$env:Path;$prefix" }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
