//! Costumes (ActorCostume + RigCombine): each preset is a list of costume items; every item is a
//! prefab exported to assets/export/costumes/<uid>.glb. Its skinned meshes are rebound onto the
//! beast's bones by name (RigCombine.AttachBones), and anything hanging off a bone the beast also
//! has - rigid hats, extra bones like capes - is re-parented under that beast bone
//! (RigCombine.ParentRequiredBones / AttachBonelessMeshRenderer). Costume physics (dangly
//! rigidbodies + ReAttachJoints) is not simulated yet; those bones follow rigidly.
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::render::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::scene::SceneInstance;
use serde::Deserialize;

use crate::play::{NodeEntities, Sim};

#[derive(Deserialize)]
struct Index {
    presets: Vec<Preset>,
    items: HashMap<String, Item>,
}

#[derive(Deserialize, Clone)]
pub struct Preset {
    pub name: String,
    #[allow(dead_code)]
    pub unlocked: bool,
    /// (item uid, colour id)
    pub items: Vec<(u16, u32)>,
}

#[derive(Deserialize)]
struct Item {
    slot: u32,
    /// Slots this item hides (a full-body kigurumi hides the hat and the legs).
    #[serde(default)]
    disable: Vec<u32>,
    /// Prefab file name, e.g. `Construction_hardHat.prefab`.
    #[serde(default)]
    name: String,
}

#[derive(Resource)]
pub struct Costumes {
    pub presets: Vec<Preset>,
    items: HashMap<u16, Item>,
    /// Actor -> preset applied (so each beast is dressed once).
    applied: HashMap<usize, usize>,
    /// Preset chosen in the menu for local players (in-process launch); falls back to GB_COSTUME.
    choice: Option<String>,
    /// Lobby: preset chosen for each joined actor (the keyboard player is rarely actor 0 once
    /// someone backed out, so one global choice never reached their beast).
    actor_choices: HashMap<usize, String>,
    /// Set by the lobby's left/right costume switch: existing pieces are despawned and every
    /// actor is re-dressed with the new preset.
    pub redress: bool,
}

/// Asset root, for reading a costume item's sidecar (`CostumeSettings` / `KeepIn`/`KeepOut`).
#[derive(Resource)]
pub struct CostumeRoot(pub std::path::PathBuf);

/// Editor slots (`CostumeSlot`): 1 head (hats, hair), 2 eyewear, 3 face (beards, masks), 4 body, 5 back / accessories, 6 legs.
pub const SLOT_NAMES: [&str; 6] = ["Head", "Eyewear", "Face", "Body", "Back", "Legs"];

impl Costumes {
    /// All item uids that can go in `slot` (1..=6), sorted.
    pub fn slot_items(&self, slot: u32) -> Vec<u16> {
        let mut v: Vec<u16> = self.items.iter().filter(|(_, i)| i.slot == slot).map(|(k, _)| *k).collect();
        v.sort_unstable();
        v
    }

    /// Readable name of an item from its prefab file name.
    pub fn item_label(&self, uid: u16) -> String {
        let raw = self.items.get(&uid).map_or("", |i| i.name.as_str());
        let base = raw.strip_suffix(".prefab").unwrap_or(raw);
        let base = base.strip_prefix("costume_").or_else(|| base.strip_prefix("Costume_")).unwrap_or(base);
        crate::menu::pretty_name(base)
    }

    /// Slot an item belongs to.
    pub fn item_slot(&self, uid: u16) -> Option<u32> {
        self.items.get(&uid).map(|i| i.slot)
    }

