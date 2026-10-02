//! Physics-driven sound effects: object impacts from the stage's `PhysicAudioEmitter` data (soft / hard thresholds and
//! clip lists from `PhysicsAudioData`), beast footsteps, punches, body falls and grunts. Clip names come from the
//! player's extracted audio (`tools/extract/audio.py`); with no audio extracted this module does nothing.
use super::*;
use crate::audio::{AudioLib, Sfx};
use std::collections::HashMap;

/// Voice banks (`GB VO <bank> <emotion>`); each actor gets one.
const VOICES: [&str; 12] = ["BOO", "DEE", "DOF", "GOO", "HUG", "OLO", "OOK", "TUM", "UYU", "WUB", "YEE", "YUP"];
const GRUNTS: [&str; 5] = ["UU", "DOH", "VERT", "NIE", "MUNGY"];

struct Emitter {
    name: String,
    soft: Vec<String>,
    hard: Vec<String>,
    soft_threshold: f32,
    hard_threshold: f32,
    soft_volume: f32,
    hard_volume: f32,
    soft_pitch: (f32, f32),
    hard_pitch: (f32, f32),
    cooldown: f32,
    last: f32,
}

#[derive(Default)]
pub struct SoundState {
    built: bool,
    emitters: HashMap<usize, Emitter>,
    parents: Vec<Option<usize>>,
    /// node -> clips of its `PlaySoundOnJointBreak` GeneralAudioData.
    break_sounds: HashMap<usize, Vec<String>>,
    rng: u32,
    last_foot: HashMap<usize, f32>,
    last_voice: HashMap<usize, f32>,
    last_punch: HashMap<usize, f32>,
    alive_prev: Vec<bool>,
    hands_prev: Vec<[(bool, bool); 2]>,
    message_prev: String,
}

impl SoundState {
    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223).max(1);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn pick<'a>(&mut self, list: &'a [String]) -> Option<&'a String> {
        if list.is_empty() {
            None
        } else {
            let i = (self.rand() * list.len() as f32) as usize % list.len();
            list.get(i)
        }
    }

    fn pick_family(&mut self, lib: &AudioLib, family: &str) -> Option<String> {
        let list = lib.family(family);
        self.pick(list).cloned()
    }
}

fn pitch(range: (f32, f32), r: f32) -> f32 {
    range.0 + (range.1 - range.0) * r
}

