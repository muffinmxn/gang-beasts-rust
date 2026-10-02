//! Sound: clips extracted from the player's own copy of the game (`tools/extract/audio.py` -> `assets/export/audio/*.wav`,
//! `audio-index.json`, `audio-config.json`, `audio-<stage>.json`). Game code pushes named clips onto [`Sfx`]; [`drain`]
//! turns them into Bevy audio entities. Nothing is loaded until it is first played.
use bevy::{audio::Volume, prelude::*};
use std::{collections::HashMap, path::Path};

/// One queued sound.
pub struct SfxCall {
    pub clip: String,
    pub volume: f32,
    pub speed: f32,
    pub pos: Option<Vec3>,
}

#[derive(Resource, Default)]
pub struct Sfx {
    queue: Vec<SfxCall>,
    /// Last play time per clip family, to stop a pile-up of identical sounds in one frame.
    last: HashMap<String, f32>,
    /// Where the listener (camera) is, for distance attenuation of positional sounds.
    pub listener: Vec3,
    pub master: f32,
    pub enabled: bool,
}

impl Sfx {
    pub fn play(&mut self, clip: &str, volume: f32, speed: f32) {
        self.push(clip, volume, speed, None);
    }

    pub fn play_at(&mut self, clip: &str, volume: f32, speed: f32, pos: Vec3) {
        self.push(clip, volume, speed, Some(pos));
    }

    fn push(&mut self, clip: &str, volume: f32, speed: f32, pos: Option<Vec3>) {
        if self.enabled && self.queue.len() < 64 {
            self.queue.push(SfxCall { clip: clip.to_string(), volume, speed, pos });
        }
    }
}

/// Clip name -> wav file name inside `audio/`.
#[derive(Resource, Default)]
pub struct AudioLib {
    pub files: HashMap<String, String>,
    /// Parsed `audio-config.json` (UI sounds, music, physics audio data), or `Null`.
    pub config: serde_json::Value,
    pub root: std::path::PathBuf,
    families: HashMap<String, Vec<String>>,
}

/// "GB SFX FEET SOFTY5b" -> "GB SFX FEET SOFTY": the numbered variants of one sound.
pub fn family_key(name: &str) -> String {
    let mut s = name.trim_end();
    // A trailing variant letter after the number ("...5b").
    let mut rev = s.chars().rev();
    if let (Some(last), Some(prev)) = (rev.next(), rev.next()) {
        if last.is_ascii_lowercase() && prev.is_ascii_digit() {
            s = &s[..s.len() - 1];
        }
    }
    s.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end().to_string()
}

impl AudioLib {
    /// All clips of a family (see [`family_key`]); empty when unknown.
    pub fn family(&self, key: &str) -> &[String] {
        self.families.get(key).map_or(&[], |v| v.as_slice())
    }

}

#[derive(Component)]
struct SfxVoice;

#[derive(Component)]
pub struct MusicVoice;

pub fn plugin(app: &mut App, root: &Path) {
    let index: HashMap<String, serde_json::Value> = std::fs::read(root.join("audio-index.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let enabled = !index.is_empty() && std::env::var_os("GB_NO_AUDIO").is_none();
    let files = index
        .iter()
        .filter_map(|(name, v)| v["file"].as_str().map(|f| (name.clone(), f.to_string())))
        .collect();
    let config = std::fs::read(root.join("audio-config.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null);
    if !enabled {
        info!("audio: no extracted clips (run tools/extract/audio.py); running silent");
    }
    info!("audio: {} clips indexed", index.len());
    let master = std::env::var("GB_VOLUME").ok().and_then(|v| v.parse().ok()).unwrap_or(0.8);
    let mut families: HashMap<String, Vec<String>> = HashMap::new();
    for name in index.keys() {
        families.entry(family_key(name)).or_default().push(name.clone());
    }
    for v in families.values_mut() {
        v.sort();
    }
    let files: HashMap<String, String> = files;
    app.insert_resource(AudioLib { files, config, root: root.to_path_buf(), families })
        .insert_resource(Sfx { master, enabled, ..Default::default() })
        .add_systems(Update, (track_listener, menu_sounds, music, drain).chain());
}

fn track_listener(cameras: Query<&GlobalTransform, With<Camera3d>>, mut sfx: ResMut<Sfx>) {
    if let Some(t) = cameras.iter().next() {
        sfx.listener = t.translation();
    }
}

fn drain(
    mut commands: Commands,
    mut sfx: ResMut<Sfx>,
    lib: Res<AudioLib>,
    assets: Res<AssetServer>,
    time: Res<Time>,
    voices: Query<(), With<SfxVoice>>,
) {
    let calls = std::mem::take(&mut sfx.queue);
    let mut live = voices.iter().count();
    let now = time.elapsed_secs();
    for c in calls {
        let Some(file) = lib.files.get(&c.clip) else { continue };
        if live >= 40 {
            break;
        }
        // The same clip at most every 40 ms.
        if sfx.last.get(&c.clip).is_some_and(|t| now - t < 0.04) {
            continue;
        }
        sfx.last.insert(c.clip.clone(), now);
        let mut volume = c.volume * sfx.master;
        if let Some(p) = c.pos {
            volume /= 1.0 + (p - sfx.listener).length() / 18.0;
        }
        if volume < 0.01 {
            continue;
        }
        if std::env::var_os("GB_AUDIO_DEBUG").is_some() {
            info!("sfx: {} vol {:.2} speed {:.2}", c.clip, volume, c.speed);
        }
        commands.spawn((
            SfxVoice,
            AudioPlayer::<AudioSource>(assets.load(format!("audio/{file}"))),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume.min(2.0))).with_speed(c.speed.clamp(0.25, 4.0)),
        ));
        live += 1;
    }
}

/// Menu navigation clicks (`AudioConfig.uiDefaultConfig`): highlight on up/down, change on left/right, click on confirm.
fn menu_sounds(keys: Res<ButtonInput<KeyCode>>, menu: Option<Res<crate::menu::Menu>>, lib: Res<AudioLib>, mut sfx: ResMut<Sfx>) {
    if menu.is_none() {
        return;
    }
    let ui = &lib.config["AudioConfig"]["AudioConfig"]["_ptrs"]["uiDefaultConfig"];
    let vol = |k: &str| lib.config["AudioConfig"]["AudioConfig"][ "uiDefaultConfig"][k].as_f64().unwrap_or(0.2) as f32 * 3.0;
    let name = |k: &str| ui[k].as_str().map(str::to_string);
    if keys.any_just_pressed([KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::KeyW, KeyCode::KeyS]) {
        if let Some(c) = name("hightlightSFX") {
            sfx.play(&c, vol("highlightSFXVolume"), 1.0);
        }
    }
    if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::KeyA, KeyCode::KeyD, KeyCode::KeyQ, KeyCode::KeyE, KeyCode::KeyZ, KeyCode::KeyX, KeyCode::KeyB]) {
        if let Some(c) = name("changeValSFX") {
            sfx.play(&c, vol("changeValSFXVolume"), 1.0);
        }
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::Space]) {
        if let Some(c) = name("clickSFX") {
            sfx.play(&c, vol("clickSFXVolume"), 1.0);
        }
    }
}