    /// Items of a named preset.
    pub fn preset_items(&self, name: &str) -> Vec<(u16, u32)> {
        self.presets.iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.items.clone()).unwrap_or_default()
    }

    /// Build the outfit from per-slot picks (`picks[slot-1]`), honouring the slots an item hides, and dress the actor in it.
    pub fn apply_custom(&mut self, actor: usize, picks: &[Option<u16>; 6]) -> Vec<(u16, u32)> {
        let mut hidden = [false; 7];
        for uid in picks.iter().flatten() {
            if let Some(item) = self.items.get(uid) {
                for d in &item.disable {
                    if (*d as usize) < 7 {
                        hidden[*d as usize] = true;
                    }
                }
            }
        }
        let items: Vec<(u16, u32)> = picks
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.filter(|_| !hidden[i + 1]).map(|uid| (uid, 0)))
            .collect();
        match self.presets.iter_mut().find(|p| p.name == "Custom") {
            Some(p) => p.items = items.clone(),
            None => self.presets.push(Preset { name: "Custom".into(), unlocked: true, items: items.clone() }),
        }
        self.actor_choices.insert(actor, "Custom".into());
        if actor == 0 {
            self.choice = Some("Custom".into());
        }
        self.redress = true;
        items
    }

    /// Cycle the chosen preset (lobby left/right). Returns false when there is nothing to cycle.
    /// Clears `applied` and flags `redress` so `dress` re-runs with the new preset.
    pub fn cycle_preset(&mut self, delta: isize) -> bool {
        if self.presets.is_empty() {
            return false;
        }
        let n = self.presets.len() as isize;
        let current = self
            .choice
            .as_ref()
            .and_then(|name| self.presets.iter().position(|p| p.name.eq_ignore_ascii_case(name)))
            .unwrap_or(0) as isize;
        let next = ((current + delta) % n + n) % n;
        self.choice = Some(self.presets[next as usize].name.clone());
        self.redress = true;
        true
    }

    /// Cycle the preset of one lobby actor.
    pub fn cycle_preset_for(&mut self, actor: usize, delta: isize) -> bool {
        if self.presets.is_empty() {
            return false;
        }
        let n = self.presets.len() as isize;
        let current = self
            .actor_choices
            .get(&actor)
            .or(self.choice.as_ref().filter(|_| actor == 0))
            .and_then(|name| self.presets.iter().position(|p| p.name.eq_ignore_ascii_case(name)))
            .unwrap_or_else(|| preset_for(self, actor).unwrap_or(0)) as isize;
        let next = ((current + delta) % n + n) % n;
        let name = self.presets[next as usize].name.clone();
        self.actor_choices.insert(actor, name.clone());
        if actor == 0 {
            self.choice = Some(name);
        }
        self.redress = true;
        true
    }

    /// Dress an actor in a named preset (Waves enemies).
    pub fn set_preset_for(&mut self, actor: usize, name: &str) {
        if self.presets.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            self.actor_choices.insert(actor, name.to_string());
            self.redress = true;
        }
    }

    /// Preset name chosen for an actor (what the match launch passes through).
    pub fn chosen_for(&self, actor: usize) -> Option<&str> {
        self.actor_choices.get(&actor).map(String::as_str).or(self.chosen_name())
    }

    /// The preset an actor is actually wearing (applied), else the chosen one: what the match launch must
    /// carry over so the lobby look is the match look even if the player never pressed Z/X.
    pub fn worn_for(&self, actor: usize) -> Option<String> {
        self.applied
            .get(&actor)
            .and_then(|&i| self.presets.get(i))
            .map(|p| p.name.clone())
            .or_else(|| self.chosen_for(actor).map(str::to_string))
    }

    /// Name of the currently chosen preset, for the lobby label.
    pub fn chosen_name(&self) -> Option<&str> {
        self.choice.as_deref()
    }
}

impl Costumes {
    /// New stage (in-process switch): actors are new, so dress them again.
    #[allow(dead_code)]
    pub fn reset(&mut self, choice: Option<String>) {
        self.applied.clear();
        if choice.is_some() {
            self.choice = choice;
        }    }
}

/// One costume piece waiting to be (or already) bound onto an actor's skeleton.
#[derive(Component)]
struct Piece {
    actor: usize,
    /// Export uid of this costume item (for its `CostumeSettings` / `KeepIn`/`KeepOut` volumes).
    uid: u16,
    /// Saved colour id of this item in the preset (`CostumeSaveItem.ColorId`, the base tint level).
    colour_id: u32,
    bound: bool,
}

/// The beast's own mesh while an outfit hides part of it: the masked clone, the original handle
/// and the per-vertex covered set, so the body is restored when the outfit changes and several
/// items can union their coverage.
#[derive(Component)]
struct MaskedBody {
    original: Handle<Mesh>,
    covered: Vec<bool>,
}

/// A rigid costume part (hat, mask, head) that `bind` re-parented under a beast bone. `redress`
/// must despawn these explicitly: once re-parented they are no longer descendants of the piece
/// root, so a plain "despawn the piece" leaves them on the beast forever (MORTY's head stayed on
/// after switching outfits).
#[derive(Component)]
struct CostumeAttachment {
    actor: usize,
}

/// A costume material that takes the player palette, and which `GetPlayerTint` shade it uses, so a
/// palette change can re-tint the already-bound outfit (the lobby colour switch).
#[derive(Component)]
struct CostumeTintSlot {
    offset: i32,
    /// `Hair`/`User` materials use `GetHairTint` instead of `GetPlayerTint`.
    hair: bool,
}

/// The shade family of a costume material, from its name, exactly as `ActorCostume.SetCostumeTint`
/// matches: `TintA_Light` -> +0, `TintB_Light` -> +1, `TintA_Dark` -> +2, `TintB_Dark` -> +3.
/// A name containing `Original` is not a slot at all (the source restores its authored colour);
/// neither is `Hair`/`User` (handled by separate tint functions we do not reproduce yet).
fn tint_slot_offset(name: &str, colour_id: u32) -> Option<(i32, bool)> {
    // `ActorCostume.SetCostumeTint` (0x6a68b0): the item's saved colour id is the base level. An
    // `Original` material is skipped when it is 0, otherwise it is tinted at level `id - 1`.
    let mut level = colour_id as i32;
    if name.contains("Original") {
        if colour_id == 0 {
            return None;
        }
        level = colour_id as i32 - 1;
    }
    // `User` and `Hair` take the hair ramp at level + 2.
    if name.contains("User") || name.contains("Hair") {
        return Some((level + 2, true));
    }
    let slot = if name.contains("TintA_Light") {
        0
    } else if name.contains("TintB_Light") {
        1
    } else if name.contains("TintA_Dark") {
        2
    } else if name.contains("TintB_Dark") {
        3
    } else {
        return None;
    };
    Some((level + slot, false))
}

