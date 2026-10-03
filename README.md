# gb-rust — a Rust / Bevy / PhysX engine for Gang Beasts levels

An unofficial, from-scratch **engine** that loads data extracted from *your own legitimately owned copy* of
**Gang Beasts** and runs it: ragdoll beasts on PhysX 4.1, the stage physics, rounds, costumes, menus and
some stage events (trains, scrolling roads, spinning wheel…).

**This repository contains no game content.** No models, textures, sounds, fonts, shaders, level data or
decompiled game code are included — only original Rust source and Python tools that read *your* installed
game. You need to own the game to use it, and you must not redistribute what the tools extract.

> Gang Beasts is © Boneloaf / Double Fine. This project is not affiliated with or endorsed by them.
> The code is MIT licensed (see `LICENSE`); that licence covers this code only, not the game or its data.
> Some gameplay constants and behaviours were derived by studying how the game behaves; if you are a rights
> holder and want something changed or removed, open an issue.

## What you need

| | |
|---|---|
| OS | Windows 10/11 (developed there; the Rust code is portable but the tooling is `.bat` / D3D-flavoured) |
| Rust | stable, via <https://rustup.rs> (MSVC toolchain) |
| C++ build tools | Visual Studio Build Tools (C++), and **CMake** — `physx-sys` compiles PhysX 4.1 |
| Python | 3.11 or newer, with `pip install -r tools/extract/requirements.txt Pillow` |
| The game | a legitimate install of Gang Beasts (Steam or Microsoft Store). Make sure it is the PC build |

The first `cargo build` takes a long time (it builds PhysX and Bevy). It is capped at 4 parallel jobs in
`.cargo/config.toml` so your machine stays usable; raise it with `cargo build -j 8`.

## 1. Point the tools at your game

Find the folder that **contains** `Gang Beasts_Data` (for example
`C:\Program Files (x86)\Steam\steamapps\common\Gang Beasts` or the Microsoft Store `...\Content`), then:

```bat
set GB_GAME_DIR=C:\path\to\the\folder\that\contains\Gang Beasts_Data
```

Do **not** extract from, or write into, a store-protected folder you cannot read; copy the game folder
somewhere you own and point `GB_GAME_DIR` at the copy. The tools only ever *read* the game.

## 2. Extract the data the engine needs

```bat
python -m pip install -r tools\extract\requirements.txt Pillow
tools\extract\extract_all.bat
```

This writes `assets\export\` (glTF + JSON sidecars for the beast, the costumes, the UI and each stage, plus
lightmaps and settings). It is `.gitignore`d. Individual steps are documented at the top of each script in
`tools\extract\` (`export.py` has the stage / prefab / settings commands), so you can extract just one stage:

```bat
python tools\extract\export.py scene stages-rooftop_scenes rooftop
python tools\extract\export.py graphics stages-rooftop_scenes rooftop-graphics
```

Some stages reference meshes that are not in the shipped bundles; the exporter skips those and logs
`skip missing mesh`. Content bundles are added with `set GB_EXTRA_BUNDLES=stages-trawler` (comma separated).

### Audio (optional, about 2.6 GB of WAV)

```bat
python tools\extractudio.py all     :: every clip + sound config + per-stage emitters and music
```

This writes `assets\exportudio\*.wav`, `audio-index.json`, `audio-config.json` and `audio-<stage>.json`. The game plays
menu clicks, music beds and stage ambience, object impact sounds (from each stage's `PhysicAudioEmitter` data), punches,
footsteps, grunts and KO stingers. Without these files it runs silent. `GB_NO_AUDIO=1` mutes, `GB_VOLUME=0.8` sets the master.

## 3. Build and run

```bat
cargo build -p gb_game
run.bat                      :: main menu (lobby, costumes, stage + mode picker)
run.bat rooftop              :: straight into a stage
target\debug\gb_game.exe subway --players 2 --wins 3 --mode gang
```

Useful options: `--assets <dir>` (default `assets\export`), `--players N`, `--wins N`,
`--mode melee|gang|waves|football`, `--bots N` (local AI opponents), `--spawn N`, `--player-color N`,
`--costume <preset name>`. Soccer is played on `alley`.
Many `GB_*` environment variables tune rendering and gameplay; press **F2** in game for the live knob panel.

Lobby controls: `Q`/`E` (or ←/→) change colour, `Z`/`X` (or Shift + ←/→) change costume, `B` add an AI player (up to 5) and `Shift+B` remove one. The Mode row picks Melee, Gang, Waves or Soccer and the Stage row only lists maps that mode can be played on. Your choices (mode, stage, wins, AI count, colour) and your best Waves run are remembered in `%APPDATA%\gb-rust\lobby.json`. Gameplay controls are
listed by the in-game pause/controls screen.

## Layout

```
crates/gb_phys   PhysX 4.1 binding + Unity-scene physics import (no Bevy)
crates/gb_logic  beast / actor gameplay logic (no Bevy, unit-tested)
crates/gb_game   the Bevy 0.16 app: rendering, menus, costumes, rounds, stage events
tools/extract    Python exporters that read YOUR game files -> assets/export
tools/harness    optional screenshot / comparison helpers
```

## Status

Playable: 23 stages, modes Melee, Gang, Waves (the game's 4-wave data) and Soccer (Alley), local AI
opponents, costumes, lobby, and many stage events (subway trains, truck roads, Ferris wheel, falling props, doors, fans,
elevator failure, trawler capsize, shark hunting/breaching/carrying, gondola cables that snap under hits, Gerstner sea waves, train landslide). Work in progress: birds (Rooftop/Crane critters), Train-stage track turns, per-stage graphics parity, animation
clips, swimming, audio, networking.

## Contributing

Pull requests are welcome for the engine code. **Never** commit extracted assets, game files or decompiled
code — the `.gitignore` is a whitelist for that reason; keep it that way.
