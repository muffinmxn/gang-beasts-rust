# Status and TODO (for contributors / forks)

This file is the living to-do list. **Update it in the same commit as the work** (move items between sections, add
newly found bugs). The repo contains no game content: never commit extracted assets, decompiled game code or
screenshots of the game. See `README.md` for setup.

Legend: `[x]` done, `[~]` partial / approximate, `[ ]` not started.

## How the project is organised

| Path | What |
|---|---|
| `crates/gb_phys` | PhysX 4.1 binding and Unity-scene physics import (rigidbodies, colliders, joints, contacts). No Bevy. |
| `crates/gb_logic` | Beast / actor gameplay logic ported from the game (movement, grabbing, punching, damage). No Bevy. |
| `crates/gb_game` | The Bevy 0.16 app: rendering, menus, costumes, rounds, modes, stage events, water, audio. |
| `crates/gb_game/src/play/events.rs` | All per-stage scripted events (trains, trucks, sharks, cranes, elevators, cables, ...). |
| `tools/extract` | Python exporters that read **your own** copy of the game into `assets/export/` (git-ignored). |
| `tools/harness` | Screenshot / comparison helpers. |

Useful workflow: the maintainers keep a private, uncommitted Il2CppDumper dump and Ghidra decompiles of single classes
(game code is never committed). If you do the same for your own copy, port the logic from the field list and decompile,
then verify with an offscreen render (`GB_WINDOW_OFFSCREEN=1 GB_SCREENSHOT=out.png GB_SCREENSHOT_AFTER=600 target/debug/gb_game.exe <stage>`).

## Game modes (`crates/gb_game/src/round.rs`, `play.rs`)

- [x] Melee, Gang (teams), local AI players (`--bots N`, lobby `B` / `Shift+B`).
- [x] Waves: the game's 4-wave data, Big and Tiny beasts (own prefabs and AIProfiles), enemies spawn in the room behind the
  stage door and walk out, humans are pushed out of that room, results then back to the menu, best run remembered.
  - [~] Waves 3-4 use Tiny/Big enemies that the retail `WavesData` does not have (added for variety).
  - [ ] Only Rooftop / Subway / Grind / Incinerator are offered; Chute and Aquarium are test-only stages.
- [~] Soccer (Alley): ball, goals, score HUD, kick-off reset. Bots dribble toward the goal but rarely score
  (see "Bots"). Costumed beasts do not show their team colour.
- [x] Rumble was removed (it is not a mode of the retail game).
- [ ] King of the Hill, Capture the Flag, Big Fight (enum values exist in the game; not in the menu).
- [ ] Online / networking (everything is local).

## Bots (`play.rs` `bot_inputs`)

- [x] `ControlHandeler_Computer` timings, per-type `AIProfile` (Normal / Big / Tiny), ledge check, stuck -> jump.
- [ ] Real NavMesh-style routing (they walk in straight lines; Waves enemies have a special door waypoint).
- [~] Soccer: one chaser per team, the rest hold a defensive spot; a dribble assist carries the ball (bots have no kick). Bots now score a few goals per minute.

## Stage events (`play/events.rs`)

Done: subway trains, truck roads and wander, Ferris wheel, doors/shutters (`OnTriggerStayApplyForce`), falling props
(`PoolSpawner`), fans, elevators (incl. failure), cranes, buoyancy (rides the sea waves), sharks (state machine,
breach, carry, damage), trawler capsize, train landslide and scrolling track, gondola cables (damage and snap).

- [~] Train stage: left / right track sections are in (the whole track is shifted sideways so the held train stays on its
  rails). Missing: `NodeFollower` (cars steering and yawing along `TrackNode` chains), landslide start/end pieces.
- [ ] Birds on Rooftop / Crane (`BirdActor`, `BirdFSM`, `CritterEscalationManager`): the bird nodes are inactive in the
  scene, have no exported meshes and need runtime body creation.
- [ ] Per-mode object activation (`GamemodeEnabled`) beyond what is special-cased.
- [ ] Towers `SlowDestroyTowersStairwell`, billboard/girders joint-break tuning, `TrucksMoveRoadVertex` wobble.
- [~] Sharks: jaw animation not ported. Elevators / sharks / cranes are tuned by eye.

## Graphics

- [x] Lightmaps (RGBM), vinyl lightmaps, URP-style post, per-stage fog, sky domes, Gerstner sea (`water.rs`).
- [~] Per-stage colour grades for Ring, Incinerator and Subway are hand-tuned stand-ins (`main.rs`, env knobs
  `GB_RING_*`, `GB_INC_*`, `GB_SUBWAY_EV`), not matched against real captures.