/// `ActorCostume.GetHairTint` (0x6a9980), level mod 4: `v*0.25+0.8`, `v+0.3`, `v-0.2`, `v*0.25+0.1`.
fn hair_tint(value: Vec3, offset: i32) -> Vec3 {
    match offset.rem_euclid(4) {
        0 => value * 0.25 + Vec3::splat(0.8),
        1 => value + Vec3::splat(0.3),
        2 => value - Vec3::splat(0.2),
        _ => value * 0.25 + Vec3::splat(0.1),
    }
    .max(Vec3::ZERO)
}

fn slot_tint(value: Vec3, offset: i32, hair: bool) -> Vec3 {
    if hair { hair_tint(value, offset) } else { player_tint(value, offset) }
}

/// `ActorCostume.GetPlayerTint(value, level)` (RVA 0x6a9b20): a four-step shade ramp around the
/// palette colour (`0.5*value + {+0.25, +0.125, -0.125, -0.25}` for level mod 4). The source
/// replaces the material's base colour with this shade, which is why a costume has light and dark
/// regions of the same player colour rather than one flat fill.
fn player_tint(value: Vec3, offset: i32) -> Vec3 {
    let add = match offset.rem_euclid(4) {
        0 => 0.25,
        1 => 0.125,
        2 => -0.125,
        _ => -0.25,
    };
    Vec3::new(
        0.5 * value.x + add,
        0.5 * value.y + add,
        0.5 * value.z + add,
    )
}

pub fn plugin(app: &mut App, root: &std::path::Path) {
    let path = root.join("costumes/index.json");
    let index: Option<Index> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let Some(index) = index else {
        warn!("no costumes: {} missing", path.display());
        return;
    };
    let items = index
        .items
        .into_iter()
        .filter_map(|(k, v)| k.parse().ok().map(|k| (k, v)))
        .collect();
    info!("{} costume presets", index.presets.len());
    let mut presets = index.presets;
    // The player's own outfit from the costume editor (saved in the lobby prefs file).
    if let Some(list) = crate::menu::load_prefs()["custom_costume"].as_array() {
        let items: Vec<(u16, u32)> = list
            .iter()
            .filter_map(|e| Some((e[0].as_u64()? as u16, e[1].as_u64().unwrap_or(0) as u32)))
            .collect();
        if !items.is_empty() {
            presets.push(Preset { name: "Custom".into(), unlocked: true, items });
        }
    }
    app.insert_resource(Costumes {
        presets,
        items,
        applied: HashMap::new(),
        choice: None,
        actor_choices: HashMap::new(),
        redress: false,
    })
    .insert_resource(CostumeRoot(root.to_path_buf()))
    .add_systems(Update, (wave_costumes, redress, dress, bind, retint, hide_parked_costumes).chain());
}

/// Lobby costume switch: despawn the pieces that are already on the beasts and clear `applied`
/// so `dress` re-runs with the newly chosen preset.
fn redress(
    mut costumes: ResMut<Costumes>,
    pieces: Query<Entity, With<Piece>>,
    attachments: Query<(Entity, &CostumeAttachment)>,
    mut masked: Query<(Entity, &mut Mesh3d, &MaskedBody)>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    if !costumes.redress {
        return;
    }
    costumes.redress = false;
    // Put every masked beast mesh back to its original geometry before re-dressing.
    for (entity, mut mesh, body) in &mut masked {
        mesh.0 = body.original.clone();
        commands.entity(entity).remove::<MaskedBody>();
    }
    // Re-parented rigid parts first: they hang off beast bones, not off the piece root.
    // `despawn` is recursive in Bevy 0.16, so a scene root's spawned meshes cannot survive the switch (that is how cosmetics
    // were staying on the beast); despawning descendants one by one as well only produced "entity does not exist" warnings.
    for (entity, _attachment) in &attachments {
        commands.entity(entity).try_despawn();
    }
    for entity in &pieces {
        commands.entity(entity).try_despawn();
    }
    let _ = &children;
    costumes.applied.clear();
}

