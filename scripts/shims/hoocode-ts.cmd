@echo off
rem hoocode-ts: run the TypeScript hoocode (npm @kolisachint/hoocode-agent).
rem `hoocode` now names the Rust build; see scripts/shims/hoocode-ts for details.
setlocal
set "PKG=@kolisachint/hoocode-agent"

if defined HOOCODE_TS_BIN goto custom

for /f "delims=" %%r in ('npm root -g 2^>nul') do set "NPMROOT=%%r"
if defined NPMROOT if exist "%NPMROOT%\@kolisachint\hoocode-agent\bin\hoocode.js" goto global

npx --yes --package %PKG% -- hoocode %*
exit /b %ERRORLEVEL%

:custom
"%HOOCODE_TS_BIN%" %*
exit /b %ERRORLEVEL%

:global
node "%NPMROOT%\@kolisachint\hoocode-agent\bin\hoocode.js" %*
exit /b %ERRORLEVEL%