/// Music beds: the menu's day anthem, or a stage's A side plus its ambience (`SceneAudioConfig`), and the stage's looping
/// `SceneAudioClip` machinery (one voice per distinct loop).
fn music(
    mut commands: Commands,
    lib: Res<AudioLib>,
    assets: Res<AssetServer>,
    sim: Option<NonSend<crate::play::Sim>>,
    menu: Option<Res<crate::menu::Menu>>,
    existing: Query<Entity, With<MusicVoice>>,
    mut current: Local<String>,
    sfx: Res<Sfx>,
) {
    if !sfx.enabled {
        return;
    }
    // (clip, volume)
    let mut layers: Vec<(String, f32)> = Vec::new();
    let key;
    if menu.is_some() {
        key = "menu".to_string();
        layers.push(("GB Days Anthem".into(), 0.15));
    } else if let Some(sim) = sim {
        // Even rounds play the A side, odd rounds the B side; the drums join when the fight narrows to two.
        let rounds: u32 = sim.round.wins.iter().sum();
        let alive = sim.actors.iter().enumerate().filter(|(k, a)| crate::round::alive(a.state) && !sim.parked.get(*k).copied().unwrap_or(false)).count();
        let b_side = rounds % 2 == 1;
        let drums = alive <= 2 && sim.actors.len() > 2;
        key = format!("{}:{}:{}", sim.stage_name, b_side, drums);
        let stage = sim.stage_name.clone();
        if let Ok(b) = std::fs::read(lib.root.join(format!("audio-{stage}.json"))) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                if let Some(music) = v["music"].as_object().and_then(|m| m.values().next()) {
                    if let Some(c) = music[if b_side { "bSide" } else { "aSide" }].as_str().or(music["aSide"].as_str()) {
                        layers.push((c.to_string(), 0.18));
                    }
                    if let (true, Some(c)) = (drums, music["drums"].as_str()) {
                        layers.push((c.to_string(), 0.16));
                    }
                    if let Some(c) = music["ambience"].as_str() {
                        layers.push((c.to_string(), 0.3));
                    }
                }
                let mut seen = std::collections::HashSet::new();
                for c in v["clips"].as_array().into_iter().flatten() {
                    let (Some(clip), true) = (c["ptrs"]["clip"].as_str(), c["loop"].as_i64() == Some(1)) else { continue };
                    if seen.insert(clip.to_string()) {
                        let vol = c["volume"].as_f64().unwrap_or(0.3) as f32;
                        layers.push((clip.to_string(), (vol * 0.5).clamp(0.02, 0.35)));
                    }
                }
            }
        }
    } else {
        return;
    }
    if *current == key {
        return;
    }
    *current = key;
    for e in &existing {
        commands.entity(e).despawn();
    }
    for (clip, volume) in layers {
        if let Some(file) = lib.files.get(&clip) {
            commands.spawn((
                MusicVoice,
                AudioPlayer::<AudioSource>(assets.load(format!("audio/{file}"))),
                PlaybackSettings::LOOP.with_volume(Volume::Linear(volume * sfx.master / 0.8)),
            ));
        }
    }
}