/// Default preset for player k: the lobby's chosen preset (or `GB_COSTUME`) belongs to the first
/// player — every other local player gets their own preset from the deterministic per-player
/// formula, or all beasts end up dressed identically.
fn preset_for(costumes: &Costumes, k: usize) -> Option<usize> {
    if costumes.presets.is_empty() {
        return None;
    }
    // `--costumes a|b|c` from the lobby: each local player's worn preset ("none" = no outfit).
    if let Some(name) = std::env::var("GB_COSTUMES").ok().and_then(|v| v.split('|').nth(k).map(str::to_string)) {
        if name == "none" {
            return None;
        }
        if let Some(i) = costumes.presets.iter().position(|p| p.name.eq_ignore_ascii_case(&name)) {
            return Some(i);
        }
    }
    if let Some(name) = costumes.actor_choices.get(&k) {
        if let Some(i) = costumes.presets.iter().position(|p| p.name.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
    }
    let choice = costumes.choice.clone().or_else(|| std::env::var("GB_COSTUME").ok());
    if let Some(name) = choice {
        if k == 0 {
            if name == "none" {
                return None;
            }
            if let Some(i) = costumes.presets.iter().position(|p| p.name.eq_ignore_ascii_case(&name)) {
                return Some(i);
            }
        }
    }
    Some((k * 7 + 3) % costumes.presets.len())
}

fn dress(
    sim: Option<NonSend<Sim>>,
    mut costumes: ResMut<Costumes>,
    map: Res<NodeEntities>,
    assets: Res<AssetServer>,
    mut commands: Commands,
    mut logged: Local<HashSet<usize>>,
) {
    let Some(sim) = sim else { return };
    if std::env::var_os("GB_COSTUME_DEBUG").is_some() {
        info!(
            "dress: called, actors {}, applied {:?}",
            sim.actors.len(),
            costumes.applied.keys().collect::<Vec<_>>()
        );
    }
    for k in 0..sim.actors.len() {
        if costumes.applied.contains_key(&k) {
            continue;
        }
        if std::env::var_os("GB_COSTUME_DEBUG").is_some() && logged.insert(k) {
            info!(
                "dress: actor {k} waiting (scene {}, mapped keys {})",
                sim.actor_scene_of(k),
                map.0.keys().filter(|(s, _)| *s == sim.actor_scene_of(k)).count()
            );
        }
        // Wait until the beast's render nodes are mapped.
        let scene = sim.actor_scene_of(k);
        if !map.0.keys().any(|(s, _)| *s == scene) {
            continue;
        }
        let Some(p) = preset_for(&costumes, k) else {
            costumes.applied.insert(k, usize::MAX);
            continue;
        };
        if std::env::var_os("GB_COSTUME_DEBUG").is_some() {
            info!(
                "dress: actor {k} scene {scene} preset '{}' items {}",
                costumes.presets[p].name,
                costumes.presets[p].items.len()
            );
        }
        for &(uid, colour_id) in &costumes.presets[p].items {
            if !costumes.items.contains_key(&uid) {
                continue;
            }
            commands.spawn((
                SceneRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(format!("costumes/{uid}.glb")))),
                Transform::IDENTITY,
                Piece { actor: k, uid, colour_id, bound: false },
            ));
        }
        info!("player {} costume: {}", k + 1, costumes.presets[p].name);
        costumes.applied.insert(k, p);
    }
}

/// The beast node's world matrix in the exported (glTF, X-mirrored) space, composed up the parent
/// chain from the sidecar's stored transforms — i.e. its BIND pose. Used to attach rigid costume
/// parts so they keep the costume's authored pose relative to the bone.
fn bind_world(beast: &gb_phys::Sidecar, node: usize) -> Mat4 {
    let mut chain = Vec::new();
    let mut current = Some(node);
    while let Some(index) = current {
        chain.push(index);
        current = beast.nodes[index].parent;
    }
    chain.reverse();
    let mut world = Mat4::IDENTITY;
    for index in chain {
        let t = &beast.nodes[index].transform;
        world *= Mat4::from_scale_rotation_translation(
            Vec3::from_array(t.scale),
            Quat::from_array(t.rotation),
            Vec3::from_array(t.translation),
        );
    }
    world
}

/// Mesh assets the unseen-mesh pass needs, bundled so `bind` stays within Bevy's parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
struct MaskAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    inverse_bindposes: Res<'w, Assets<SkinnedMeshInverseBindposes>>,
}

