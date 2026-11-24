@echo off
title Spotify Downloader Menu

:menu
cls
echo.
echo ============ Spotify Downloader Options ============
echo.
echo   1. Download Song (URL/URI/ID)
echo   2. Download Album (URL/URI/ID)
echo   3. Download Playlist (URL/URI/ID)
echo   4. Download by Search Query
echo   5. Configure Settings
echo   6. Exit
echo.
echo ___________________________________________________________
set "choice="  REM Clear previous choice
set /p choice=" Enter your choice (1-6): "

if "%choice%"=="1" goto song
if "%choice%"=="2" goto album
if "%choice%"=="3" goto playlist
if "%choice%"=="4" goto query
if "%choice%"=="5" goto settings
if "%choice%"=="6" goto exit

echo.
echo Invalid option selected. Please try again.
timeout /t 2 /nobreak > nul
goto menu

:song
cls
echo ========= Download Spotify Song =========
echo.
set "input=" REM Clear previous input
set /p input=" Enter URL/URI/ID for the Song: "
if not defined input (
    echo Input cannot be empty. Returning to menu...
    timeout /t 2 /nobreak > nul
    goto menu
)
echo.
echo Running command: node spotify_node.js song "%input%"
echo ----------------------------------------------------
node spotify_node.js song "%input%"
echo ----------------------------------------------------
echo.
echo Operation complete. Press any key to return to the menu...
pause > nul
goto menu

:album
cls
echo ========= Download Spotify Album =========
echo.
set "input="
set /p input=" Enter URL/URI/ID for the Album: "
 if not defined input (
    echo Input cannot be empty. Returning to menu...
    timeout /t 2 /nobreak > nul
    goto menu
)
echo.
echo Running command: node spotify_node.js album "%input%"
echo ----------------------------------------------------
node spotify_node.js album "%input%"
echo ----------------------------------------------------
echo.
echo Operation complete. Press any key to return to the menu...
pause > nul
goto menu

:playlist
cls
echo ======== Download Spotify Playlist ========
echo.
set "input="
set /p input=" Enter URL/URI/ID for the Playlist: "
 if not defined input (
    echo Input cannot be empty. Returning to menu...
    timeout /t 2 /nobreak > nul
    goto menu
)
echo.
echo Running command: node spotify_node.js playlist "%input%"
echo ----------------------------------------------------
node spotify_node.js playlist "%input%"
echo ----------------------------------------------------
echo.
echo Operation complete. Press any key to return to the menu...
pause > nul
goto menu

:query
cls
echo ====== Download by Search Query ======
echo.
set "input="
set /p input=" Enter your search query (e.g., Artist - Title): "
 if not defined input (
    echo Input cannot be empty. Returning to menu...
    timeout /t 2 /nobreak > nul
    goto menu
)
echo.
echo Running command: node spotify_node.js query "%input%"
echo ----------------------------------------------------
node spotify_node.js query "%input%"
echo ----------------------------------------------------
echo.
echo Operation complete. Press any key to return to the menu...
pause > nul
goto menu

:settings
cls
echo ========= Configure Settings =========
echo.
echo Running command: node spotify_node.js settings
echo ----------------------------------------------------
node spotify_node.js settings
echo ----------------------------------------------------
echo.
echo Settings viewed/updated. Press any key to return to the menu...
pause > nul
goto menu

:exit
cls
echo Exiting Spotify Downloader Menu...
timeout /t 1 /nobreak > nul
exit /b 0