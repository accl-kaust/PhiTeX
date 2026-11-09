# TeX Live's scheme-basic (plain TeX, LaTeX, pdfTeX) installed into
# $Root from $Repo, on Windows or Linux alike (checks.yml's texlive job:
# a frozen year's tlnet-final, so the two systems get the same files).
#
#   pwsh scripts/ci/install-tl.ps1 -Root ~/texlive -Repo https://…/2025/tlnet-final
param(
    [Parameter(Mandatory)] [string] $Root,
    [Parameter(Mandatory)] [string] $Repo
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Root = [IO.Path]::GetFullPath($Root.Replace('~', $HOME))
$work = Join-Path ([IO.Path]::GetTempPath()) "install-tl-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$texdir = (Join-Path $Root 'tl') -replace '\\', '/'
@"
selected_scheme scheme-basic
TEXDIR $texdir
TEXMFLOCAL $texdir/texmf-local
TEXMFSYSCONFIG $texdir/texmf-config
TEXMFSYSVAR $texdir/texmf-var
TEXMFHOME ~/texmf
TEXMFCONFIG ~/.texlive/texmf-config
TEXMFVAR ~/.texlive/texmf-var
instopt_adjustpath 0
instopt_adjustrepo 0
instopt_write18_restricted 1
tlpdbopt_autobackup 0
tlpdbopt_install_docfiles 0
tlpdbopt_install_srcfiles 0
tlpdbopt_desktop_integration 0
tlpdbopt_file_assocs 0
tlpdbopt_w32_multi_user 0
"@ | Set-Content -Encoding ascii (Join-Path $work 'tl.profile')
if ($IsWindows) {
    Invoke-WebRequest "$Repo/install-tl.zip" -OutFile (Join-Path $work 'install-tl.zip')
    Expand-Archive (Join-Path $work 'install-tl.zip') $work
    $it = Get-ChildItem $work -Directory -Filter 'install-tl-*' | Select-Object -First 1
    & cmd.exe /d /c "`"$($it.FullName)\install-tl-windows.bat`" -no-gui -profile `"$work\tl.profile`" -repository $Repo"
} else {
    Invoke-WebRequest "$Repo/install-tl-unx.tar.gz" -OutFile (Join-Path $work 'install-tl.tgz')
    tar -xzf (Join-Path $work 'install-tl.tgz') -C $work
    $it = Get-ChildItem $work -Directory -Filter 'install-tl-*' | Select-Object -First 1
    & perl (Join-Path $it.FullName 'install-tl') -profile (Join-Path $work 'tl.profile') -repository $Repo
}
if ($LASTEXITCODE -ne 0) { throw "install-tl failed ($LASTEXITCODE)" }
Remove-Item -Recurse -Force $work
Get-ChildItem (Join-Path $texdir 'bin') | ForEach-Object { $_.FullName }