fn bind(
    sim: Option<NonSend<Sim>>,
    root_path: Option<Res<CostumeRoot>>,
    map: Res<NodeEntities>,
    spawner: Res<SceneSpawner>,
    mut pieces: Query<(Entity, &mut Piece, &SceneInstance)>,
    children: Query<&Children>,
    names: Query<&Name>,
    globals: Query<&GlobalTransform>,
    mut skins: Query<&mut SkinnedMesh>,
    mut material_handles: Query<&mut MeshMaterial3d<StandardMaterial>>,
    material_names: Query<&bevy::gltf::GltfMaterialName>,
    menu: Option<Res<crate::menu::Menu>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mask_assets: MaskAssets,
    mut bodies: Query<(&mut Mesh3d, Option<&mut MaskedBody>)>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };
    for (root, mut piece, instance) in &mut pieces {
        if piece.bound || !spawner.instance_is_ready(**instance) {
            continue;
        }
        // Beast bone name -> render entity for this actor.
        let scene = sim.actor_scene_of(piece.actor);
        let mut beast_nodes: Vec<(usize, Entity)> = map
            .0
            .iter()
            .filter(|((s, _), _)| *s == scene)
            .map(|((_, n), e)| (*n, *e))
            .collect();
        beast_nodes.sort_by_key(|(node, _)| *node);
        let bones: HashMap<&str, Entity> = beast_nodes
            .iter()
            .map(|(n, e)| (sim.beast_node_name(*n), *e))
            .collect();
        // Same-skeleton fallback: a costume joint whose NAME does not match a beast node would
        // otherwise keep its own rest pose (it stays where the costume authored it) and the piece
        // pokes out of the body. Costume and beast are exported from the same rig, so the node
        // order matches — but name-only binding is the safe path (see below).
        if bones.is_empty() {
            continue;
        }
        let descendants: Vec<Entity> = children.iter_descendants(root).collect();
        let bone_of = |e: Entity| names.get(e).ok().and_then(|n| bones.get(n.as_str()).copied());
        // Costume meshes follow the ragdoll, so their bind-pose bounds leave the frustum as the
        // beast moves and Bevy culls them (the costume "despawns" when the beast is high up).
        // Disable frustum culling on every costume mesh, exactly as the beast's own meshes do.
        for &e in &descendants {
            if bodies.contains(e) {
                commands
                    .entity(e)
                    .insert(bevy::render::view::NoFrustumCulling);
            }
        }
        // Skinned meshes: joints bind to the beast bone of the same NAME. A joint with no name
        // match keeps its own node (the piece sits slightly off), which is far safer than the
        // index fallback that was here: the beast's mapped node list contains meshes and
        // colliders too, so wrong-index joints bound to non-bone nodes and the outfit stretched
        // from the spawn point to the body.
        for &e in &descendants {
            if let Ok(mut skin) = skins.get_mut(e) {
                for joint in skin.joints.iter_mut() {
                    if let Some(b) = bone_of(*joint) {
                        *joint = b;
                    }
                }
            }
        }
        // Anything parented to a matching costume bone that isn't itself a matching bone (extra
        // bones, rigid hats/masks) moves under the beast bone, keeping its local offset. The
        // moved entity is marked so `redress` can despawn it later (it is no longer a descendant
        // of the piece root). The marker must go on the CHILD, never on the beast bone.
        for &e in &descendants {
            let Some(beast_bone) = bone_of(e) else { continue };
            if let Ok(kids) = children.get(e) {
                for &kid in kids {
                    if bone_of(kid).is_none() {
                        // Rigid part (hat, hair, mask): parent it to the beast bone keeping the
                        // AUTHORED pose. The costume is authored on the same rig, so the local
                        // transform relative to the bone's BIND pose is what carries over —
                        // `inverse(bone_bind_world) * kid_authored_world`. (Using the bone's
                        // *current* pose would throw the hat to the rig origin, and keeping the
                        // raw local transform drops the parent chain's scale: hats are authored
                        // under a 2.54x bone with a 0.39x mesh.)
                        if let Ok(kid_world) = globals.get(kid) {
                            if let Some(bone_node) = beast_nodes
                                .iter()
                                .find(|(_, entity)| *entity == beast_bone)
                                .map(|(node, _)| *node)
                            {
                                let bind = bind_world(sim.beast_sidecar(), bone_node);
                                let local = bind.inverse() * kid_world.affine();
                                let (scale, rotation, translation) =
                                    local.to_scale_rotation_translation();
                                commands.entity(kid).insert(Transform {
                                    translation,
                                    rotation,
                                    scale,
                                });
                            }
                        }
                        commands
                            .entity(kid)
                            .insert(CostumeAttachment { actor: piece.actor });
                        commands.entity(beast_bone).add_child(kid);
                    }
                }
            }
        }
        // Costume albedo correction. The costume materials are authored near-white (the default
        // onesie's material is literally "White"), and under the menu's cool sky ambient they
        // render as a blown pink (measured 233,208,203 against the reference's warm cream
        // 214,199,180: too bright by ~10% and too blue). This multiplies a measured correction
        // into the costume albedo only (never the scenery): warm it and bring it down to the
        // reference level. Stand-in until the baked GI / SH ambient gives the real result;
        // GB_COSTUME_TINT="r,g,b" overrides.
        // Older costume exports wrote Color32 vertex colours as 0..255 floats (the diver's suit
        // multiplied its albedo by ~50 and blew the whole screen out). Normalise them in place.
        for &e in &descendants {
            let Ok((mesh, _)) = bodies.get(e) else { continue };
            let handle = mesh.0.clone();
            let Some(asset) = mask_assets.meshes.get(&handle) else { continue };
            let needs = matches!(
                asset.attribute(Mesh::ATTRIBUTE_COLOR),
                Some(bevy::render::mesh::VertexAttributeValues::Float32x4(c)) if c.iter().any(|v| v[0] > 1.5 || v[1] > 1.5 || v[2] > 1.5)
            );
            if needs {
                if let Some(asset) = mask_assets.meshes.get_mut(&handle) {
                    if let Some(bevy::render::mesh::VertexAttributeValues::Float32x4(c)) =
                        asset.attribute_mut(Mesh::ATTRIBUTE_COLOR)
                    {
                        for v in c.iter_mut() {
                            for k in 0..4 {
                                v[k] /= 255.0;
                            }
                        }
                    }
                }
            }
        }
        let correction = costume_tint();
        let gamma_to_linear = |v: f32| {
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        for &e in &descendants {
            let Ok(mut handle) = material_handles.get_mut(e) else { continue };
            let Some(material) = materials.get(&handle.0) else { continue };
            // Unity exported material colours in GAMMA space into the glTF factors; the loader
            // treats them as linear, which washes the costume out (gbrender's finding). Convert
            // gamma -> linear, then apply the measured correction.
            let base = material.base_color.to_linear();
            let linear = Color::linear_rgb(
                gamma_to_linear(base.red),
                gamma_to_linear(base.green),
                gamma_to_linear(base.blue),
            )
            .to_linear();
            let mut adjusted = material.clone();
            // Keep the authored alpha (glass visors are 18% opaque).
            adjusted.base_color = Color::linear_rgba(
                linear.red * correction[0],
                linear.green * correction[1],
                linear.blue * correction[2],
                material.base_color.alpha(),
            );
            // Costume GLBs carry emissive on some pieces; keeping it makes the outfit glow.
            // Only true authored emission (the Logos etc.) should ever emit.
            adjusted.emissive = LinearRgba::BLACK;
            // Glass visors (URP Lit, alpha ~0.18, near-mirror smoothness) throw the sun's HDR
            // highlight straight into bloom and wash out the whole screen; keep them dull.
            if matches!(adjusted.alpha_mode, AlphaMode::Blend) {
                adjusted.perceptual_roughness = adjusted.perceptual_roughness.max(0.6);
                adjusted.reflectance = adjusted.reflectance.min(0.04);
                adjusted.metallic = 0.0;
            }
            *handle = MeshMaterial3d(materials.add(adjusted));
            // Debug: GB_COSTUME_HIDE_MAT=<material name> hides costume meshes using it.
            if let (Ok(hide), Ok(n)) = (std::env::var("GB_COSTUME_HIDE_MAT"), material_names.get(e)) {
                if n.0 == hide {
                    commands.entity(e).insert(Visibility::Hidden);
                }
            }
        }
        // Costume colour slots (`ActorCostume.SetCostumeTint`). Only the materials whose name
        // carries a `Tint*` slot take the player colour; a name containing `Original` keeps its
        // authored tone. Flooding every `TintA*`/`TintB*` (including `Original_*`) with the flat
        // palette is what turned the astronaut and owl outfits into one solid player-coloured
        // red. Each slot is a SHADE of the palette (see `player_tint`), which is what gives an
        // outfit a light and a dark region of the same colour.
        let player_colour = menu
            .as_ref()
            .and_then(|m| m.selected_color())
            // Match play: with no menu (a stage) each fighter wears its OWN palette colour, the
            // same index `apply_player_colors` gives the body.
            .or_else(|| Some(sim.player_color(piece.actor)));
        let palette = player_colour
            .map(|c| c.to_linear())
            .unwrap_or(LinearRgba::new(1.0, 1.0, 1.0, 1.0));
        for &e in &descendants {
            let Ok(mut handle) = material_handles.get_mut(e) else { continue };
            let name = material_names.get(e).map(|n| n.0.clone()).unwrap_or_default();
            let Some((offset, hair)) = tint_slot_offset(&name, piece.colour_id) else { continue };
            let Some(material) = materials.get(&handle.0) else { continue };
            let mut adjusted = material.clone();
            let shade = slot_tint(Vec3::new(palette.red, palette.green, palette.blue), offset, hair);
            adjusted.base_color = Color::linear_rgba(shade.x, shade.y, shade.z, material.base_color.alpha());
            adjusted.emissive = LinearRgba::BLACK;
            if std::env::var_os("GB_UNSEEN_DEBUG").is_some() {
                info!("tint slot {name}: offset {offset} hair {hair} shade {shade:?} palette {palette:?} alpha {} mode {:?} unlit {} tex {}", material.base_color.alpha(), material.alpha_mode, material.unlit, material.base_color_texture.is_some());
            }
            adjusted.metallic = adjusted.metallic.min(std::env::var("GB_TINT_METAL_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0));
            let new_handle = materials.add(adjusted);
            *handle = MeshMaterial3d(new_handle);
            commands.entity(e).insert(CostumeTintSlot { offset, hair });
        }
        // RemoveUnseenMesh: collapse the beast's own mesh where this item covers it, so the body
        // does not poke through the outfit. The costume sidecar carries the item's
        // `CostumeSettings` (ReduceUsingMesh) and its `KeepIn*`/`KeepOut*` volumes; a beast bone
        // whose rest position sits inside a KeepIn volume (and outside every KeepOut volume) is
        // covered, and the beast mesh vertices weighted to it are collapsed onto that bone.
        if let Some(root) = root_path.as_ref() {
            if let Some(volumes) = crate::unseen::load(&root.0, piece.uid) {
                if std::env::var_os("GB_UNSEEN_DEBUG").is_some() {
                    info!(
                        "unseen: uid {} reduce {} keepin {} keepout {}",
                        piece.uid,
                        volumes.reduce,
                        volumes.keep_in.len(),
                        volumes.keep_out.len()
                    );
                }
                if volumes.reduce {
                    // Rest-pose cloth positions: the costume is authored on the same rig as the
                    // wearer, so its skinned-mesh vertices share the rig space the mask tests in.
                    let mut cloth_points: Vec<[Vec3; 3]> = Vec::new();
                    for &e in &descendants {
                        if !skins.contains(e) {
                            continue;
                        }
                        let Ok((mesh, _)) = bodies.get(e) else { continue };
                        let Some(asset) = mask_assets.meshes.get(&mesh.0) else { continue };
                        let Some(bevy::render::mesh::VertexAttributeValues::Float32x3(points)) =
                            asset.attribute(Mesh::ATTRIBUTE_POSITION)
                        else {
                            continue;
                        };
                        let idx: Vec<u32> = match asset.indices() {
                            Some(bevy::render::mesh::Indices::U16(i)) => i.iter().map(|v| *v as u32).collect(),
                            Some(bevy::render::mesh::Indices::U32(i)) => i.clone(),
                            None => continue,
                        };
                        for t in idx.chunks_exact(3) {
                            cloth_points.push([
                                Vec3::from_array(points[t[0] as usize]),
                                Vec3::from_array(points[t[1] as usize]),
                                Vec3::from_array(points[t[2] as usize]),
                            ]);
                        }
                    }
                    if std::env::var_os("GB_UNSEEN_DEBUG").is_some() {
                        let (mn, mx) = cloth_points.iter().flatten().fold(
                            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                            |(mn, mx), p| (mn.min(*p), mx.max(*p)),
                        );
                        info!(
                            "unseen: uid {} reduce {} keepin {} keepout {} cloth {} verts bbox {:?}..{:?}",
                            piece.uid, volumes.reduce, volumes.keep_in.len(),
                            volumes.keep_out.len(), cloth_points.len(), mn, mx
                        );
                    }
                    let entity_node: HashMap<Entity, usize> = beast_nodes
                        .iter()
                        .map(|(node, entity)| (*entity, *node))
                        .collect();
                    for (node, entity) in &beast_nodes {
                        let path = &sim.beast_sidecar_of(piece.actor).nodes[*node].path;
                        if !path.ends_with("_skinnedMesh") {
                            continue;
                        }
                        // The mapped entity is the glTF NODE (it carries `GltfExtras`); the skinned
                        // mesh primitives are its children (the head has two). The mask must run on
                        // the primitive entity that actually owns the `Mesh3d` + `SkinnedMesh`, or
                        // it silently does nothing (that was the body-poking-through bug).
                        let mut targets = vec![*entity];
                        targets.extend(children.iter_descendants(*entity));
                        for target in targets {
                            if !skins.contains(target) {
                                continue;
                            }
                            mask_beast_mesh(
                                target,
                                &volumes,
                                &cloth_points,
                                sim.beast_sidecar_of(piece.actor),
                                &entity_node,
                                &mut skins,
                                &mut mask_assets.meshes,
                                &mask_assets.inverse_bindposes,
                                &mut bodies,
                                &mut commands,
                            );
                        }
                    }
                }
            }
        }
        piece.bound = true;
    }
}

/// Collapses the vertices of one wearer mesh that a costume item hides (see [`crate::unseen`]).
/// Which vertices are hidden is recomputed per item from the item's own cloth geometry, and the
/// mask is the union over every equipped item, always applied to the unmasked mesh, so switching
/// outfits restores the body exactly.
fn mask_beast_mesh(
    entity: Entity,
    volumes: &crate::unseen::CostumeVolumes,
    cloth_points: &[[Vec3; 3]],
    beast: &gb_phys::Sidecar,
    entity_node: &HashMap<Entity, usize>,
    skins: &mut Query<&mut SkinnedMesh>,
    mesh_assets: &mut Assets<Mesh>,
    inverse_bindposes: &Assets<SkinnedMeshInverseBindposes>,
    bodies: &mut Query<(&mut Mesh3d, Option<&mut MaskedBody>)>,
    commands: &mut Commands,
) {
    let Ok(skin) = skins.get(entity) else { return };
    let Some(bindposes) = inverse_bindposes.get(&skin.inverse_bindposes) else { return };
    let grow = std::env::var("GB_UNSEEN_GROW")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);

    let Ok((mut mesh, existing)) = bodies.get_mut(entity) else { return };
    let (original, mut covered) = match existing {
        Some(body) => (body.original.clone(), body.covered.clone()),
        None => (mesh.0.clone(), Vec::new()),
    };
    let Some(source) = mesh_assets.get(&original).cloned() else { return };
    let Some(bevy::render::mesh::VertexAttributeValues::Float32x3(positions)) =
        source.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return;
    };
    let normals = source.attribute(Mesh::ATTRIBUTE_NORMAL).and_then(|v| match v {
        bevy::render::mesh::VertexAttributeValues::Float32x3(n) => Some(n.clone()),
        _ => None,
    });
    let Some(normals) = normals else { return };
    let joints = source.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).and_then(|v| match v {
        bevy::render::mesh::VertexAttributeValues::Uint16x4(j) => Some(j.clone()),
        _ => None,
    });
    let weights = source.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).and_then(|v| match v {
        bevy::render::mesh::VertexAttributeValues::Float32x4(w) => Some(w.clone()),
        _ => None,
    });
    let (Some(joints), Some(weights)) = (joints, weights) else { return };

    // Mesh space -> rig space at the bind pose: each joint's bind world times its inverse bind.
    // (The exported mesh vertices are in the mesh's own Z-up space; the skin matrices carry the
    // mapping, so the raw node transform is not enough.)
    let joint_matrices: Vec<(Mat4, Mat4)> = skin
        .joints
        .iter()
        .enumerate()
        .map(|(i, joint)| {
            let bind = entity_node
                .get(joint)
                .map(|node| bind_world(beast, *node))
                .unwrap_or(Mat4::IDENTITY);
            let inverse = bindposes.get(i).copied().unwrap_or(Mat4::IDENTITY);
            (bind, inverse)
        })
        .collect();

    let world_positions: Vec<[f32; 3]> = (0..positions.len())
        .map(|i| {
            let p = Vec3::from_array(positions[i]);
            let mut w = Vec3::ZERO;
            for k in 0..4 {
                if weights[i][k] == 0.0 {
                    continue;
                }
                let (bind, inverse) = joint_matrices
                    .get(joints[i][k] as usize)
                    .copied()
                    .unwrap_or((Mat4::IDENTITY, Mat4::IDENTITY));
                w += (bind * inverse).transform_point3(p) * weights[i][k];
            }
            [w.x, w.y, w.z]
        })
        .collect();
    let world_normals: Vec<[f32; 3]> = (0..normals.len())
        .map(|i| {
            let n = Vec3::from_array(normals[i]);
            let mut w = Vec3::ZERO;
            for k in 0..4 {
                if weights[i][k] == 0.0 {
                    continue;
                }
                let (bind, inverse) = joint_matrices
                    .get(joints[i][k] as usize)
                    .copied()
                    .unwrap_or((Mat4::IDENTITY, Mat4::IDENTITY));
                w += (bind * inverse).transform_vector3(n) * weights[i][k];
            }
            [w.x, w.y, w.z]
        })
        .collect();
    let covered_now =
        crate::unseen::covered_vertices(&world_positions, &world_normals, cloth_points, volumes);
    if covered.len() != covered_now.len() {
        covered.resize(covered_now.len(), false);
    }
    let mut changed = false;
    for (slot, now) in covered.iter_mut().zip(covered_now.iter()) {
        if *now && !*slot {
            *slot = true;
            changed = true;
        }
    }
    if std::env::var_os("GB_UNSEEN_DEBUG").is_some() {
        let (mn, mx) = world_positions.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(mn, mx), p| (mn.min(Vec3::from_array(*p)), mx.max(Vec3::from_array(*p))),
        );
        info!(
            "unseen {:?}: {} verts, {} covered (item cloth {}), wearer bbox {:?}..{:?}",
            mesh.0.id(),
            covered.len(),
            covered.iter().filter(|c| **c).count(),
            cloth_points.len(),
            mn,
            mx
        );
    }
    if !changed {
        return;
    }
    let mut masked_mesh = source.clone();
    if !crate::unseen::remove_covered_faces(&mut masked_mesh, &source, &covered, grow) {
        return;
    }
    mesh.0 = mesh_assets.add(masked_mesh);
    commands
        .entity(entity)
        .insert(MaskedBody { original, covered });
}

