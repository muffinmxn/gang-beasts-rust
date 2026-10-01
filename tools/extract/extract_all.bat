@echo off
rem Extracts everything the engine needs from YOUR OWN installed copy of Gang Beasts into assets\export.
rem   set GB_GAME_DIR=C:\path\to\Gang Beasts\Content     (the folder that contains "Gang Beasts_Data")
rem   python -m pip install -r tools\extract\requirements.txt Pillow
rem Run from the repository root:  tools\extract\extract_all.bat
rem Nothing here is shipped with the engine; the output is for your personal use only.
setlocal
if "%GB_GAME_DIR%"=="" (echo Set GB_GAME_DIR first. & exit /b 1)
set PY=python
set X=tools\extract
%PY% %X%\export.py settings physics
%PY% %X%\export.py prefab "Core/Beasts/actor_humanoidMediumEctomorph.prefab" beast
%PY% %X%\export_player_colors.py
%PY% %X%\export_strings.py
%PY% %X%\export_fonts.py
%PY% %X%\export_prompt_sprites.py
%PY% %X%\export_costume_items.py
%PY% %X%\export_costumes.py
for %%S in (menu rooftop aquarium incinerator alley billboard blimp buoy chute containers crane elevators girders gondola grind lighthouse ring subway towers train trawler trucks vents wheel) do (
  echo == %%S
  %PY% %X%\export.py scene stages-%%S_scenes %%S
  %PY% %X%\export.py graphics stages-%%S_scenes %%S-graphics
)
echo Done. Build and run:  cargo run -p gb_game -- menu
