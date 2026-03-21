@echo off
set "VSWHERE=C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"

for /f "usebackq tokens=*" %%B in (`"%VSWHERE%" -latest -prerelease -products * -requires Microsoft.Component.MSBuild -find MSBuild\**\Bin\MSBuild.exe`) do (
    set "MSBUILD=%%B"
)

echo MSBUILD=%MSBUILD%

set OPENCON=%~dp0
set ARCH=x64

echo Building TerminalApp (Debug x64)...
"%MSBUILD%" "%OPENCON%OpenConsole.slnx" /t:Build /m /p:Configuration=Debug /p:GenerateAppxPackageOnBuild=false /p:Platform=x64 /p:AppxBundle=false /v:minimal