fn build(sim: &Sim, st: &mut SoundState, lib: &AudioLib) {
    st.built = true;
    st.rng = 0x1234_5678 ^ sim.stage_name.len() as u32;
    let src = &sim.scenes[0].1;
    st.parents = src.nodes.iter().map(|n| n.parent).collect();
    let stage_audio: serde_json::Value = std::fs::read(lib.root.join(format!("audio-{}.json", sim.stage_name)))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null);
    let data = &lib.config["PhysicsAudioData"];
    for (i, n) in src.nodes.iter().enumerate() {
        let Some(c) = n.components.iter().find(|c| c.script.as_deref() == Some("PhysicAudioEmitter")) else { continue };
        let go = c.data["m_GameObject"]["m_PathID"].as_i64().unwrap_or(0);
        let Some(name) = stage_audio["emitters"][go.to_string()].as_str() else { continue };
        let d = &data[name];
        if d.is_null() {
            continue;
        }
        let list = |k: &str| -> Vec<String> {
            d["_ptrs"][k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let f = |k: &str, def: f32| d[k].as_f64().map_or(def, |v| v as f32);
        st.emitters.insert(
            i,
            Emitter {
                name: name.to_string(),
                soft: list("softDirectImpactSound"),
                hard: list("hardDirectImpactSound"),
                soft_threshold: f("softTriggerThreshold", 2.0),
                hard_threshold: f("hardTriggerThreshold", 6.0),
                soft_volume: f("softImpactVolumeModifier", 0.3),
                hard_volume: f("hardImpactVolumeModifier", 0.6),
                soft_pitch: (f("minSoftImpactPitchModifier", 0.9), f("maxSoftImpactPitchModifier", 1.1)),
                hard_pitch: (f("minHardImpactPitchModifier", 0.9), f("maxHardImpactPitchModifier", 1.1)),
                cooldown: f("enableCooldownDelay", 0.2),
                last: -10.0,
            },
        );
    }
    // PlaySoundOnJointBreak: GeneralAudioData clips played where a scene joint of that object breaks.
    let mut go_node: HashMap<i64, usize> = HashMap::new();
    for (i, n) in src.nodes.iter().enumerate() {
        if let Some(go) = n.components.first().and_then(|c| c.data["m_GameObject"]["m_PathID"].as_i64()) {
            go_node.entry(go).or_insert(i);
        }
    }
    for c in stage_audio["clips"].as_array().into_iter().flatten() {
        if c["class"].as_str() != Some("PlaySoundOnJointBreak") {
            continue;
        }
        let (Some(go), Some(name)) = (c["go"].as_i64(), c["ptrs"]["generalAudioData"].as_str()) else { continue };
        let Some(&node) = go_node.get(&go) else { continue };
        let clips: Vec<String> = lib.config["GeneralAudioData"][name]["_ptrs"]["clips"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !clips.is_empty() {
            st.break_sounds.insert(node, clips);
        }
    }
    info!("audio: {} impact emitters, {} joint-break sounds on {}", st.emitters.len(), st.break_sounds.len(), sim.stage_name);
}

fn emitter_key(st: &SoundState, mut node: usize) -> Option<usize> {
    for _ in 0..4 {
        if st.emitters.contains_key(&node) {
            return Some(node);
        }
        node = st.parents.get(node).copied().flatten()?;
    }
    None
}

/// Footstep family for the surface an emitter name describes.
fn footstep_family(surface: &str) -> &'static str {
    let s = surface.to_ascii_lowercase();
    if s.contains("wood") {
        "GB SFX FootWOOD"
    } else if s.contains("metal") || s.contains("grate") || s.contains("railing") || s.contains("trawler") {
        "GB SFX FOOTmetalNEW"
    } else if s.contains("ice") {
        "GB SFX FootICE"
    } else if s.contains("dirt") {
        "GB SFX FootDIRT"
    } else if s.contains("wet") {
        "GB FootSteps WET"
    } else if s.contains("ring") || s.contains("bouncy") {
        "GB SFX FootBOUNCY"
    } else {
        "GB SFX FEET SOFTY"
    }
}

/// Call once per physics step with that step's contacts.
pub fn contact_sounds(sim: &mut Sim, st: &mut SoundState, sfx: &mut Sfx, lib: &AudioLib) {
    if std::env::var_os("GB_AUDIO_DEBUG").is_some() && sim.world.steps % 200 == 0 {
        info!("audio: step {} contacts {} enabled {}", sim.world.steps, sim.world.contacts.len(), sfx.enabled);
    }
    // Joints that just broke: stage objects with PlaySoundOnJointBreak speak up.
    let broken = sim.world.new_broken_joints();
    if !sfx.enabled {
        return;
    }
    if !st.built && (!sim.world.contacts.is_empty() || !broken.is_empty()) {
        build(sim, st, lib);
    }
    for (own, _) in broken {
        let Some(body) = own else { continue };
        let Some(node) = sim.world.actors.iter().find(|a| a.body == Some(body)).map(|a| a.node) else { continue };
        let found = {
            let mut n = node;
            let mut found = None;
            for _ in 0..4 {
                if st.break_sounds.contains_key(&n) {
                    found = Some(n);
                    break;
                }
                match st.parents.get(n).copied().flatten() {
                    Some(p) => n = p,
                    None => break,
                }
            }
            found
        };
        let Some(key) = found else { continue };
        let list = st.break_sounds[&key].clone();
        if let Some(clip) = st.pick(&list).cloned() {
            sfx.play_at(&clip, 0.9, 1.0, mirror_position(sim.world.pose(body).position));
        }
    }
    if sim.world.contacts.is_empty() {
        return;
    }
    let now = sim.world.steps as f32 * sim.world.settings.fixed_timestep;
    // body -> (actor, part)
    let mut beast: HashMap<usize, (usize, Part)> = HashMap::new();
    for (k, a) in sim.actors.iter().enumerate() {
        for p in Part::ALL {
            beast.insert(a.beast.body(p), (k, p));
        }
    }
    let inst0 = sim.scenes[0].0;
    for c in &sim.world.contacts {
        if c.kind != gb_phys::ContactKind::Enter {
            continue;
        }
        let speed = c.relative_velocity.length();
        let at = c.points.first().map_or(Vec3::ZERO, |p| mirror_position(p.position));
        let this = sim.world.actors[c.this];
        let other = sim.world.actors[c.other];
        let this_beast = this.body.and_then(|b| beast.get(&b)).copied();
        let other_beast = other.body.and_then(|b| beast.get(&b)).copied();
        match (this_beast, other_beast) {
            (Some((a, part)), Some((b, hit))) if a != b => {
                // Punch / kick landing on another beast.
                let striking = matches!(part, Part::LeftHand | Part::RightHand | Part::LeftForarm | Part::RightForarm | Part::LeftFoot | Part::RightFoot);
                if striking && speed > 3.0 && now - st.last_punch.get(&a).copied().unwrap_or(-9.0) > 0.12 {
                    st.last_punch.insert(a, now);
                    let area = match hit {
                        Part::Head => "FACE",
                        Part::Chest | Part::Stomach | Part::Waist | Part::Hips | Part::Crotch => "BODY",
                        _ => "LIMB",
                    };
                    let weight = if speed > 9.0 { "HEAVY" } else { "LIGHT" };
                    if let Some(clip) = st.pick_family(lib, &format!("GB PUNCH SFX XTRA {area} {weight}")) {
                        let r = st.rand();
                        sfx.play_at(&clip, (0.3 + speed * 0.03).min(0.8), 0.92 + 0.16 * r, at);
                    }
                    // The one that got hit grunts now and then.
                    if speed > 6.0 && now - st.last_voice.get(&b).copied().unwrap_or(-9.0) > 1.4 {
                        st.last_voice.insert(b, now);
                        let emo = GRUNTS[(st.rand() * GRUNTS.len() as f32) as usize % GRUNTS.len()];
                        let bank = VOICES[(b * 5 + 3) % VOICES.len()];
                        if let Some(clip) = st.pick_family(lib, &format!("GB VO {bank} {emo}")) {
                            sfx.play_at(&clip, 0.55, 1.0, at);
                        }
                    }
                }
            }
            (Some((a, part)), None) if other.instance == inst0 || other.body.is_some() => {
                let surface = emitter_key(st, other.node).and_then(|k| st.emitters.get(&k)).map(|e| e.name.clone()).unwrap_or_default();
                let foot = matches!(part, Part::LeftFoot | Part::RightFoot | Part::LeftLeg | Part::RightLeg);
                if foot && speed > 1.1 && now - st.last_foot.get(&a).copied().unwrap_or(-9.0) > 0.16 {
                    st.last_foot.insert(a, now);
                    if let Some(clip) = st.pick_family(lib, footstep_family(&surface)) {
                        let r = st.rand();
                        sfx.play_at(&clip, (0.12 + speed * 0.03).min(0.45), 0.9 + 0.2 * r, at);
                    }
                } else if !foot && matches!(part, Part::Head | Part::Chest | Part::Hips | Part::Stomach | Part::Waist) && speed > 5.5 {
                    let fam = if speed > 9.0 { "GB SFX BODYhitGROUND" } else { "GB SFX LIMB HIT HARD" };
                    if let Some(clip) = st.pick_family(lib, fam) {
                        sfx.play_at(&clip, (0.3 + speed * 0.04).min(1.0), 1.0, at);
                    }
                }
            }
            (None, _) => {
                // A stage object struck by anything: its PhysicsAudioData decides.
                let Some(key) = emitter_key(st, this.node).filter(|_| this.instance == inst0) else { continue };
                let (r0, r1) = (st.rand(), st.rand());
                let Some(e) = st.emitters.get_mut(&key) else { continue };
                if speed < e.soft_threshold || now - e.last < e.cooldown {
                    continue;
                }
                e.last = now;
                let hard = speed >= e.hard_threshold;
                let (list, vol, range) = if hard && !e.hard.is_empty() {
                    (&e.hard, e.hard_volume, e.hard_pitch)
                } else {
                    (&e.soft, e.soft_volume, e.soft_pitch)
                };
                if list.is_empty() {
                    continue;
                }
                let clip = list[(r0 * list.len() as f32) as usize % list.len()].clone();
                let volume = (vol * 2.2 * (speed / e.hard_threshold.max(1.0)).clamp(0.35, 1.5)).min(1.2);
                sfx.play_at(&clip, volume, pitch(range, r1), at);
            }
            _ => {}
        }
    }
}

/// KO sounds when a beast goes down, and the round banner stingers. Call every physics step.
pub fn round_sounds(sim: &mut Sim, st: &mut SoundState, sfx: &mut Sfx, lib: &AudioLib) {
    // Sounds requested by game logic (cable snaps, shark bites, splashes).
    for (family, volume, pos) in std::mem::take(&mut sim.sound_queue) {
        if !sfx.enabled {
            continue;
        }
        let clip = if lib.files.contains_key(&family) { Some(family) } else { st.pick_family(lib, &family) };
        if let Some(clip) = clip {
            match pos {
                Some(p) => sfx.play_at(&clip, volume, 1.0, mirror_position(p)),
                None => sfx.play(&clip, volume, 1.0),
            }
        }
    }
    // Glass panes that just broke.
    let broken = sim.fractures.glass.iter().filter(|g| g.broken).count();
    if broken > sim.glass_broken_prev {
        if let Some(clip) = st.pick_family(lib, "GB GLASS BREAK SFX") {
            sfx.play(&clip, 0.9, 1.0);
        }
    }
    sim.glass_broken_prev = broken;
    if !sfx.enabled || sim.lobby {
        return;
    }
    let alive: Vec<bool> = sim.actors.iter().enumerate().map(|(k, a)| crate::round::alive(a.state) && !sim.parked.get(k).copied().unwrap_or(false)).collect();
    if st.alive_prev.len() == alive.len() {
        for k in 0..alive.len() {
            if st.alive_prev[k] && !alive[k] && !sim.parked.get(k).copied().unwrap_or(false) {
                if let Some(clip) = st.pick_family(lib, "GB_SFX_RingOut") {
                    sfx.play(&clip, 0.6, 1.0);
                }
                let bank = VOICES[(k * 5 + 3) % VOICES.len()];
                if let Some(clip) = st.pick_family(lib, &format!("GB VO {bank} NIE")) {
                    sfx.play(&clip, 0.5, 1.0);
                }
            }
        }
    }
    st.alive_prev = alive;
    // Swings and grabs: whoosh + effort voice on the rising edge.
    let hands: Vec<[(bool, bool); 2]> = sim.actors.iter().map(|a| [(a.control.hands[0].punch, a.control.hands[0].joint.is_some()), (a.control.hands[1].punch, a.control.hands[1].joint.is_some())]).collect();
    if st.hands_prev.len() == hands.len() {
        for k in 0..hands.len() {
            if sim.parked.get(k).copied().unwrap_or(false) {
                continue;
            }
            for side in 0..2 {
                let (punch, grab) = hands[k][side];
                let (pp, pg) = st.hands_prev[k][side];
                let at = mirror_position(sim.world.pose(sim.actors[k].beast.body(Part::Hips)).position);
                let bank = VOICES[(k * 5 + 3) % VOICES.len()];
                if punch && !pp {
                    if let Some(clip) = st.pick_family(lib, "GB SFX SWING") {
                        sfx.play_at(&clip, 0.5, 1.0, at);
                    }
                    if st.rand() < 0.3 && now_voice_ok(st, k, sim) {
                        if let Some(clip) = st.pick_family(lib, &format!("GB VO {bank} DA")) {
                            sfx.play_at(&clip, 0.45, 1.0, at);
                        }
                    }
                }
                if grab && !pg && now_voice_ok(st, k, sim) {
                    if let Some(clip) = st.pick_family(lib, &format!("GB VO {bank} MEH")) {
                        sfx.play_at(&clip, 0.45, 1.0, at);
                    }
                }
            }
        }
    }
    st.hands_prev = hands;
    let text = sim.round.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    if text != st.message_prev {
        if !text.is_empty() {
            let lower = text.to_ascii_lowercase();
            if lower.contains("wins") || lower.contains("defeated") || lower.contains("scores") {
                sfx.play("GB SFX WIN STAR 1 ALL", 0.7, 1.0);
                if let Some(k) = alive_first(sim) {
                    let bank = VOICES[(k * 5 + 3) % VOICES.len()];
                    if let Some(clip) = st.pick_family(lib, &format!("GB VO {bank} WIN LAUGH")) {
                        sfx.play(&clip, 0.6, 1.0);
                    }
                }
            } else if lower.starts_with("wave") || lower.contains("challenger") || lower.contains("round") {
                if let Some(clip) = st.pick_family(lib, "GB SFX CountDown a") {
                    sfx.play(&clip, 0.6, 1.0);
                }
            }
        }
        st.message_prev = text;
    }
}

/// Rate-limits an actor's voice lines to one per 0.8 s of simulated time.
fn now_voice_ok(st: &mut SoundState, k: usize, sim: &Sim) -> bool {
    let now = sim.world.steps as f32 * sim.world.settings.fixed_timestep;
    if now - st.last_voice.get(&k).copied().unwrap_or(-9.0) > 0.8 {
        st.last_voice.insert(k, now);
        true
    } else {
        false
    }
}

fn alive_first(sim: &Sim) -> Option<usize> {
    (0..sim.actors.len()).find(|&k| crate::round::alive(sim.actors[k].state) && !sim.parked.get(k).copied().unwrap_or(false))
}