- [ ] Water: depth fade, shoreline foam, refraction. Sea LOD tile seams show as thin dark lines at the horizon.
- [ ] Shadowmask / directional lightmaps / light-probe parity, SSAO and shadow softness parity.
- [ ] Particles, glass and fog cards; Incinerator fire is an emissive stand-in.
- [~] Alley: the daytime sky dome is skipped (black backdrop) so the gaps around the court are not blue; the missing front wall is still absent.
- [~] Grind: the follow camera is capped at 32 m so it stays inside the room (the authored 200 m zoom-out showed the
  unlit black exterior shell and Void boxes). Lightmapped vinyl surfaces use only the lightmap, so the shell outside the
  baked area is black; `GB_LM_DEBUG=1` prints lightmap stats.

## Characters and costumes

- [x] Costume presets, per-player colour, unseen-mesh removal, tint model, per-player costumes carried into the match.
- [~] Costume editor: preset, per-slot items (head, eyewear, face, body, back, legs) and colour; the outfit is saved as the
  "Custom" preset and carried into matches. Missing: per-item colours,
  voice / face expressions, unlock rules.
- [ ] Costume physics (dangling parts), animation clips / emotes.
- [ ] Swim, dive, backflip, double jump, headbutt, elbow (beasts in water are floated by a simple hack).

## UI / HUD / meta

- [x] Lobby (mode/stage/wins/AI/colour, remembered between runs), loading screen, round banner and coinboard.
- [ ] In-match HUD parity: name bars, timers, key prompts (needs real captures to match).
- [~] Settings: graphics and audio (master / music / effects) are wired; controls rebinding is not.
- [ ] Save / progression / unlockables.

## Audio (`crates/gb_game/src/audio.rs`)

See the "Audio" section in `README.md` for extraction. Status of each part is tracked here:

- [x] Extraction of every `AudioClip` to WAV plus sound configs and per-stage emitter / music data (`tools/extract/audio.py`).
- [x] Menu clicks, music beds (menu anthem, stage A side + ambience), looping stage machinery (`SceneAudioClip`), object impacts
  from `PhysicAudioEmitter` data, punches, footsteps by surface, body falls, grunts, swing whooshes, effort voices, win laughs, KO stingers, round banner stingers,
  shark bite / jaw / splash, cable snaps, glass breaking, water splashes.
- [ ] Verify real playback on a machine with an audio device (developed headless: the clip selection is logged with
  `GB_AUDIO_DEBUG=1`, loudness values are estimates).
- [~] Music: A/B side per round and drums when two or fewer fight are in. Missing: warp stingers, crossfades, round-flow transitions (`MusicController`), pause music.
- [ ] Positional (3D) sources for ambient loops, doppler, mixer groups and snapshots from the AudioMixer assets.
- [ ] Voices: emotes, laughs, snore loop; tentacles, trawler groans, bulb pops, train / crane / elevator / wheel loops,
  countdown voice (`AudioDatabase`), `SoundOnTriggerEnter` / `PlaySoundOnJointBreak` stage sounds.
- [ ] Convert the WAVs to Ogg to shrink the 2.6 GB extraction.

## Physics (`crates/gb_phys`)

- [x] Scene settings, collision matrix, joints (Unity basis fix), CCD modes, kinematic movers, joint release.
- [ ] Unity's 0.04 s maximum timestep clamp is not applied (it would slow debug builds down).

## Known bugs

- Subway is still darker than the retail game after the +1 EV stand-in.
- Alley black screen-edge patches; Alley exports with missing meshes (the menu-content bundle is required).
- Bots in Soccer rarely score; the ball is reset after being wedged for 8 s.
- Debug builds are slow on integrated GPUs; use `cargo run --release` for play.

## Debug knobs (environment variables)

`GB_EVENT_TIME_SCALE` (speed up stage events), `GB_DRIFT_REPORT` / `GB_DRIFT_FILTER`, `GB_PROP_DEBUG`,
`GB_WAVES_FIRST=N`, `GB_WAVES_KILL`, `GB_WAVES_LOSE`, `GB_WAVES_TYPE=1|2`, `GB_WAVE_TRACE`, `GB_WAVE_TEST_ENTER`,
`GB_SOCCER_TEST`, `GB_SOCCER_TRACE`, `GB_RELOAD_TEST=<step>`, `GB_ROAD_DEBUG`, `GB_NO_WATER_WAVES`,
`GB_WAVE_HEIGHT` / `GB_WAVE_BUMP` / `GB_WAVE_FOAM` / `GB_WAVE_TIME`, `GB_NO_RETURN_TO_MENU` (tests). F2 opens the
in-game knob panel.

## Contributing checklist

1. `cargo build -j 4` with **zero warnings**, `cargo test` green.
2. Verify the change in the real build (offscreen render or play it).
3. Update this file, commit, push. Small commits, one topic each.
