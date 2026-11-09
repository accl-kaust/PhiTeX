# phitex end to end on a TeX Live install, Windows or Linux alike
# (checks.yml's texlive job): `phitex build` of a small LaTeX document
# (a table of contents, cross-references and mathematics: reruns) in a
# folder whose name has a space, then `phitex watch` on it: its first
# build, an edit on disk built again, and `q` quitting it. The PDF goes
# to $Out, for the job comparing the systems' PDFs byte for byte.
#
#   pwsh scripts/ci/e2e.ps1 -Phitex target/fastdev/phitex -TexBin ~/texlive/tl/bin/x86_64-linux -Out e2e
param(
    [Parameter(Mandatory)] [string] $Phitex,
    [Parameter(Mandatory)] [string] $TexBin,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
$full = { param($p) [IO.Path]::GetFullPath($p.Replace('~', $HOME)) }
$Phitex = & $full $Phitex
$TexBin = & $full $TexBin
$Out = & $full $Out
$env:PATH = $TexBin + [IO.Path]::PathSeparator + $env:PATH
$env:SOURCE_DATE_EPOCH = '0'
$env:FORCE_SOURCE_DATE = '1'
$env:PHITEX_CACHE_DIR = Join-Path ([IO.Path]::GetTempPath()) "phitex cache $PID"
$dir = Join-Path ([IO.Path]::GetTempPath()) "phitex e2e $PID"
New-Item -ItemType Directory -Force $dir, $Out | Out-Null
Set-Location $dir
@'
\documentclass{article}
\usepackage{amsmath}
\title{PhiTeX on two systems}
\author{CI}
\date{}
\begin{document}
\maketitle
\tableofcontents
\section{Mathematics}\label{sec:math}
Euler's identity, \eqref{eq:euler}, is in Section~\ref{sec:math} on
page~\pageref{eq:euler}.
\begin{equation}\label{eq:euler}
  e^{i\pi} + 1 = 0
\end{equation}
\begin{align}
  \sum_{k=1}^{n} k &= \frac{n(n+1)}{2}, &
  \int_0^1 x^2\,dx &= \frac{1}{3}.
\end{align}
\section{Text}
The quick brown fox jumps over the lazy dog. \emph{Emphasis}, \textbf{bold}
and \texttt{typewriter} text, and a footnote.\footnote{Down here.}
\end{document}
'@ | Set-Content -NoNewline -Encoding utf8NoBOM 'doc.tex'

& $Phitex --version
& $Phitex build doc.tex
if ($LASTEXITCODE -ne 0) { throw "phitex build failed ($LASTEXITCODE)" }
if (-not (Test-Path 'doc.pdf')) { throw 'phitex build wrote no doc.pdf' }
Copy-Item 'doc.pdf' (Join-Path $Out 'doc.pdf')
Write-Host "build: doc.pdf, $((Get-Item doc.pdf).Length) bytes"

# (the watch: its JSON events read a line at a time, each wait bounded)
$si = [Diagnostics.ProcessStartInfo]::new($Phitex)
foreach ($a in 'watch', '--write', '--json', '-y', 'doc.tex') { $si.ArgumentList.Add($a) }
$si.WorkingDirectory = $dir
$si.RedirectStandardInput = $true
$si.RedirectStandardOutput = $true
$si.UseShellExecute = $false
$p = [Diagnostics.Process]::Start($si)
function Wait-Build([int] $n) {
    $until = [DateTime]::UtcNow.AddMinutes(10)
    while ([DateTime]::UtcNow -lt $until) {
        $t = $p.StandardOutput.ReadLineAsync()
        if (-not $t.Wait([TimeSpan]::FromMinutes(10))) { break }
        $l = $t.Result
        if ($null -eq $l) { throw "the watch ended before build $n" }
        Write-Host "watch: $l"
        $ev = try { $l | ConvertFrom-Json } catch { $null }
        if ($ev -and $ev.build -eq $n) {
            if ($ev.ok -ne $true) { throw "the watch's build $n failed" }
            return
        }
    }
    $p.Kill()
    throw "no build $n from the watch in 10 minutes"
}
Wait-Build 1
Start-Sleep -Milliseconds 200
(Get-Content -Raw 'doc.tex').Replace('lazy dog', 'sleepy dog') | Set-Content -NoNewline -Encoding utf8NoBOM 'doc.tex'
Wait-Build 2
$p.StandardInput.WriteLine('q')
$p.StandardInput.Close()
if (-not $p.WaitForExit(60000)) { $p.Kill(); throw 'the watch did not quit on q' }
Write-Host "watch: quit, status $($p.ExitCode)"
if ($p.ExitCode -ne 0) { throw "the watch exited $($p.ExitCode)" }
Set-Location $Out
Remove-Item -Recurse -Force $dir
