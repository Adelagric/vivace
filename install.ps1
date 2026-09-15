# Installs the vivacity binary from the latest GitHub release (Windows x86_64).
#   irm https://raw.githubusercontent.com/Adelagric/vivacity/main/install.ps1 | iex
# or, as a file: powershell -ExecutionPolicy Bypass -File install.ps1
# Variables: VIVACITY_VERSION (default: latest), VIVACITY_INSTALL_DIR
#            (default: %LOCALAPPDATA%\Programs\vivacity), VIVACITY_DOWNLOAD_BASE
#            (default: the GitHub release downloads — a directory or URL holding
#            the assets, used by the CI self-test).
$ErrorActionPreference = 'Stop'
$repo = 'Adelagric/vivacity'
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
  Write-Error "vivacity: no Windows $arch build yet (x86_64 only); build from source with cargo"
}
$target = 'x86_64-pc-windows-msvc'
$dir = if ($env:VIVACITY_INSTALL_DIR) { $env:VIVACITY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\vivacity' }
$tag = $env:VIVACITY_VERSION
if (-not $tag) {
  # The `releases/latest` redirect carries the tag: no API call, no rate limit.
  # HttpWebRequest works the same on Windows PowerShell 5.1 and pwsh 7.
  $req = [System.Net.HttpWebRequest]::Create("https://github.com/$repo/releases/latest")
  $req.Method = 'HEAD'
  $req.AllowAutoRedirect = $false
  $location = $req.GetResponse().Headers['Location']
  if (-not $location) { Write-Error 'vivacity: could not determine the latest release' }
  $tag = ($location -split '/tag/')[-1]
}
$name = "vivacity-$tag-$target.tar.gz"
$base = if ($env:VIVACITY_DOWNLOAD_BASE) { $env:VIVACITY_DOWNLOAD_BASE } else { "https://github.com/$repo/releases/download/$tag" }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("vivacity-install-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "vivacity: downloading $name"
  if ($base -match '^(https?|file)://') {
    Invoke-WebRequest -Uri "$base/$name" -OutFile (Join-Path $tmp $name)
    Invoke-WebRequest -Uri "$base/$name.sha256" -OutFile (Join-Path $tmp "$name.sha256")
  } else {
    Copy-Item (Join-Path $base $name), (Join-Path $base "$name.sha256") -Destination $tmp
  }
  # `<hex>  <name>` as written by sha256sum; compare case-insensitively.
  $expected = ((Get-Content (Join-Path $tmp "$name.sha256") -Raw) -split '\s+')[0]
  $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $name)).Hash
  if ($expected.ToLower() -ne $actual.ToLower()) {
    Write-Error "vivacity: sha256 mismatch for $name (expected $expected, got $actual)"
  }
  # tar.exe (bsdtar) ships with Windows 10 1803+.
  & tar.exe -xzf (Join-Path $tmp $name) -C $tmp vivacity.exe
  if ($LASTEXITCODE -ne 0) { Write-Error "vivacity: tar.exe failed ($LASTEXITCODE)" }
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Copy-Item (Join-Path $tmp 'vivacity.exe') (Join-Path $dir 'vivacity.exe') -Force
  Write-Host "vivacity: installed to $dir\vivacity.exe ($tag)"
  $onPath = ($env:PATH -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $dir.TrimEnd('\')) }
  if (-not $onPath) {
    Write-Host "vivacity: add $dir to your PATH, e.g.:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$dir', 'User')"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