/// Puts every masked beast mesh back to its original geometry (outfit switch / removal).
#[allow(dead_code)]
fn restore_masked_bodies(
    mut masked: Query<(Entity, &mut Mesh3d, &MaskedBody)>,
    mut commands: Commands,
) {
    for (entity, mut mesh, body) in &mut masked {
        mesh.0 = body.original.clone();
        commands.entity(entity).remove::<MaskedBody>();
    }
}

/// Lobby colour switch: re-tint the already-bound costume slots from their authored tone when the
/// player's palette swatch changes (the costume is bound once, so without this the outfit kept
/// the colour it was first dressed with — "colour switching doesn't really work").
fn retint(
    costumes: Res<Costumes>,
    menu: Option<Res<crate::menu::Menu>>,
    slots: Query<(&CostumeTintSlot, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut last: Local<Option<usize>>,
) {
    let Some(menu) = menu else { return };
    if costumes.presets.is_empty() {
        return;
    }
    if *last == Some(menu.player_color) {
        return;
    }
    *last = Some(menu.player_color);
    let Some(colour) = menu.selected_color() else { return };
    let palette = colour.to_linear();
    let shade_of = |offset: i32, hair: bool| {
        slot_tint(Vec3::new(palette.red, palette.green, palette.blue), offset, hair)
    };
    for (slot, handle) in &slots {
        let Some(material) = materials.get(&handle.0) else { continue };
        let mut adjusted = material.clone();
        // Same `GetPlayerTint` shade the bind pass used, from the freshly selected palette.
        let shade = shade_of(slot.offset, slot.hair);
        adjusted.base_color = Color::linear_rgba(shade.x, shade.y, shade.z, material.base_color.alpha());
        adjusted.emissive = LinearRgba::BLACK;
        materials.insert(&handle.0, adjusted);
    }
}

/// Costume albedo correction (linear multipliers). The costume materials are gamma-decoded to
/// linear at bind time, so the authored albedo is correct; 1.0 unless a beast-only measurement
/// says otherwise (the old 0.74/0.77/0.80 came from a scenery-contaminated measurement).
/// `GB_COSTUME_TINT="r,g,b"` overrides.
fn costume_tint() -> [f32; 3] {
    // Through the knob store so the in-game panel (F2) can change it live.
    crate::devgui::knob_rgb("GB_COSTUME_TINT", [1.0, 1.0, 1.0])
}

/// Waves: apply the costumes the wave director asked for.
fn wave_costumes(sim: Option<NonSendMut<Sim>>, mut costumes: ResMut<Costumes>) {
    let Some(mut sim) = sim else { return };
    for (actor, name) in std::mem::take(&mut sim.wave_costumes) {
        costumes.set_preset_for(actor, &name);
    }
}

/// Costume pieces of a parked beast (a defeated wave enemy) must not stay behind where it fell.
fn hide_parked_costumes(
    sim: Option<NonSend<Sim>>,
    mut pieces: Query<(&Piece, &mut Visibility), Without<CostumeAttachment>>,
    mut attached: Query<(&CostumeAttachment, &mut Visibility), Without<Piece>>,
) {
    let Some(sim) = sim else { return };
    let want = |actor: usize| {
        if sim.parked.get(actor).copied().unwrap_or(false) { Visibility::Hidden } else { Visibility::Inherited }
    };
    for (p, mut v) in &mut pieces {
        let w = want(p.actor);
        if *v != w {
            *v = w;
        }
    }
    for (a, mut v) in &mut attached {
        let w = want(a.actor);
        if *v != w {
            *v = w;
        }
    }
}
