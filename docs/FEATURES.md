# Feature reference

A detailed description of what the engine implements, how it works, and what is approximate. For the short to-do list see
[`STATUS_AND_TODO.md`](STATUS_AND_TODO.md). Everything here reads data from the player's own copy of the game; no game
content is in this repository.

Legend: **Done** = matches the game's logic as far as we can tell, **Approx** = works but simplified or tuned by eye,
**Missing** = not implemented.

## 1. Physics and beasts

| Feature | State | Notes |
|---|---|---|
| PhysX 4.1 scene settings from the game | Done | gravity, 0.02 s step, solver iterations, friction/PCM/ABP, sleep and depenetration limits, 32x32 layer matrix |
| Scene import | Done | rigidbodies, box/sphere/capsule/convex/mesh colliders, ConfigurableJoints (Unity basis fix), CCD modes, kinematic movers |
| Beast ragdoll | Done | `gb_logic`: stand / walk / run / jump / fall / climb states, grab (left/right, lift), punch, kick, duck/headbutt, damage and knockout |
| Beast variants | Done | Normal, **Big** (1.2 scale, heavier) and **Tiny** prefabs, each with their own sidecar and AI profile |
| Swimming / diving / backflip / double jump | Missing | beasts in water are floated by a spring-and-drag helper |
| Unity's 0.04 s max timestep clamp | Missing | would make slow debug builds run in slow motion |

## 2. Modes and rounds

- **Melee** (free for all), **Gang** (two balanced teams, shared colour): round flow Playing -> Settle -> Winner zoom ->
  End of round (coinboard with balloons) -> Loading, wins to win configurable (1-10).
- **Waves**: the game's `WavesData` (four waves: 1 firefighter, 2 riot police incl. a Big one, then fallback costumes).
  Waves 3 and 4 add Tiny and Big enemies (not in the retail data). Local players are red. Enemies spawn in the room behind
  the stage door and walk out; humans are pushed out of the room; losing (or clearing wave 4) shows the result and returns
  to the menu; the best run is remembered. Offered on Rooftop, Subway, Grind and Incinerator only.
- **Soccer** (Alley): ball, goals, score HUD, kick-off reset, wedged-ball reset. Bots chase the ball but rarely score.
- Removed on purpose: Rumble (not a retail mode). Missing: King of the Hill, Capture the Flag, Big Fight, online.

## 3. Bots

- Port of `ControlHandeler_Computer` timings (wind-up / punch / reset scaled by the `AIProfile` punch delay), per-type
  profiles (Normal / Big / Tiny: speed, punch force, wind-up), ledge check, stuck -> jump, periodic lift.
- No navigation mesh: bots walk in straight lines (Waves enemies get a door waypoint).

## 4. Stages and events

20 stages load and run. Events (all in `crates/gb_game/src/play/events.rs`):

| Stage | Events |
|---|---|
| Subway | trains spawn, push beasts and despawn |
| Trucks | scrolling road tiles, trucks wander inside bounds, wheels spin, beasts start on the truck roofs |
| Wheel | Ferris wheel rotation, burger cars on joints |
| Chute / Incinerator / Grind | doors and shutters (`OnTriggerStayApplyForce`), falling props from pools |
| Vents | fan cycle that blows beasts up |
| Elevators | shuttling cars and a cable-snap malfunction |
| Crane / Containers | driven cranes slide along their gantry, container colours |
| Buoy / Lighthouse / Trawler / Containers / Crane / Wheel | Gerstner sea waves, buoyant ice / buoys / hulls that ride them |
| Buoy / Trawler | sharks: sleep, search, attack, breach, carry, dive, retreat; they take damage and get knocked out |
| Trawler | capsize after the sink delay |
| Train | scrolling track with left / right jogs (track shifted under the held train), landslide boulders; cars do not steer or yaw |
| Gondola | cables take collision damage (game formula) and snap |
| Rooftop / Crane | birds are **missing** (inactive pooled critters, no exported meshes) |

## 5. Graphics

- URP-style post (exposure, contrast, saturation, tonemap), RGBM lightmaps, baked-light skipping, per-stage fog (exact
  surface-fog pass or a crude distance fog on a few stages), sky domes, reflection probes.
- Water: `water.rs` + `shaders/water.wgsl` (Gerstner vertical swell with distance fade, two scrolling wave normal maps, crest
  foam). Missing: depth fade, shoreline foam, refraction.
- Hand-tuned stage grades: Ring (no desaturation, -1 EV), Incinerator, Subway (+1 EV). Experimental alternate engine
  (`GB_ENGINE=1`) is off by default.

## 6. Costumes and colours

- Presets from the game's costume database, tinted with the game's palette shades; the player's costume is carried from the
  lobby into the match; enemies in Waves wear the wave costumes.
- Costume editor: a real display beast with rows for the preset, the six item slots (Head, Eyewear, Face, Body, Back,
  Legs; items hide the slots they `disable`), and colour. The outfit is saved as the `Custom` preset in the lobby prefs
  file and used in matches. Missing: per-item colours, voices, face expressions, unlock rules.

## 7. Menus and UI

- Menu scene from the game (camera glides between screens), lobby with join / ready, mode / wins / stage rows (a Random stage option, Wins hidden
  for Waves, stages filtered by mode), AI count, colour and costume keys, remembered choices
  (`%APPDATA%/gb-rust/lobby.json`), loading splash, pause screen (resume / volume / menu / quit).
- Settings: graphics rows, **audio sliders** (master / music / effects), controls help.
- Missing: controller rebinding, in-match name bars and key prompts matched to the retail HUD.

## 8. Audio

- Extraction: `tools/extract/audio.py` (clips -> WAV, configs, per-stage emitters and music, loudness).
- Engine (`audio.rs`, `play/sounds.rs`): on-demand clip loading with voice limits and distance falloff; loudness-matched
  mixing with master / music / effects gains; menu clicks; music beds (menu anthem, stage A / B side, drums when two or
  fewer fight, ambience, looping machinery); object impacts from `PhysicAudioEmitter` thresholds and clip lists; punches by
  body area; footsteps by surface; body falls; grunts, effort voices, win laughs; KO and round stingers; shark, cable,
  glass and splash sounds.
- Stage trigger sounds (`SoundOnTriggerEnter`) play when a beast enters the volume. Missing: 3D positioned loops, mixer snapshots / groups, emotes, countdown
  voice, Ogg conversion to shrink the 2.6 GB extraction. Loudness values are estimates, tune with the sliders.

## 9. Tools and debugging

- `tools/extract`: `export.py` (stages, prefabs, costumes, UI, settings), `audio.py`, `water_params.py`,
  `extract_all.bat`.
- Environment knobs: see the table in `STATUS_AND_TODO.md`; F2 opens the in-game knob panel.

## 10. Known bugs

- Subway is still darker than the retail game; Ring / Incinerator colours are approximations.
- Alley has black patches at the screen edges.
- Soccer bots wedge the ball into walls; the ball resets after 8 s.
- Sea LOD tiles show thin dark seams at the horizon.
- The costume editor beast is a placeholder for the real per-part editor.
