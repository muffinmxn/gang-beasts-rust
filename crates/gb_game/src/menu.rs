//! Main menu (Menu Alley scene, `stages-menu_scenes_all`): the authored 3D room and camera space
//! with TMP screen UI laid out from its Unity Canvas RectTransforms. The camera follows the
//! active screen's `cameraTarget` + `cameraOffset` with the scene's 40 deg lens.
//!
//! Flow ported: Splash ("Press a Button") -> Main Menu (Local / ... / Quit) -> Local Beast
//! Select (Mode, Wins, Stage, Start). Start launches a local rooftop match.
use crate::scene::{SceneData, SourceRect};
use bevy::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

const SPLASH: &str = "Managers/Menu/Load/Canvas/Splash";
const MAIN: &str = "Managers/Menu/Main Menu/Canvas/Main Menu";
const SETTINGS: &str = "Managers/Menu/Settings Menu/Canvas/Root Settings";
const COSTUME_EDITOR: &str = "Managers/Menu/Costume Menu/Canvas/Costumes Root/CostumeEditContainer";
const LOBBY: &str = "Managers/Menu/Beast Menu/Canvas/Local Beast Select Menu";
const GRAPHICS: &str = "graphics";
const CONTROLS: &str = "controls";
const AUDIO: &str = "audio";
const CREDITS: &str = "credits";
const FOV_DEGREES: f32 = 40.0;
const COSTUME_COLOR_ITEM: usize = usize::MAX;
const COSTUME_PRESET_ITEM: usize = usize::MAX - 2;
/// Button.m_Colors of the main menu buttons (normal yellow, selected pink).
const NORMAL: Color = Color::srgb(1.0, 0.752, 0.0);
const SELECTED: Color = Color::srgb(0.778, 0.442, 0.534);

#[derive(Resource)]
pub struct MenuRoot(pub PathBuf);

#[derive(Clone)]
struct MenuText {
    /// Canvas-local authored coordinates (pixels, origin at the canvas pivot).
    ui_position: Vec2,
    text: String,
    size: f32,
    align: JustifyText,
    width: f32,
    /// Selectable that tints this text (Button / MultiGraphic row), if any.
    item: Option<usize>,
    color: Color,
    /// World-space canvas text: pivot position and canvas-pixel -> world scale. Projected
    /// through the camera like Unity renders a world-space Canvas. None: screen overlay.
    world: Option<(Vec3, f32)>,
    /// Only shown while the OG loading screen is up (the splash's LoadingPlatform state).
    loading: bool,
}

struct Screen {
    camera: Transform,
    canvas_size: Vec2,
    items: Vec<usize>,
    /// item -> (up, down)
    nav: HashMap<usize, (Option<usize>, Option<usize>)>,
    default: Option<usize>,
    texts: Vec<MenuText>,
}

#[derive(Resource)]
pub struct Menu {
    /// Local players who joined in the lobby.
    joined: usize,
    /// Lobby host: the first device that joined (None = keyboard).
    host: Option<Option<Entity>>,
    /// Lobby: the host is ready, so the match options show and take input.
    lobby_options: bool,
    /// BeastMenuSpawner spawn points (Managers/Menu/Spawns/Spawn), Bevy space.
    lobby_spawns: Vec<Transform>,
    /// Costume editor: where the display beast stands, and its actor while the screen is open.
    editor_spawn: Option<Transform>,
    editor_actor: Option<usize>,
    /// Costume editor row label: the costume the display beast wears.
    editor_costume: String,
    screens: HashMap<&'static str, Screen>,
    current: &'static str,
    selected: Option<usize>,
    wins: u32,
    palette: Vec<(String, Color)>,
    /// Selected palette swatch (lobby colour switch, Q/E).
    pub player_color: usize,
    /// Exported stages the lobby can launch (name, has all three export files).
    stages: Vec<String>,
    /// Selected stage index (lobby left/right on the stage row).
    stage_index: usize,
    mode: crate::round::Mode,
    /// Local AI opponents added to the match (B in the lobby).
    bots: usize,
    item_keys: HashMap<usize, String>,
    root: PathBuf,
    /// Audio settings, 0..=10 steps (master / music / effects). Persisted in the lobby prefs file.
    pub vol_master: u32,
    pub vol_music: u32,
    pub vol_sfx: u32,
    dirty: bool,
    resolution: usize,
    vsync: bool,
    bloom: bool,
    /// Settings: realtime shadows on/off (applies to every directional light immediately).
    shadows: bool,
    /// Settings: ambient occlusion on the stage camera (inserts/removes the SSAO component).
    ao: bool,
    /// Settings rows from the source's `SETTINGS_GRAPHICS_*` screen. Values are indices into the
    /// per-row option lists; `apply_graphics` pushes them into the engine.
    aa: usize,               // 0 Off, 1 2x, 2 4x, 3 8x (Unity's anti-aliasing row)
    aniso: usize,            // 0 Off, 1 2x, … 4 16x (anisotropic filtering)
    quality: usize,          // 0 Low, 1 Medium, 2 High, 3 Ultra
    texture_quality: usize,  // 0 Low, 1 Medium, 2 High
    ssr: bool,               // screen space reflections (stand-in: reflection intensity)
    grain: bool,             // film grain
    dof: usize,              // 0 Off, 1 Gaussian, 2 Bokeh
    ca: bool,                // chromatic aberration
    vignette: bool,
    screen_mode: usize,      // 0 Windowed, 1 Borderless, 2 Fullscreen
    /// Set by `input` when a graphics row changed; `apply_graphics` consumes it.
    graphics_dirty: bool,
    /// Last input device used in the lobby: false = keyboard/mouse, true = gamepad. Drives
    /// which bind glyphs the tooltips show.
    pub pad_active: bool,
    status: String,
    /// Seconds left on the OG loading screen before the match process is launched.
    launch: Option<f32>,
    /// Arguments for the pending match launch.
    launch_args: Vec<String>,
}

#[derive(Component)]
struct MenuLabel(usize);

/// Marker for the lobby's bind-prompt glyphs (keyboard / gamepad icons).
#[derive(Component)]
struct PromptIcon;

/// The authored (stage volume) bloom intensity, so the Bloom row can toggle it off and back
/// without losing the tuned value.
#[derive(Component)]
pub struct AuthoredBloom(pub f32);

const RESOLUTIONS: [(u32, u32); 3] = [(1280, 720), (1600, 900), (1920, 1080)];

fn option_screen(camera: Transform, rows: &[(&str, &str)], keys: &mut HashMap<usize, String>) -> Screen {
    option_screen_spaced(camera, rows, keys, 300.0, 90.0, 40.0)
}

/// Like [`option_screen`] but with explicit row geometry. The graphics screen has 17 rows and
/// must fit the 720-tall window (the source screen scrolls; ours is one page).
fn option_screen_spaced(
    camera: Transform,
    rows: &[(&str, &str)],
    keys: &mut HashMap<usize, String>,
    first_y: f32,
    spacing: f32,
    size: f32,
) -> Screen {
    let mut items = Vec::new();
    let texts = rows.iter().enumerate().map(|(i, &(key, text))| {
        let item = if key.is_empty() { None } else {
            let id = usize::MAX - 1 - keys.len();
            keys.insert(id, key.to_string());
            items.push(id);
            Some(id)
        };
        MenuText { world: None, ui_position: Vec2::new(0.0, first_y - i as f32 * spacing),
            text: text.into(), size, align: JustifyText::Center,
            width: 1000.0, item, color: NORMAL, loading: false }
    }).collect();
    Screen { camera, canvas_size: Vec2::new(1080.0, 1000.0), default: items.first().copied(),
        items, nav: HashMap::new(), texts }
}

impl Menu {
    fn go(&mut self, to: &'static str) {
        if !self.screens.contains_key(to) { return; }
        self.current = to;
        self.selected = self.screens.get(to).and_then(|s| s.default);
        self.status.clear();
        self.dirty = true;
    }
    /// Selected palette swatch colour (lobby colour switch), if any.
    pub fn selected_color(&self) -> Option<Color> {
        self.palette.get(self.player_color).map(|(_, color)| *color)
    }
}

pub fn plugin(app: &mut App) {
    // Main's Startup system queues the 3D camera. PostStartup runs after it exists.
    app.add_systems(PostStartup, setup)
        .add_systems(
            Update,
            (input, apply_graphics, cycle_for_capture, animate_grain, layout, hide_static_rigs).chain(),
        );
}

fn strip_key(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(t)
        .to_string()
}

fn setup(
    mut commands: Commands,
    root: Res<MenuRoot>,
    assets: Res<AssetServer>,
    cams: Query<Entity, With<Camera3d>>,
) {
    let scene = match SceneData::load(&root.0, "menu") {
        Ok(s) => s,
        Err(e) => {
            error!("menu: {e}");
            return;
        }
    };
    let strings: Value = std::fs::read(root.0.join("ui/strings_en.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // Stages this lobby can launch: an export is launchable when `<name>.glb`, `<name>.json`
    // and `<name>-graphics.json` all exist (the same set the CLI accepts).
    let prefs = load_prefs();
    let stages: Vec<String> = {
        let mut found: Vec<String> = std::fs::read_dir(&root.0)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let stem = name.strip_suffix(".glb")?;
                        // Test-only exports the retail game never offers.
                        (!matches!(stem, "chute" | "aquarium" | "menu" | "beast" | "beast_big" | "beast_tiny")
                            && root.0.join(format!("{stem}.json")).is_file()
                            && root.0.join(format!("{stem}-graphics.json")).is_file())
                        .then(|| stem.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    };
        let palette: Vec<(String, Color)> = std::fs::read(root.0.join("player-colors.json"))
        .ok()
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .and_then(|data| data["colors"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let rgba = entry["rgba"].as_array()?;
            (rgba.len() == 4).then(|| {
                let component = |i: usize| rgba[i].as_f64().unwrap_or(1.0) as f32;
                let name = entry["loc_code"]
                    .as_str()
                    .and_then(|code| strings[code].as_str())
                    .unwrap_or("Color")
                    .to_string();
                (
                    name,
                    Color::linear_rgba(component(0), component(1), component(2), component(3)),
                )
            })
        })
        .collect();
    let palette_len = palette.len();
    let nodes = &scene.nodes;
    // The stage viewer loads the first directional light only. Alley has a separate
    // 2-unit key in addition to its 0.2-unit fill; retain both authored orientations.
    let source_world = scene.world_transforms();
    let mut first_sun = true;
    for (i, node) in nodes.iter().enumerate().filter(|(_, n)| n.active_in_hierarchy) {
        for component in &node.components {
            let d = &component.data;
            if component.kind != "Light" || d["m_Type"].as_u64() != Some(1)
                || d["m_Enabled"].as_u64() != Some(1) { continue; }
            if first_sun { first_sun = false; continue; }
            let c = &d["m_Color"];
            crate::look::Look::menu().spawn_sun(
                &mut commands,
                Color::srgb(c["r"].as_f64().unwrap_or(1.0) as f32,
                    c["g"].as_f64().unwrap_or(1.0) as f32,
                    c["b"].as_f64().unwrap_or(1.0) as f32),
                // Rooftop's outdoor calibration (2,600 lux per Unity unit) scaled so this 2.0 key
                // lands at Rooftop's 1.5-unit sun: the menu should read like Rooftop. 4000 is the
                // off-screen tuner's best for the title reference.
                d["m_Intensity"].as_f64().unwrap_or(1.0) as f32
                    * std::env::var("GB_MENU_SUN_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(4000.0),
                d["m_Shadows"]["m_Type"].as_u64().unwrap_or(0) > 0
                    && std::env::var_os("GB_NO_SHADOWS").is_none(),
                d["m_Shadows"]["m_Strength"].as_f64().unwrap_or(1.0) as f32,
                crate::scene::unity_light_to_bevy(source_world[i].compute_transform()),
            );
        }
    }
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            children.entry(p).or_default().push(i);
        }
    }
    let script = |i: usize, name: &str| {
        nodes[i]
            .components
            .iter()
            .find(|c| c.script.as_deref() == Some(name))
            .map(|c| &c.data)
    };
    let is_item = |i: usize| {
        nodes[i].components.iter().any(|c| {
            c.script.as_deref() == Some("Button")
                || c.script
                    .as_deref()
                    .is_some_and(|s| s.starts_with("MenuHandler"))
        }) && !script(i, "DisabledIfPlatform").is_some_and(|d| d["OnSteam"].as_i64() == Some(1))
    };
    // RectTransform size: parent size * anchor span + sizeDelta.
    let mut sizes: Vec<Vec2> = vec![Vec2::ZERO; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        if let Some(SourceRect {
            anchor_min,
            anchor_max,
            size_delta,
            ..
        }) = n.rect
        {
            let parent = n.parent.map_or(Vec2::ZERO, |p| sizes[p]);
            sizes[i] =
                parent * (Vec2::from(anchor_max) - Vec2::from(anchor_min)) + Vec2::from(size_delta);
        }
    }
    // Vertical/HorizontalLayoutGroup re-lay their children at runtime (the saved anchored
    // positions are stale). Children that are inactive, disabled for this platform, or out of
    // scope (online/wireless) take no space.
    let hidden = |i: usize| {
        !nodes[i].active
            || script(i, "DisabledIfPlatform").is_some_and(|d| d["OnSteam"].as_i64() == Some(1))
            || nodes[i].path.ends_with("Main Menu/TextSection/Online")
            || nodes[i].path.ends_with("Main Menu/TextSection/Wireless")
    };
    let mut layout_pos: Vec<Option<Vec2>> = vec![None; nodes.len()];
    for (p, pn) in nodes.iter().enumerate() {
        let (group, vertical) = match (script(p, "VerticalLayoutGroup"), script(p, "HorizontalLayoutGroup")) {
            (Some(g), _) => (g, true),
            (None, Some(g)) => (g, false),
            _ => continue,
        };
        let Some(prect) = pn.rect else { continue };
        let size = sizes[p];
        let pad = &group["m_Padding"];
        let pf = |k: &str| pad[k].as_f64().unwrap_or(0.0) as f32;
        let (l, r, t, b) = (pf("m_Left"), pf("m_Right"), pf("m_Top"), pf("m_Bottom"));
        let spacing = group["m_Spacing"].as_f64().unwrap_or(0.0) as f32;
        let align = group["m_ChildAlignment"].as_i64().unwrap_or(0);
        let (row, col) = (align / 3, align % 3);
        let kids: Vec<usize> = children.get(&p).into_iter().flatten().copied()
            .filter(|&c| !hidden(c) && nodes[c].rect.is_some())
            .collect();
        if kids.is_empty() {
            continue;
        }
        let main = |c: usize| if vertical { sizes[c].y } else { sizes[c].x };
        let total: f32 = kids.iter().map(|&c| main(c)).sum::<f32>() + spacing * (kids.len() as f32 - 1.0);
        let pivot = Vec2::from_array(prect.pivot) * size;
        let mut acc = 0.0;
        for &c in &kids {
            let cs = sizes[c];
            let centre = if vertical {
                let top = match row {
                    0 => size.y - t,
                    1 => b + (size.y - t - b + total) * 0.5,
                    _ => b + total,
                };
                let x = match col {
                    0 => l + cs.x * 0.5,
                    1 => l + (size.x - l - r) * 0.5,
                    _ => size.x - r - cs.x * 0.5,
                };
                Vec2::new(x, top - acc - cs.y * 0.5)
            } else {
                let left = match col {
                    0 => l,
                    1 => l + (size.x - l - r - total) * 0.5,
                    _ => size.x - r - total,
                };
                let y = match row {
                    0 => size.y - t - cs.y * 0.5,
                    1 => b + (size.y - t - b) * 0.5,
                    _ => b + cs.y * 0.5,
                };
                Vec2::new(left + acc + cs.x * 0.5, y)
            };
            acc += main(c) + spacing;
            let cp = nodes[c].rect.map_or(Vec2::splat(0.5), |r| Vec2::from_array(r.pivot));
            layout_pos[c] = Some(centre + (cp - Vec2::splat(0.5)) * cs - pivot);
        }
    }
    // RectTransform.anchoredPosition is separate from Transform in Unity's export. Rebuild the
    // hierarchy with each pivot placed relative to its anchor rectangle so labels follow their
    // authored menu layout instead of collapsing at their GameObject origins.
    let mut world: Vec<GlobalTransform> = Vec::with_capacity(nodes.len());
    for n in nodes {
        let mut local = n.transform.to_transform();
        if let Some(rect) = n.rect {
            let parent_size = n.parent.map_or(Vec2::ZERO, |p| sizes[p]);
            let parent_pivot = n
                .parent
                .and_then(|p| nodes[p].rect)
                .map_or(Vec2::ZERO, |r| Vec2::from_array(r.pivot));
            let anchor_min = Vec2::from_array(rect.anchor_min);
            let anchor_max = Vec2::from_array(rect.anchor_max);
            let pivot = Vec2::from_array(rect.pivot);
            let anchored = Vec2::from_array(rect.anchored_position);
            let pivot_position = layout_pos[world.len()].unwrap_or(
                parent_size * (anchor_min + (anchor_max - anchor_min) * pivot - parent_pivot)
                    + anchored,
            );
            // Sidecar transforms are X-mirrored (Bevy space): canvas-local X runs the other way.
            local.translation.x = -pivot_position.x;
            local.translation.y = pivot_position.y;
        }
        let parent = n.parent.map_or(Mat4::IDENTITY, |p| {
            world[p].compute_transform().compute_matrix()
        });
        world.push(GlobalTransform::from(parent * local.compute_matrix()));
    }
    let lobby_spawns: Vec<Transform> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.path == "Managers/Menu/Spawns/Spawn")
        .map(|(i, _)| {
            let t = world[i].compute_transform();
            Transform::from_translation(t.translation + Vec3::Y * 0.5).with_rotation(t.rotation)
        })
        .collect();
    let editor_spawn = nodes.iter().position(|n| n.path == "Managers/Menu/Costume Menu/CameraTarget").map(|i| {
        let t = world[i].compute_transform();
        let o = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        Transform::from_translation(t.translation + Vec3::new(o("GB_EDITOR_DX", 0.0), o("GB_EDITOR_DY", -0.6), o("GB_EDITOR_DZ", 0.0)))
            .with_rotation(Quat::from_rotation_y(o("GB_EDITOR_YAW", 1.57)))
    });
    let mut item_keys = HashMap::new();
    let mut screens = HashMap::new();
    for key in [SPLASH, MAIN, SETTINGS, COSTUME_EDITOR, LOBBY] {
        let Some(s) = nodes.iter().position(|n| n.path == key) else {
            continue;
        };
        let data = script(s, "BaseMenuScreen").cloned().unwrap_or_default();
        let target = data["cameraTarget"]["node"]
            .as_u64()
            .map(|t| t as usize)
            .unwrap_or(s);
        let off = &data["cameraOffset"];
        let offset = Vec3::new(
            -(off["x"].as_f64().unwrap_or(0.0) as f32),
            off["y"].as_f64().unwrap_or(0.0) as f32,
            off["z"].as_f64().unwrap_or(-7.0) as f32,
        );
        let target_transform = world[target].compute_transform();
        // TextMeshProUGUI belongs to a Unity Canvas. Convert its authored RectTransform
        // position to canvas pixels once, then draw it in Bevy's 2D UI layer. That keeps the
        // labels fixed while the Alley camera glides between its source targets.
        let mut canvas = s;
        while !nodes[canvas].components.iter().any(|c| c.kind == "Canvas") {
            let Some(parent) = nodes[canvas].parent else {
                break;
            };
            canvas = parent;
        }
        let canvas_size = sizes[canvas].max(Vec2::splat(1.0));
        let canvas_transform = world[canvas].compute_transform();
        let canvas_inverse = canvas_transform.compute_matrix().inverse();
        // Walk the active part of the screen (inactive prompts / sub-screens stay hidden).
        let mut stack = vec![(s, None::<usize>)];
        let mut items = vec![];
        let mut texts = vec![];
        while let Some((i, item)) = stack.pop() {
            // Network and wireless entry points are intentionally out of scope; don't leave
            // selectable dead ends in the offline-only build.
            if key == MAIN
                && (nodes[i].path.ends_with("/Online") || nodes[i].path.ends_with("/Wireless"))
            {
                continue;
            }
            if script(i, "DisabledIfPlatform")
                .is_some_and(|data| data["OnSteam"].as_i64() == Some(1))
            {
                continue;
            }
            // The Splash screen is authored in its loading state (LoadingPlatform active,
            // PressAnyKey inactive). The real title screen shows the authored "Press a Button"
            // prompt once loading finishes, so for the splash we render PressAnyKey and hide
            // the loading overlays instead of the transient loading state.
            let splash_override = key == SPLASH
                && (nodes[i].path.ends_with("/PressAnyKey")
                    || nodes[i].path.ends_with("/LoadingPlatform")
                    || nodes[i].path.ends_with("/LoadingUser"));
            let active = if splash_override {
                // Keep both authored splash states; the layout shows the one that matches
                // whether a match is currently loading.
                nodes[i].path.ends_with("/PressAnyKey")
                    || nodes[i].path.ends_with("/LoadingPlatform")
            } else {
                nodes[i].active
            };
            if i != s && !active {
                continue;
            }
            let item = if is_item(i) {
                items.push(i);
                Some(i)
            } else {
                item
            };
            if let Some(t) = script(i, "TextMeshProUGUI") {
                let loc_key = script(i, "LocalizeStringEvent")
                    .and_then(|l| l["m_StringReference"]["m_TableEntryReference"]["m_Key"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| strip_key(t["m_text"].as_str().unwrap_or("")));
                let text = strings
                    .get(&loc_key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| strip_key(t["m_text"].as_str().unwrap_or("")));
                if let Some(it) = item {
                    item_keys.entry(it).or_insert(loc_key.clone());
                }
                let auto = t["m_enableAutoSizing"].as_i64() == Some(1);
                let size = if auto {
                    t["m_fontSizeMax"].as_f64()
                } else {
                    t["m_fontSize"].as_f64()
                }
                .unwrap_or(36.0) as f32;
                // Unconverted TMP components keep the authoritative legacy m_textAlignment
                // (65535 = unset, then m_HorizontalAlignment applies).
                let legacy = t["m_textAlignment"].as_i64().unwrap_or(65535);
                let horizontal = if legacy != 65535 {
                    legacy & 0xff
                } else {
                    t["m_HorizontalAlignment"].as_i64().unwrap_or(2)
                };
                let align = match horizontal {
                    1 => JustifyText::Left,
                    4 => JustifyText::Right,
                    _ => JustifyText::Center,
                };
                let c = &t["m_fontColor"];
                let f = |k: &str| c[k].as_f64().unwrap_or(1.0) as f32;
                texts.push(MenuText {
                    world: Some((world[i].translation(), world[i].to_scale_rotation_translation().0.y.abs())),
                    ui_position: {
                        let local = canvas_inverse.transform_point3(world[i].translation());
                        Vec2::new(local.x, local.y)
                    },
                    text,
                    size,
                    align,
                    width: sizes[i].x,
                    item,
                    color: Color::srgba(f("r"), f("g"), f("b"), f("a")),
                    // The splash's LoadingPlatform subtree is the OG loading screen; its labels
                    // (e.g. "Loading") only show while a match is starting. The text is a child
                    // of LoadingPlatform, so match the subtree, not the node itself.
                    loading: key == SPLASH
                        && (nodes[i].path.contains("LoadingPlatform")
                            || nodes[i].path.contains("LoadingUser")),
                });
            }
            for &c in children.get(&i).into_iter().flatten().rev() {
                stack.push((c, item));
            }
        }
        // Lobby tooltips, matching the reference capture (lobby_colour_1080.png): the colour
        // selector hint with arrows sits top-right, and the button prompts sit bottom-right in
        // the same yellow as the source UI.
        if key == LOBBY {
            let prompt = Color::srgb(1.0, 0.83, 0.09);
            let mut hint = |text: &str, position: Vec2, size: f32| {
                texts.push(MenuText {
                    world: None,
                    ui_position: position,
                    text: text.to_string(),
                    size,
                    align: JustifyText::Center,
                    width: canvas_size.x * 0.4,
                    item: None,
                    color: prompt,
                    loading: false,
                });
            };
            // Controls help sits bottom-left, clear of the option rows and the SUBMIT / BACK prompts on the right.
            hint("< >  or  Q E :  colour", Vec2::new(-canvas_size.x * 0.30, -canvas_size.y * 0.30), 30.0);
            hint("Z X :  costume", Vec2::new(-canvas_size.x * 0.30, -canvas_size.y * 0.36), 30.0);
            hint("B  /  Shift B :  add / remove AI", Vec2::new(-canvas_size.x * 0.30, -canvas_size.y * 0.42), 30.0);
            hint("SUBMIT", Vec2::new(canvas_size.x * 0.36, -canvas_size.y * 0.40), 40.0);
            hint("BACK", Vec2::new(canvas_size.x * 0.36, -canvas_size.y * 0.46), 40.0);
        }
        if key == COSTUME_EDITOR {
            items.push(COSTUME_PRESET_ITEM);
            item_keys.insert(COSTUME_PRESET_ITEM, "MENU_COSTUME_PRESET".to_string());
            texts.push(MenuText {
                world: None,
                ui_position: Vec2::new(-canvas_size.x * 0.28, -canvas_size.y * 0.30),
                text: "Costume".to_string(),
                size: 42.0,
                align: JustifyText::Center,
                width: canvas_size.x * 0.45,
                item: Some(COSTUME_PRESET_ITEM),
                color: palette.first().map_or(Color::WHITE, |(_, color)| *color),
                loading: false,
            });
            items.push(COSTUME_COLOR_ITEM);
            item_keys.insert(COSTUME_COLOR_ITEM, "MENU_COSTUME_COLOR".to_string());
            let color = palette.first().map_or(Color::WHITE, |(_, color)| *color);
            texts.push(MenuText {
                world: None,
                ui_position: Vec2::new(-canvas_size.x * 0.28, -canvas_size.y * 0.38),
                text: "Color".to_string(),
                size: 42.0,
                align: JustifyText::Center,
                width: canvas_size.x * 0.45,
                item: Some(COSTUME_COLOR_ITEM),
                color,
                loading: false,
            });
        }
        // The source CinemachineTransposer uses LockToTargetWithWorldUp and a one-second
        // position damping value. The Composer centers its LookAt target, which is the
        // BaseMenuScreen cameraTarget in this scene.
        let camera = Transform::from_translation(
            target_transform.translation + target_transform.rotation * offset,
        )
        .looking_at(target_transform.translation, Vec3::Y);
        let mut nav = HashMap::new();
        for &it in &items {
            if it == COSTUME_COLOR_ITEM {
                nav.insert(it, (Some(COSTUME_PRESET_ITEM), None));
                continue;
            }
            if it == COSTUME_PRESET_ITEM {
                nav.insert(it, (None, Some(COSTUME_COLOR_ITEM)));
                continue;
            }
            let n = script(it, "Button")
                .or_else(|| {
                    nodes[it]
                        .components
                        .iter()
                        .find(|c| {
                            c.script
                                .as_deref()
                                .is_some_and(|s| s.starts_with("MenuHandler"))
                        })
                        .map(|c| &c.data)
                })
                .map(|d| &d["m_Navigation"]);
            let link = |k: &str| {
                n.and_then(|n| n[k]["node"].as_u64())
                    .map(|x| x as usize)
                    .filter(|x| items.contains(x))
            };
            nav.insert(it, (link("m_SelectOnUp"), link("m_SelectOnDown")));
        }
        let default = if key == COSTUME_EDITOR {
            Some(COSTUME_COLOR_ITEM)
        } else {
            data["defaultSelection"]["node"]
                .as_u64()
                .map(|x| x as usize)
                .filter(|x| items.contains(x))
                .or(items.first().copied())
        };
        screens.insert(
            key,
            Screen {
                camera,
                canvas_size,
                items,
                nav,
                default,
                texts,
            },
        );
    }
    info!(
        "menu loaded: splash={} labels, main={} labels, lobby={} labels, cameras={}",
        screens.get(SPLASH).map_or(0, |s| s.texts.len()),
        screens.get(MAIN).map_or(0, |s| s.texts.len()),
        screens.get(LOBBY).map_or(0, |s| s.texts.len()),
        cams.iter().count()
    );
    for cam in &cams {
        commands.entity(cam).remove::<crate::FlyCam>();
        // The source Splash canvas is only a loading overlay; its press prompt is inactive in the
        // exported scene. Start on the actual title menu once this scene has finished loading.
        if let Some(s) = screens.get(MAIN) {
            commands.entity(cam).insert(s.camera);
        }
    }
    // Unity's costume editor instantiates its actor prefab at this scene-authored point.
    // The actor prefab itself is exported separately as beast.glb, so place that render rig
    // into the same Menu Alley room instead of inventing a flat character card.
    if let Some(spawn) = nodes
        .iter()
        .position(|node| node.path.ends_with("/Actor Spawn Location"))
        .map(|index| world[index].compute_transform())
    {
        commands.spawn((
            SceneRoot(assets.load(bevy::gltf::GltfAssetLabel::Scene(0).from_asset("beast.glb"))),
            spawn,
        ));
    } else {
        warn!("menu: costume actor spawn point was not exported");
    }
    // The costume editor's own key light lives under the inactive `Costume Customization`
    // subtree, so the scene loader skips it and the actor renders unlit. Spawn the authored
    // spotlights from that subtree so the costume area is lit like the real game.
    for (i, node) in nodes.iter().enumerate() {
        if !node.path.contains("/Costume Customization/") {
            continue;
        }
        for component in &node.components {
            if component.kind != "Light" || component.data["m_Enabled"].as_u64() != Some(1) {
                continue;
            }
            let d = &component.data;
            let c = &d["m_Color"];
            let color = Color::srgb(
                c["r"].as_f64().unwrap_or(1.0) as f32,
                c["g"].as_f64().unwrap_or(1.0) as f32,
                c["b"].as_f64().unwrap_or(1.0) as f32,
            );
            let intensity = d["m_Intensity"].as_f64().unwrap_or(1.0) as f32 * 500.0;
            let range = d["m_Range"].as_f64().unwrap_or(10.0) as f32;
            let transform = crate::scene::unity_light_to_bevy(world[i].compute_transform());
            match d["m_Type"].as_u64() {
                Some(0) => {
                    commands.spawn((
                        SpotLight {
                            color,
                            intensity,
                            range,
                            inner_angle: (d["m_InnerSpotAngle"].as_f64().unwrap_or(21.8) as f32
                                * 0.5)
                                .to_radians(),
                            outer_angle: (d["m_SpotAngle"].as_f64().unwrap_or(30.0) as f32 * 0.5)
                                .to_radians(),
                            shadows_enabled: d["m_Shadows"]["m_Type"].as_u64().unwrap_or(0) > 0,
                            ..default()
                        },
                        transform,
                    ));
                }
                Some(2) | Some(3) => {
                    commands.spawn((
                        PointLight { color, intensity, range, ..default() },
                        transform,
                    ));
                }
                _ => {}
            }
        }
    }
    let settings_camera = screens.get(SETTINGS).map_or(Transform::IDENTITY, |s| s.camera);
    screens.insert(SETTINGS, option_screen(settings_camera, &[
        ("", "Settings"), ("SETTINGS_GRAPHICS", "Graphics"),
        ("SETTINGS_AUDIO", "Audio"), ("SETTINGS_INPUT", "Controls"), ("SETTINGS_RESET", "Reset settings"),
        ("BACK_MAIN", "Back")], &mut item_keys));
    screens.insert(GRAPHICS, option_screen_spaced(settings_camera, &[
        ("", "Graphics"), ("SETTINGS_GRAPHICS_RESOLUTION", "Resolution"),
        ("SETTINGS_GRAPHICS_SCREENMODE", "Mode"), ("SETTINGS_GRAPHICS_VSYNC", "V-sync"),
        ("SETTINGS_GRAPHICS_AA", "Anti-aliasing"),
        ("SETTINGS_GRAPHICS_ANISOTROPIC_FILTER", "Anisotropic Filter"),
        ("SETTINGS_GRAPHICS_QUALITY", "Quality"),
        ("SETTINGS_GRAPHICS_TEXTURE_QUALITY", "Texture Quality"),
        ("SETTINGS_GRAPHICS_SHADOWS", "Shadows"),
        ("SETTINGS_GRAPHICS_AMBIENT_OCCLUSION", "Ambient Occlusion"),
        ("SETTINGS_GRAPHICS_SCREEN_SPACE_REFLECTIONS", "Screen Space Reflection"),
        ("SETTINGS_GRAPHICS_BLOOM", "Bloom"),
        ("SETTINGS_GRAPHICS_DEPTH_OF_FIELD", "Depth of Field"),
        ("SETTINGS_GRAPHICS_CHROMATIC_ABERRATION", "Chromatic Aberration"),
        ("SETTINGS_GRAPHICS_GRAIN", "Grain"),
        ("SETTINGS_GRAPHICS_VIGNETTE", "Vignette"),
        ("BACK_SETTINGS", "Back")], &mut item_keys, 340.0, 46.0, 32.0));
    screens.insert(AUDIO, option_screen(settings_camera, &[
        ("", "Audio"), ("SETTINGS_AUDIO_MASTER", "Master"), ("SETTINGS_AUDIO_MUSIC", "Music"),
        ("SETTINGS_AUDIO_SFX", "Effects"), ("BACK_SETTINGS", "Back")], &mut item_keys));
    screens.insert(CONTROLS, option_screen(settings_camera, &[
        ("", "Controls"), ("", "WASD / arrows: move"),
        ("", "Space: jump   Shift: lift"), ("", "C / Ctrl: duck   F: kick"),
        ("", "Mouse buttons: punch / hold to grab"),
        ("", "Controller: left stick + face / shoulder buttons"),
        ("BACK_SETTINGS", "Back")], &mut item_keys));
    let main_camera = screens.get(MAIN).map_or(Transform::IDENTITY, |s| s.camera);
    screens.insert(CREDITS, option_screen(main_camera, &[
        ("", "Gang Beasts"), ("", "Original game by Boneloaf"),
        ("", "Rust reimplementation using local game assets"),
        ("BACK_MAIN", "Back")], &mut item_keys));
    // Only the unlocked palette is implemented so far; don't expose inert outfit controls.
    if let Some(s) = screens.get_mut(COSTUME_EDITOR) {
        s.items.retain(|id| *id == COSTUME_COLOR_ITEM || *id == COSTUME_PRESET_ITEM);
        s.texts.retain(|t| t.item == Some(COSTUME_COLOR_ITEM) || t.item == Some(COSTUME_PRESET_ITEM));
        let back_id = usize::MAX - 1 - item_keys.len();
        item_keys.insert(back_id, "BACK_MAIN".into());
        s.items.push(back_id);
        s.texts.push(MenuText { world: None, ui_position: Vec2::new(0.0, -450.0), text: "Back".into(),
            size: 40.0, align: JustifyText::Center, width: 400.0,
            item: Some(back_id), color: NORMAL, loading: false });
    }
    let start = match std::env::var("GB_MENU_SCREEN").as_deref() {
        Ok("settings") => SETTINGS, Ok("audio") => AUDIO, Ok("graphics") => GRAPHICS, Ok("controls") => CONTROLS,
        Ok("costumes") => COSTUME_EDITOR, Ok("lobby") => LOBBY, Ok("credits") => CREDITS,
        Ok("splash") => SPLASH,
        _ => MAIN,
    };
    for cam in &cams {
        if let Some(s) = screens.get(start) { commands.entity(cam).insert(s.camera); }
    }
    let selected = screens.get(start).and_then(|s| s.default);
    commands.insert_resource(Menu {
        joined: 0,
        host: None,
        lobby_options: false,
        lobby_spawns,
        editor_spawn,
        editor_actor: None,
        editor_costume: String::new(),
        screens,
        current: start,
        selected,
        wins: prefs["wins"].as_u64().map_or(3, |w| (w as u32).clamp(1, 10)),
        // `stages.len()` is the "Random" entry.
        stage_index: if prefs["stage"].as_str() == Some("random") {
            stages.len()
        } else {
            prefs["stage"]
                .as_str()
                .and_then(|name| stages.iter().position(|s| s == name))
                .or_else(|| stages.iter().position(|s| s == "rooftop"))
                .unwrap_or(0)
        },
        mode: match std::env::var("GB_MODE") {
            Ok(id) => crate::round::Mode::from_id(&id),
            Err(_) => crate::round::Mode::from_id(prefs["mode"].as_str().unwrap_or("")),
        },
        bots: std::env::var("GB_BOTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| prefs["bots"].as_u64().map_or(0, |b| (b as usize).min(5))),
        stages,
        palette,
        player_color: if std::env::var_os("GB_COLOUR").is_some() {
            env_index("GB_COLOUR", 0)
        } else {
            prefs["colour"].as_u64().unwrap_or(0) as usize
        }
        .min(palette_len.saturating_sub(1)),
        item_keys,
        root: root.0.clone(),
        vol_master: prefs["vol_master"].as_u64().map_or(DEFAULT_VOL.0, |v| v.min(10) as u32),
        vol_music: prefs["vol_music"].as_u64().map_or(DEFAULT_VOL.1, |v| v.min(10) as u32),
        vol_sfx: prefs["vol_sfx"].as_u64().map_or(DEFAULT_VOL.2, |v| v.min(10) as u32),
        dirty: true,
        resolution: 0,
        vsync: true,
        bloom: true,
        shadows: true,
        ao: true,
        // Graphics rows: defaults mirror the harness baselines (AA 4x, aniso 8x, high quality,
        // no screen-space extras). GB_SET_* presets them for captures/A-B.
        aa: env_index("GB_SET_AA", 2),
        aniso: env_index("GB_SET_ANISO", 3),
        quality: env_index("GB_SET_QUALITY", 2),
        texture_quality: env_index("GB_SET_TEXTURE_QUALITY", 2),
        ssr: env_index("GB_SET_SSR", 0) != 0,
        grain: env_index("GB_SET_GRAIN", 0) != 0,
        dof: env_index("GB_SET_DOF", 0),
        ca: env_index("GB_SET_CA", 0) != 0,
        vignette: env_index("GB_SET_VIGNETTE", 0) != 0,
        screen_mode: env_index("GB_SET_SCREENMODE", 0),
        graphics_dirty: true,
        pad_active: false,
        status: String::new(),
        launch: None,
        launch_args: Vec::new(),
    });
}

/// Pushes the graphics rows into the engine. Runs whenever a row changed (or once at startup,
/// which is how `GB_SET_*` presets take effect for captures).
///
/// Mapping (each row is a real engine switch):
///   Resolution      -> Window::resolution
///   Mode            -> Window::mode (windowed / borderless / fullscreen)
///   V-sync          -> Window::present_mode
///   Anti-aliasing   -> Msaa sample count (0/2x/4x/8x). SSAO forces MSAA off, like URP.
///   Anisotropic     -> sampler anisotropy on every loaded image
///   Quality         -> directional shadow map size + SSAO quality level
///   Texture Quality -> sampler filter/mipmap filter (trilinear vs nearest mips vs point)
///   Shadows         -> DirectionalLight::shadows_enabled
///   Ambient Occlusion -> ScreenSpaceAmbientOcclusion component
///   Screen Space Reflection -> material reflectance scale (stand-in: real SSR is not ported;
///                      it makes the specular response stronger, visible on lit edges/logo)
///   Bloom           -> camera Bloom intensity
///   Depth of Field  -> Bevy DepthOfField component (Gaussian / Bokeh)
///   Chromatic Aberration / Grain / Vignette -> urp_post uniforms (URP UberPost effects)
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn apply_graphics(
    menu: Option<ResMut<Menu>>,
    mut commands: Commands,
    mut windows: Query<&mut Window>,
    mut cameras: Query<(
        Entity,
        &mut bevy::core_pipeline::bloom::Bloom,
        Option<&AuthoredBloom>,
        Option<&mut crate::urp_post::UrpPost>,
    ), With<Camera3d>>,
    mut sun_lights: Query<&mut DirectionalLight>,
    mut shadow_map: ResMut<bevy::pbr::DirectionalLightShadowMap>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut vinyl_materials: ResMut<Assets<crate::vinyl::VinylMaterial>>,
    mut applied: Local<Option<GraphicsApplied>>,
) {
    let Some(mut menu) = menu else { return };
    if !menu.graphics_dirty {
        return;
    }
    menu.graphics_dirty = false;
    let snapshot = GraphicsApplied {
        aa: menu.aa,
        aniso: menu.aniso,
        quality: menu.quality,
        texture_quality: menu.texture_quality,
        ssr: menu.ssr,
        shadows: menu.shadows,
        ao: menu.ao,
        bloom: menu.bloom,
        dof: menu.dof,
        ca: menu.ca,
        grain: menu.grain,
        vignette: menu.vignette,
        resolution: menu.resolution,
        vsync: menu.vsync,
        screen_mode: menu.screen_mode,
    };
    let previous = applied.replace(snapshot);
    let prev = previous.unwrap_or(GraphicsApplied::defaults_before_first());
    // Window: resolution, v-sync, mode.
    if let Ok(mut w) = windows.single_mut() {
        let (width, height) = RESOLUTIONS[menu.resolution.min(RESOLUTIONS.len() - 1)];
        w.resolution.set(width as f32, height as f32);
        w.present_mode = if menu.vsync {
            bevy::window::PresentMode::AutoVsync
        } else {
            bevy::window::PresentMode::AutoNoVsync
        };
        w.mode = match menu.screen_mode {
            1 => bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Current),
            2 => bevy::window::WindowMode::Fullscreen(
                bevy::window::MonitorSelection::Current,
                bevy::window::VideoModeSelection::Current,
            ),
            _ => bevy::window::WindowMode::Windowed,
        };
    }
    // Samplers: anisotropic filtering and texture quality are sampler properties, so they apply
    // to every loaded image (Unity's texture quality likewise resamples the same textures).
    if prev.aniso != menu.aniso || prev.texture_quality != menu.texture_quality {
        let (mag, min, mip) = match menu.texture_quality {
            0 => (
                bevy::image::ImageFilterMode::Nearest,
                bevy::image::ImageFilterMode::Nearest,
                bevy::image::ImageFilterMode::Nearest,
            ),
            1 => (
                bevy::image::ImageFilterMode::Linear,
                bevy::image::ImageFilterMode::Linear,
                bevy::image::ImageFilterMode::Nearest,
            ),
            _ => (
                bevy::image::ImageFilterMode::Linear,
                bevy::image::ImageFilterMode::Linear,
                bevy::image::ImageFilterMode::Linear,
            ),
        };
        // wgpu requires all-linear filters when anisotropy > 1, so the low/medium texture
        // qualities (point / nearest-mip) also drop anisotropic filtering.
        let aniso = if menu.texture_quality < 2 {
            1
        } else {
            [1u16, 2, 4, 8, 16][menu.aniso.min(4)]
        };
        // Only filterable float textures can take a filtering sampler: the reflection
        // cubemaps are BC6H (non-filterable) and are sampled with their own layout.
        for (_, image) in images.iter_mut() {
            if !matches!(
                image.texture_descriptor.format.sample_type(None, None),
                Some(bevy::render::render_resource::TextureSampleType::Float { filterable: true })
            ) {
                continue;
            }
            // Keep everything else the texture already had — crucially the wrap mode. Tiling
            // surfaces (brick walls, ground) load with Repeat from their glTF sampler; replacing
            // the whole descriptor with a default one clamps them and the bricks break up.
            let filter_code = |m: bevy::image::ImageFilterMode| {
                u8::from(!matches!(m, bevy::image::ImageFilterMode::Nearest))
            };
            let matches_wanted = |d: &bevy::image::ImageSamplerDescriptor| {
                d.anisotropy_clamp == aniso
                    && filter_code(d.mag_filter) == filter_code(mag)
                    && filter_code(d.min_filter) == filter_code(min)
                    && filter_code(d.mipmap_filter) == filter_code(mip)
            };
            let mut descriptor = match &image.sampler {
                bevy::image::ImageSampler::Descriptor(descriptor) => {
                    if matches_wanted(descriptor) {
                        continue; // already matching: no re-upload
                    }
                    descriptor.clone()
                }
                // `Default` means "use ImagePlugin's default sampler" (aniso 8, linear filters,
                // clamp). Only replace it when the settings actually ask for something else.
                bevy::image::ImageSampler::Default => {
                    let plugin = bevy::image::ImageSamplerDescriptor {
                        anisotropy_clamp: 8,
                        mag_filter: bevy::image::ImageFilterMode::Linear,
                        min_filter: bevy::image::ImageFilterMode::Linear,
                        mipmap_filter: bevy::image::ImageFilterMode::Linear,
                        ..default()
                    };
                    if matches_wanted(&plugin) {
                        continue;
                    }
                    plugin
                }
            };
            descriptor.anisotropy_clamp = aniso;
            descriptor.mag_filter = mag;
            descriptor.min_filter = min;
            descriptor.mipmap_filter = mip;
            image.sampler = bevy::image::ImageSampler::Descriptor(descriptor);
        }
    }
    // Quality preset: shadow map resolution + SSAO quality level.
    if prev.quality != menu.quality {
        shadow_map.size = [1024, 2048, 2048, 4096][menu.quality.min(3)];
    }
    // Reflections stand-in: scale the specular response of every material (both plain
    // StandardMaterial and the converted VinylOrMetal that the menu scenery uses).
    if prev.ssr != menu.ssr {
        let scale = if menu.ssr { 1.6 } else { 1.0 / 1.6 };
        for (_, material) in materials.iter_mut() {
            // Reflectance maps to F0 quadratically in Bevy; scaling the base value is the
            // closest single knob to "more/less visible reflections".
            material.reflectance = (material.reflectance * scale).clamp(0.0, 1.0);
        }
        crate::vinyl::scale_vinyl_reflectance(&mut vinyl_materials, scale);
    }
    // Realtime shadows.
    if prev.shadows != menu.shadows {
        for mut light in &mut sun_lights {
            light.shadows_enabled = menu.shadows;
        }
    }
    let msaa = match menu.aa {
        0 => Msaa::Off,
        1 => Msaa::Sample2,
        3 => Msaa::Sample8,
        _ => Msaa::Sample4,
    };
    let ao_quality = match menu.quality {
        0 => bevy::pbr::ScreenSpaceAmbientOcclusionQualityLevel::Low,
        1 => bevy::pbr::ScreenSpaceAmbientOcclusionQualityLevel::Medium,
        _ => bevy::pbr::ScreenSpaceAmbientOcclusionQualityLevel::High,
    };
    for (entity, mut bloom, authored_bloom, urp) in &mut cameras {
        // Bloom: toggle the camera's authored (stage volume) intensity; the menu's tuned value
        // must survive an off/on cycle.
        let authored = authored_bloom.map_or(bloom.intensity, |b| b.0);
        bloom.intensity = if menu.bloom { authored } else { 0.0 };
        if let Some(mut urp) = urp {
            urp.ca = if menu.ca { 0.6 } else { 0.0 };
            urp.vignette = if menu.vignette { 0.35 } else { 0.0 };
            urp.grain = if menu.grain { 0.5 } else { 0.0 };
        }
        // Ambient occlusion needs MSAA off, exactly like URP.
        if menu.ao && std::env::var_os("GB_NO_SSAO").is_none() {
            commands.entity(entity).insert((
                bevy::pbr::ScreenSpaceAmbientOcclusion {
                    quality_level: ao_quality,
                    constant_object_thickness: crate::ssao_thickness("menu"),
                    ..default()
                },
                Msaa::Off,
            ));
        } else {
            commands
                .entity(entity)
                .remove::<bevy::pbr::ScreenSpaceAmbientOcclusion>()
                .insert(msaa);
        }
        // Depth of field (Bevy's URP-equivalent post effect).
        if menu.dof == 0 {
            commands.entity(entity).remove::<bevy::core_pipeline::dof::DepthOfField>();
        } else {
            use bevy::core_pipeline::dof::{DepthOfField, DepthOfFieldMode};
            commands.entity(entity).insert(DepthOfField {
                mode: if menu.dof == 1 {
                    DepthOfFieldMode::Gaussian
                } else {
                    DepthOfFieldMode::Bokeh
                },
                focal_distance: 8.0,
                ..default()
            });
        }
    }
}

/// Animates the film grain seed (URP's grain is animated; a static pattern reads as dirt).
fn animate_grain(
    menu: Option<Res<Menu>>,
    time: Res<Time>,
    mut cameras: Query<&mut crate::urp_post::UrpPost, With<Camera3d>>,
) {
    let Some(menu) = menu else { return };
    if !menu.grain {
        return;
    }
    for mut urp in &mut cameras {
        if urp.grain > 0.0 {
            urp.grain_time = time.elapsed_secs() * 60.0;
        }
    }
}

/// Harness knobs for the lobby cyclers, so a capture can verify switching without a human at the
/// keyboard: `GB_CYCLE_COLOUR=<frames>` advances the palette swatch, `GB_CYCLE_COSTUME=<frames>`
/// advances the costume preset. Both count Update frames.
fn cycle_for_capture(
    mut menu: Option<ResMut<Menu>>,
    mut costumes: Option<ResMut<crate::costume::Costumes>>,
    mut frames: Local<u32>,
) {
    let (colour_every, costume_every) = (
        std::env::var("GB_CYCLE_COLOUR").ok().and_then(|v| v.parse::<u32>().ok()),
        std::env::var("GB_CYCLE_COSTUME").ok().and_then(|v| v.parse::<u32>().ok()),
    );
    if colour_every.is_none() && costume_every.is_none() {
        return;
    }
    *frames += 1;
    let n = *frames;
    if let Some(every) = colour_every {
        if every > 0 && n % every == 0 {
            if let Some(menu) = menu.as_mut() {
                if !menu.palette.is_empty() {
                    menu.player_color = (menu.player_color + 1) % menu.palette.len();
                    menu.dirty = true;
                    info!("capture: palette swatch -> {}", menu.player_color);
                }
            }
        }
    }
    if let Some(every) = costume_every {
        if every > 0 && n % every == 0 {
            if let Some(costumes) = costumes.as_mut() {
                if costumes.cycle_preset(1) {
                    if let Some(menu) = menu.as_mut() {
                        menu.dirty = true;
                    }
                    info!("capture: costume -> {:?}", costumes.chosen_name());
                }
            }
        }
    }
}

/// Snapshot of the applied graphics rows, for change detection in `apply_graphics`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct GraphicsApplied {
    aa: usize,
    aniso: usize,
    quality: usize,
    texture_quality: usize,
    ssr: bool,
    shadows: bool,
    ao: bool,
    bloom: bool,
    dof: usize,
    ca: bool,
    grain: bool,
    vignette: bool,
    resolution: usize,
    vsync: bool,
    screen_mode: usize,
}

impl GraphicsApplied {
    /// State before the first apply: forces every branch (samplers, shadow map, reflectance).
    fn defaults_before_first() -> Self {
        Self {
            aa: usize::MAX,
            aniso: usize::MAX,
            quality: usize::MAX,
            texture_quality: usize::MAX,
            ssr: false,
            shadows: true,
            ao: true,
            bloom: true,
            dof: usize::MAX,
            ca: false,
            grain: false,
            vignette: false,
            resolution: usize::MAX,
            vsync: true,
            screen_mode: 0,
        }
    }
}

/// `usize` env override (`GB_SET_<ROW>`), used to preset graphics rows for captures.
fn env_index(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

#[derive(Default)]
struct Prev {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    ok: bool,
    back: bool,
}

fn input(
    menu: Option<ResMut<Menu>>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut prev: Local<Prev>,
    mut exit: EventWriter<AppExit>,
    pointers: Query<(&MenuLabel, &Interaction), Changed<Interaction>>,
    pad_entities: Query<(Entity, &Gamepad)>,
    mut sim: Option<NonSendMut<crate::play::Sim>>,
    mut costumes: Option<ResMut<crate::costume::Costumes>>,
) {
    let Some(mut menu) = menu else { return };
    // Which device is driving the lobby prompts: any pad press wins, any key press returns to
    // keyboard. Rebuild the labels when it flips so the glyphs swap.
    let pad_used = pads.iter().any(|g| {
        g.just_pressed(GamepadButton::South)
            || g.just_pressed(GamepadButton::East)
            || g.just_pressed(GamepadButton::Start)
            || g.just_pressed(GamepadButton::DPadLeft)
            || g.just_pressed(GamepadButton::DPadRight)
    });
    if pad_used && !menu.pad_active {
        menu.pad_active = true;
        menu.dirty = true;
    } else if keys.get_just_pressed().next().is_some() && menu.pad_active {
        menu.pad_active = false;
        menu.dirty = true;
    }
    // OG loading screen: hold the splash for a moment, then hand off to the match process.
    if let Some(t) = menu.launch {
        let t = t - time.delta_secs();
        if t <= 0.0 {
            menu.launch = None;
            let args = std::mem::take(&mut menu.launch_args);
            match std::env::current_exe().map(|exe| std::process::Command::new(exe).args(&args).spawn()) {
                Ok(Ok(_)) => { exit.write(AppExit::Success); }
                Ok(Err(e)) => {
                    menu.status = format!("Could not start Rooftop: {e}");
                    error!("{}", menu.status);
                    menu.go(LOBBY);
                }
                Err(e) => {
                    menu.status = format!("Could not find the game executable: {e}");
                    error!("{}", menu.status);
                    menu.go(LOBBY);
                }
            }
        } else {
            menu.launch = Some(t);
        }
        return;
    }
    // Lobby (BeastMenuSpawner): A / Space joins (spawns your beast), A again readies up (arms
    // raised), B unreadies, B again backs out. Beasts can't move here. Once the host (first to
    // join) is ready the match options show and the host drives them.
    // Costume editor: a real beast (so the costume system can dress it) stands in the editor while the screen is open.
    if menu.current == COSTUME_EDITOR {
        if menu.editor_actor.is_none() {
            if let (Some(sim), Some(at)) = (sim.as_mut(), menu.editor_spawn) {
                let joined = match sim.parked.iter().position(|p| *p) {
                    Some(k) => {
                        sim.unpark_beast(k, at);
                        Ok(k)
                    }
                    None => sim.spawn_beast(at),
                };
                match joined {
                    Ok(k) => menu.editor_actor = Some(k),
                    Err(e) => error!("costume editor beast: {e}"),
                }
            }
        }
    } else if let Some(k) = menu.editor_actor.take() {
        if let Some(sim) = sim.as_mut() {
            if k < sim.parked.len() {
                sim.park_beast(k);
            }
        }
    }
    if let (Some(actor), Some(costumes)) = (menu.editor_actor, costumes.as_ref()) {
        let name = costumes.worn_for(actor).unwrap_or_default();
        if name != menu.editor_costume {
            menu.editor_costume = name;
            menu.dirty = true;
        }
    }
    let in_lobby = menu.current == LOBBY;
    // Captured before the lobby block consumes `sim`: the costume switch needs to know whether a
    // beast has spawned yet (it may switch readied or not, but not before joining).
    let beast_spawned = sim.as_ref().is_some_and(|s| !s.actors.is_empty());
    let mut lobby_back_used = false;
    let mut host_ok = false;
    if in_lobby {
        if let Some(ref mut sim) = sim {
            let mut a: Vec<Option<Entity>> = vec![];
            let mut b: Vec<Option<Entity>> = vec![];
            if keys.any_just_pressed([KeyCode::Space, KeyCode::Enter]) {
                a.push(None);
            }
            if keys.any_just_pressed([KeyCode::Escape, KeyCode::ControlLeft, KeyCode::KeyC, KeyCode::Backspace]) {
                b.push(None);
            }
            for (e, g) in &pad_entities {
                if g.just_pressed(GamepadButton::South) || g.just_pressed(GamepadButton::Start) {
                    a.push(Some(e));
                }
                if g.just_pressed(GamepadButton::East) {
                    b.push(Some(e));
                }
            }
            // Debug: GB_LOBBY_AUTOJOIN=N joins N placeholder players (captures/tests);
            // GB_LOBBY_AUTOREADY also readies them.
            if sim.device_actor.is_empty() && sim.actors.is_empty() {
                if let Some(n) = std::env::var("GB_LOBBY_AUTOJOIN").ok().and_then(|v| v.parse::<u32>().ok()) {
                    let devices: Vec<Option<Entity>> = std::iter::once(None)
                        .chain((1..n).map(|i| Some(Entity::from_raw(100_000 + i))))
                        .collect();
                    a.extend(devices.iter().copied());
                    if std::env::var_os("GB_LOBBY_AUTOREADY").is_some() {
                        a.extend(devices);
                    }
                }
            }
            for device in a {
                match sim.device_actor.get(&device).copied() {
                    None => {
                        if menu.lobby_spawns.is_empty() {
                            continue;
                        }
                        let joined = match sim.parked.iter().position(|p| *p) {
                            Some(k) => {
                                sim.unpark_beast(k, menu.lobby_spawns[k % menu.lobby_spawns.len()]);
                                Ok(k)
                            }
                            None => {
                                let k = sim.actors.len();
                                sim.spawn_beast(menu.lobby_spawns[k % menu.lobby_spawns.len()])
                            }
                        };
                        match joined {
                            Ok(k) => {
                                sim.device_actor.insert(device, k);
                                if menu.host.is_none() {
                                    menu.host = Some(device);
                                }
                                info!("lobby: player {} joined", k + 1);
                            }
                            Err(e) => error!("lobby join failed: {e}"),
                        }
                    }
                    Some(k) if !sim.lobby_ready[k] => sim.lobby_ready[k] = true,
                    // Already ready: the host's A confirms the selected option.
                    Some(_) => host_ok |= menu.host == Some(device),
                }
            }
            for device in b {
                match sim.device_actor.get(&device).copied() {
                    Some(k) if sim.lobby_ready[k] => {
                        sim.lobby_ready[k] = false;
                        lobby_back_used = true;
                    }
                    Some(k) => {
                        sim.park_beast(k);
                        sim.device_actor.remove(&device);
                        if menu.host == Some(device) {
                            menu.host = sim.device_actor.keys().next().copied();
                        }
                        lobby_back_used = true;
                        info!("lobby: player {} left", k + 1);
                    }
                    None => {}
                }
            }
            menu.joined = sim.device_actor.len();
            let options = menu
                .host
                .and_then(|h| sim.device_actor.get(&h))
                .is_some_and(|&k| sim.lobby_ready[k]);
            if options != menu.lobby_options {
                menu.dirty = true;
            }
            menu.lobby_options = options;
        }
    }
    let stick = pads
        .iter()
        .map(|g| g.left_stick())
        .fold(
            Vec2::ZERO,
            |a, b| if b.length() > a.length() { b } else { a },
        );
    let pad = |b: GamepadButton| pads.iter().any(|g| g.pressed(b));
    let now = Prev {
        up: keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp])
            || pad(GamepadButton::DPadUp)
            || stick.y > 0.6,
        down: keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown])
            || pad(GamepadButton::DPadDown)
            || stick.y < -0.6,
        left: keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft])
            || pad(GamepadButton::DPadLeft)
            || stick.x < -0.6,
        right: keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight])
            || pad(GamepadButton::DPadRight)
            || stick.x > 0.6,
        ok: keys.pressed(KeyCode::Enter)
            || pad(GamepadButton::Start)
            || (!in_lobby && (keys.pressed(KeyCode::Space) || pad(GamepadButton::South))),
        back: keys.pressed(KeyCode::Escape)
            || (!in_lobby
                && (keys.any_pressed([KeyCode::ControlLeft, KeyCode::KeyC]) || pad(GamepadButton::East))),
    };
    let edge = |a: bool, b: bool| a && !b;
    let (mut up, mut down, mut left, mut right, mut ok, mut back) = (
        edge(now.up, prev.up),
        edge(now.down, prev.down),
        edge(now.left, prev.left),
        edge(now.right, prev.right),
        edge(now.ok, prev.ok),
        edge(now.back, prev.back),
    );
    *prev = now;
    // The lobby masks left/right until the host is ready (below), which would also disable the
    // costume switch. Keep the raw edges for the lobby's own controls.
    let (lobby_left, lobby_right) = (left, right);
    if in_lobby {
        // Options take input only once the host is ready; A is the host's confirm, and B only
        // leaves the lobby when nobody is in it.
        let options = menu.lobby_options;
        up &= options;
        down &= options;
        left &= options;
        right &= options;
        ok = options && host_ok;
        back &= !lobby_back_used && menu.joined == 0;
    }
    let menu = &mut *menu;
    for (label, interaction) in &pointers {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) { continue; }
        if let Some(item) = menu.screens.get(menu.current)
            .and_then(|s| s.texts.get(label.0)).and_then(|t| t.item) {
            // Selection growth doesn't require rebuilding the UI under the pointer.
            menu.selected = Some(item);
            ok |= *interaction == Interaction::Pressed;
        }
    }
    let Some(screen) = menu.screens.get(menu.current) else {
        return;
    };
    if up || down {
        if let Some(sel) = menu.selected {
            let (u, d) = screen.nav.get(&sel).copied().unwrap_or((None, None));
            let idx = screen.items.iter().position(|&x| x == sel).unwrap_or(0);
            let n = screen.items.len().max(1);
            let mut next = if up {
                u.unwrap_or(screen.items[(idx + n - 1) % n])
            } else {
                d.unwrap_or(screen.items[(idx + 1) % n])
            };
            // Waves has no Wins row: step over it.
            if menu.current == LOBBY
                && menu.mode == crate::round::Mode::Waves
                && menu.item_keys.get(&next).map(String::as_str) == Some("MENU_WINS")
            {
                let (u2, d2) = screen.nav.get(&next).copied().unwrap_or((None, None));
                let at = screen.items.iter().position(|&x| x == next).unwrap_or(0);
                next = if up {
                    u2.unwrap_or(screen.items[(at + n - 1) % n])
                } else {
                    d2.unwrap_or(screen.items[(at + 1) % n])
                };
            }
            menu.selected = Some(next);
        }
    }
    let key = menu
        .selected
        .and_then(|s| menu.item_keys.get(&s))
        .cloned()
        .unwrap_or_default();
    if (left || right) && key == "MENU_WINS" {
        menu.wins = if right {
            menu.wins % 10 + 1
        } else if menu.wins <= 1 {
            10
        } else {
            menu.wins - 1
        };
        menu.dirty = true;
    }
    if (left || right) && key == "MENU_COSTUME_PRESET" {
        if let (Some(actor), Some(costumes)) = (menu.editor_actor, costumes.as_mut()) {
            if costumes.cycle_preset_for(actor, if right { 1 } else { -1 }) {
                menu.dirty = true;
            }
        }
    }
    if (left || right) && key == "MENU_COSTUME_COLOR" && !menu.palette.is_empty() {
        menu.player_color = if right {
            (menu.player_color + 1) % menu.palette.len()
        } else if menu.player_color == 0 {
            menu.palette.len() - 1
        } else {
            menu.player_color - 1
        };
        menu.dirty = true;
    }
    // Lobby colour: the reference prompt reads "◀ ▶ COLOUR", so left/right MUST cycle the colour
    // on the non-option rows; Q/E does the same (as requested). Costume cycling moves to
    // Shift+◀▶ so it is not lost. The wins and stage rows keep plain left/right for their own
    // values.
    let (color_back, color_forward) = (
        keys.just_pressed(KeyCode::KeyQ)
            || (lobby_left && !keys.pressed(KeyCode::ShiftLeft) && !keys.pressed(KeyCode::ShiftRight)),
        keys.just_pressed(KeyCode::KeyE)
            || (lobby_right && !keys.pressed(KeyCode::ShiftLeft) && !keys.pressed(KeyCode::ShiftRight)),
    );
    if menu.current == LOBBY
        && (color_back || color_forward)
        && key != "MENU_WINS"
        && key != "MENU_STAGE"
    {
        if !menu.palette.is_empty() {
            menu.player_color = if color_forward {
                (menu.player_color + 1) % menu.palette.len()
            } else if menu.player_color == 0 {
                menu.palette.len() - 1
            } else {
                menu.player_color - 1
            };
            if let Some(costumes) = costumes.as_mut() {
                costumes.redress = true;
            }
            menu.dirty = true;
        }
    }
    if (color_back || color_forward) && menu.current != LOBBY && !menu.palette.is_empty() {
        menu.player_color = if color_forward {
            (menu.player_color + 1) % menu.palette.len()
        } else if menu.player_color == 0 {
            menu.palette.len() - 1
        } else {
            menu.player_color - 1
        };
        menu.dirty = true;
    }
    if menu.current == LOBBY && (lobby_left || lobby_right) && key == "MENU_GAME_MODE" {
        let all = crate::round::Mode::ALL;
        let at = all.iter().position(|m| *m == menu.mode).unwrap_or(0);
        menu.mode = all[if lobby_right { (at + 1) % all.len() } else { (at + all.len() - 1) % all.len() }];
        if menu.stage_index < menu.stages.len() && !stage_allowed(menu.mode, &menu.stages[menu.stage_index]) {
            let mode = menu.mode;
            if let Some(i) = menu.stages.iter().position(|s| stage_allowed(mode, s)) {
                menu.stage_index = i;
            }
        }
        menu.dirty = true;
    }
    if menu.current == LOBBY && (lobby_left || lobby_right) && key == "MENU_STAGE" {
        // The stage row cycles the exported stages (the launch used to hardcode "rooftop").
        if !menu.stages.is_empty() {
            // Entries 0..n are the stages, n is "Random".
            let n = menu.stages.len() + 1;
            for _ in 0..n {
                menu.stage_index = if lobby_right {
                    (menu.stage_index + 1) % n
                } else if menu.stage_index == 0 {
                    n - 1
                } else {
                    menu.stage_index - 1
                };
                if menu.stage_index == n - 1 || stage_allowed(menu.mode, &menu.stages[menu.stage_index]) {
                    break;
                }
            }
            menu.dirty = true;
        }
    }
    if menu.current == LOBBY && keys.just_pressed(KeyCode::KeyB) {
        // B adds an AI player (wraps back to 0 after 5); Shift+B removes one.
        menu.bots = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { (menu.bots + 5) % 6 } else { (menu.bots + 1) % 6 };
        menu.dirty = true;
    }
    let mut costume_delta: isize = 0;
    if menu.current == LOBBY && (lobby_left || lobby_right) && key != "MENU_WINS" && key != "MENU_STAGE" && key != "MENU_GAME_MODE" {
        // Shift + left/right cycles the costume preset (plain left/right is the colour now).
        if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            costume_delta = if lobby_right { 1 } else { -1 };
        }
    }
    // Dedicated costume keys (Z / X, or the pad shoulders) need no Shift.
    if menu.current == LOBBY {
        if keys.just_pressed(KeyCode::KeyZ) || pads.iter().any(|g| g.just_pressed(GamepadButton::LeftTrigger)) {
            costume_delta = -1;
        }
        if keys.just_pressed(KeyCode::KeyX) || pads.iter().any(|g| g.just_pressed(GamepadButton::RightTrigger)) {
            costume_delta = 1;
        }
    }
    if costume_delta != 0 && beast_spawned {
        let actor = menu
            .host
            .and_then(|h| sim.as_ref().and_then(|s| s.device_actor.get(&h).copied()))
            .unwrap_or(0);
        if let Some(costumes) = costumes.as_mut() {
            if costumes.cycle_preset_for(actor, costume_delta) {
                menu.dirty = true;
            }
        }
    }
    if (left || right || ok) && menu.current == AUDIO {
        let step = |v: u32| if right || ok && !left { (v + 1).min(10) } else { v.saturating_sub(1) };
        match key.as_str() {
            "SETTINGS_AUDIO_MASTER" => menu.vol_master = step(menu.vol_master),
            "SETTINGS_AUDIO_MUSIC" => menu.vol_music = step(menu.vol_music),
            "SETTINGS_AUDIO_SFX" => menu.vol_sfx = step(menu.vol_sfx),
            _ => {}
        }
        save_prefs_audio(&menu);
        menu.dirty = true;
    }
    if (left || right || ok) && menu.current == GRAPHICS {
        let mut changed = true;
        let step = |v: usize, n: usize| if right { (v + 1) % n } else if left { (v + n - 1) % n } else { v };
        match key.as_str() {
            "SETTINGS_GRAPHICS_RESOLUTION" | "RESOLUTION" =>
                menu.resolution = (menu.resolution + if left { 2 } else { 1 }) % RESOLUTIONS.len(),
            "SETTINGS_GRAPHICS_SCREENMODE" | "SCREENMODE" => menu.screen_mode = step(menu.screen_mode, 3),
            "SETTINGS_GRAPHICS_VSYNC" | "VSYNC" => menu.vsync = !menu.vsync,
            "SETTINGS_GRAPHICS_AA" | "AA" => menu.aa = step(menu.aa, 4),
            "SETTINGS_GRAPHICS_ANISOTROPIC_FILTER" | "ANISOTROPIC" =>
                menu.aniso = step(menu.aniso, 5),
            "SETTINGS_GRAPHICS_QUALITY" | "QUALITY" => menu.quality = step(menu.quality, 4),
            "SETTINGS_GRAPHICS_TEXTURE_QUALITY" | "TEXTURE_QUALITY" =>
                menu.texture_quality = step(menu.texture_quality, 3),
            "SETTINGS_GRAPHICS_SHADOWS" | "SHADOWS" => menu.shadows = !menu.shadows,
            "SETTINGS_GRAPHICS_AMBIENT_OCCLUSION" | "AO" => menu.ao = !menu.ao,
            "SETTINGS_GRAPHICS_SCREEN_SPACE_REFLECTIONS" | "SSR" => menu.ssr = !menu.ssr,
            "SETTINGS_GRAPHICS_BLOOM" | "BLOOM" => menu.bloom = !menu.bloom,
            "SETTINGS_GRAPHICS_DEPTH_OF_FIELD" | "DOF" => menu.dof = step(menu.dof, 3),
            "SETTINGS_GRAPHICS_CHROMATIC_ABERRATION" | "CA" => menu.ca = !menu.ca,
            "SETTINGS_GRAPHICS_GRAIN" | "GRAIN" => menu.grain = !menu.grain,
            "SETTINGS_GRAPHICS_VIGNETTE" | "VIGNETTE" => menu.vignette = !menu.vignette,
            _ => changed = false,
        }
        if changed {
            menu.graphics_dirty = true;
        }
    }
    if ok {
        match key.as_str() {
            "BACK_MAIN" => menu.go(MAIN),
            "BACK_SETTINGS" => menu.go(SETTINGS),
            "SETTINGS_GRAPHICS" => menu.go(GRAPHICS),
            "SETTINGS_AUDIO" => menu.go(AUDIO),
            "SETTINGS_INPUT" => menu.go(CONTROLS),
            "SETTINGS_RESET" => {
                // Reset every graphics row to its default; `apply_graphics` pushes them out.
                menu.resolution = 0;
                menu.vsync = true;
                menu.bloom = true;
                menu.shadows = true;
                menu.ao = true;
                menu.aa = 2;
                menu.aniso = 3;
                menu.quality = 2;
                menu.texture_quality = 2;
                menu.ssr = false;
                menu.grain = false;
                menu.dof = 0;
                menu.ca = false;
                menu.vignette = false;
                menu.screen_mode = 0;
                menu.graphics_dirty = true;
            }
            _ => {}
        }
        match menu.current {
            SPLASH => menu.go(MAIN),
            MAIN => match key.as_str() {
                "MENU_MAIN_LOCAL" => menu.go(LOBBY),
                "MENU_MAIN_COSTUMES" => menu.go(COSTUME_EDITOR),
                // The export's key is MENU_MAIN_SETTING (singular). Matching MENU_MAIN_SETTINGS
                // (which only exists as part of MENU_SETTINGS_CHANGED) made the row a no-op —
                // this is why the settings screen "never opened".
                "MENU_MAIN_SETTING" | "MENU_SETTINGS" | "MENU_MAIN_SETTINGS" => menu.go(SETTINGS),
                "MENU_MAIN_CREDITS" => menu.go(CREDITS),
                "MENU_MAIN_QUIT" => {
                    exit.write(AppExit::Success);
                }
                _ => {}
            },
            LOBBY
                if key == "MENU_START"
                    || key == "MENU_WINS"
                    || key == "MENU_GAME_MODE"
                    || key == "MENU_STAGE" =>
            {
                if key == "MENU_START" {
                    // Local lobby: the keyboard plus every connected pad joins.
                    // Everyone who joined in the lobby plays (at least one fighter).
                    let players = menu.joined.clamp(1, 8);
                    // Show the OG loading screen first, then hand off to the match process.
                    let stage = match menu.stages.get(menu.stage_index) {
                        Some(s) => s.clone(),
                        None => {
                            // "Random": any stage this mode can be played on.
                            let mode = menu.mode;
                            let pool: Vec<&String> = menu.stages.iter().filter(|s| stage_allowed(mode, s)).collect();
                            let seed = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.subsec_nanos() as usize);
                            pool.get(seed % pool.len().max(1)).map_or_else(|| "rooftop".to_string(), |s| (*s).clone())
                        }
                    };
                    menu.launch_args = vec![
                        stage,
                        "--players".into(),
                        players.to_string(),
                        "--wins".into(),
                        menu.wins.to_string(),
                        "--mode".into(),
                        menu.mode.id().into(),
                        "--bots".into(),
                        menu.bots.to_string(),
                        "--player-color".into(),
                        menu.player_color.to_string(),
                        // The lobby's chosen costume must follow into the match, or the player
                        // spawns in a random preset.
                        "--costume".into(),
                        costumes
                            .as_ref()
                            .and_then(|c| {
                                let actor = menu
                                    .host
                                    .and_then(|h| sim.as_ref().and_then(|s| s.device_actor.get(&h).copied()))
                                    .unwrap_or(0);
                                c.chosen_for(actor)
                            })
                            .unwrap_or("none")
                            .to_string(),
                        "--costumes".into(),
                        (0..players as usize)
                            .map(|k| {
                                costumes
                                    .as_ref()
                                    .and_then(|c| c.worn_for(k))
                                    .unwrap_or_else(|| "none".to_string())
                            })
                            .collect::<Vec<_>>()
                            .join("|"),
                        "--assets".into(),
                        menu.root.to_string_lossy().into_owned(),
                    ];
                    save_prefs(&menu);
                    menu.launch = Some(1.6);
                    menu.go(SPLASH);
                }
            }
            _ => {}
        }
    }
    if back {
        match menu.current {
            LOBBY | SETTINGS | COSTUME_EDITOR | CREDITS => menu.go(MAIN),
            GRAPHICS | CONTROLS | AUDIO => menu.go(SETTINGS),
            MAIN => {}
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout(
    sim: Option<NonSendMut<crate::play::Sim>>,
    mut commands: Commands,
    menu: Option<ResMut<Menu>>,
    assets: Res<AssetServer>,
    time: Res<Time>,
    windows: Query<&Window>,
    icons: Query<Entity, With<PromptIcon>>,
    images: Res<Assets<Image>>,
    mut cams: Query<(&mut Transform, &mut Projection, &Camera, &GlobalTransform), With<Camera3d>>,
    mut labels: Query<(
        Entity,
        &MenuLabel,
        &mut Node,
        &mut Text,
        &mut TextFont,
        &mut TextColor,
        &mut Visibility,
    )>,
) {
    let Some(mut menu) = menu else { return };
    let Some(texts) = menu
        .screens
        .get(menu.current)
        .map(|screen| screen.texts.clone())
    else {
        return;
    };
    let Ok(window) = windows.single() else { return };
    let Ok((mut cam_tf, mut proj, camera, cam_global)) = cams.single_mut() else {
        warn!(
            "menu layout skipped: expected one 3D camera, found {}",
            cams.iter().count()
        );
        return;
    };
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = FOV_DEGREES.to_radians();
    }
    // Camera glides between screens.
    // Match the source CinemachineTransposer's 1.0 second position damping. Its composer
    // uses 0.5 second horizontal/vertical damping; aiming remains stable while translating.
    let k = 1.0 - (-time.delta_secs()).exp();
    let rotation_k = 1.0 - (-time.delta_secs() * 2.0).exp();
    let Some(screen_camera) = menu.screens.get(menu.current).map(|screen| screen.camera) else {
        return;
    };
    cam_tf.translation = cam_tf.translation.lerp(screen_camera.translation, k);
    cam_tf.rotation = cam_tf.rotation.slerp(screen_camera.rotation, rotation_k);
    // Lobby beasts move relative to the menu camera (Unity space for the sim).
    if let Some(mut sim) = sim {
        let flat = |v: Vec3| gb_phys::source::mirror_position(v.with_y(0.0).normalize_or_zero());
        sim.cam_right = flat(*cam_tf.right());
        sim.cam_forward = flat(*cam_tf.forward());
    }
    if menu.dirty {
        menu.dirty = false;
        for (e, ..) in &labels {
            commands.entity(e).despawn();
        }
        // The UI face: the source's Handel Gothic (HandelGotD-Bol) — the previous snasm face was
        // a placeholder and does not match the reference captures. GB_UI_FONT overrides.
        let font: Handle<Font> = assets.load(
            std::env::var("GB_UI_FONT").unwrap_or_else(|_| "ui/fonts/HandelGotD-Bol.otf".into()),
        );
        // Bind glyphs: the reference prompts use the game's own keyboard/gamepad icons
        // (exported from core-ui_assets by tools/extract/export_prompt_sprites.py). Spawn them
        // beside the lobby's SUBMIT/BACK labels; they are static UI, positioned per screen.
        for entity in &icons {
            commands.entity(entity).despawn();
        }
        if menu.current == LOBBY {
            let w = window.width();
            let h = window.height();
            // Only the ACTIVE input device's glyphs: keyboard icons unless a pad has been used.
            let pad = menu.pad_active;
            // Size each glyph by its own aspect (forcing both width and height stretched the
            // wide key-cap sprites). Height is the knob; width follows the image.
            let glyph = |path: &str, center_x: f32, center_y: f32, height: f32| {
                let handle: Handle<Image> = assets.load(path.to_string());
                let aspect = images
                    .get(&handle)
                    .map(|image| {
                        let size = image.texture_descriptor.size;
                        if size.height == 0 {
                            1.0
                        } else {
                            size.width as f32 / size.height as f32
                        }
                    })
                    .unwrap_or(1.0);
                (
                    ImageNode::new(handle),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(w * 0.5 + center_x - height * aspect * 0.5),
                        top: Val::Px(h * 0.5 - center_y - height * 0.5),
                        width: Val::Px(height * aspect),
                        height: Val::Px(height),
                        ..default()
                    },
                    PromptIcon,
                )
            };
            // Anchored to the SUBMIT/BACK rows: the labels are centred at a fixed x, so the glyph
            // sits immediately to their left instead of at an arbitrary offset.
            let row_x = 230.0;
            let submit_icon = if pad { "ui/media/XboxOne_A.png" } else { "ui/media/Keyboard_Space.png" };
            let back_icon = if pad { "ui/media/XboxOne_B.png" } else { "ui/media/Keyboard_ESC.png" };
            let glyph_height = if pad { 40.0 } else { 34.0 };
            commands.spawn(glyph(submit_icon, row_x, -288.0, glyph_height));
            commands.spawn(glyph(back_icon, row_x, -331.0, glyph_height));
        }
        for (idx, t) in texts.iter().enumerate() {
            commands.spawn((
                Button,
                Text::new(t.text.clone()),
                TextFont {
                    font: font.clone(),
                    font_size: 20.0,
                    ..default()
                },
                TextColor(t.color),
                TextLayout::new_with_justify(t.align),
                TextShadow {
                    offset: Vec2::new(2.0, 2.0),
                    color: Color::BLACK.with_alpha(0.5),
                },
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                Visibility::Hidden,
                MenuLabel(idx),
            ));
        }
        return;
    }
    let h = window.height();
    let w = window.width();
    let canvas_size = menu
        .screens
        .get(menu.current)
        .map_or(Vec2::new(1080.0, 1000.0), |screen| screen.canvas_size);
    let scale = (w / canvas_size.x).min(h / canvas_size.y);
    // GB_MENU_LOADING forces the OG loading state for captures.
    let loading = menu.launch.is_some() || std::env::var_os("GB_MENU_LOADING").is_some();
    for (_, label, mut node, mut text, mut font, mut color, mut vis) in &mut labels {
        let Some(t) = texts.get(label.0) else {
            continue;
        };
        // The splash shows either the OG loading state or the "Press a Button" prompt.
        if menu.current == SPLASH && t.loading != loading {
            *vis = Visibility::Hidden;
            continue;
        }
        let selected = t.item.is_some() && t.item == menu.selected;
        let grow = if selected { 1.1 } else { 1.0 };
        // World-space canvases: project the text pivot and size through the camera.
        let projected = t.world.and_then(|(pos, world_scale)| {
            let depth = (pos - cam_global.translation()).dot(*cam_global.forward());
            if depth <= 0.05 {
                return None;
            }
            let px_per_unit = h / (2.0 * depth * (FOV_DEGREES.to_radians() * 0.5).tan());
            camera.world_to_viewport(cam_global, pos).ok().map(|p| (p, world_scale * px_per_unit))
        });
        if std::env::var_os("GB_MENU_DEBUG").is_some() && label.0 < 3 {
            if let Some((pos, ws)) = t.world {
                let depth = (pos - cam_global.translation()).dot(*cam_global.forward());
                eprintln!("menu text {:?} world {pos:?} scale {ws} depth {depth} cam {:?} proj {:?}", t.text, cam_global.translation(), projected.map(|p| p.0));
            }
        }
        let unit = projected.map_or(scale, |(_, u)| u);
        let size_px = t.size * unit * grow;
        let width_px = (t.width.max(t.size * 4.0)) * unit * grow;
        font.font_size = size_px.max(1.0);
        // Dynamic option values.
        let row = t.item.and_then(|id| menu.item_keys.get(&id)).map(String::as_str);
        let value = match row {
            Some("MENU_WINS") if t.text.chars().all(|c| c.is_ascii_digit()) => {
                Some(menu.wins.to_string())
            }
            Some("MENU_COSTUME_PRESET") => Some(format!("Costume: {}", if menu.editor_costume.is_empty() { "-" } else { menu.editor_costume.as_str() })),
            Some("MENU_COSTUME_COLOR") => menu
                .palette
                .get(menu.player_color)
                .map(|(name, _)| format!("Color: {name}")),
            Some("MENU_GAME_MODE")
                if crate::round::Mode::ALL.iter().any(|m| m.label() == t.text)
                    || t.text == "Soccer" || t.text == "Waves" =>
            {
                Some(if menu.bots > 0 { format!("{} +{} AI", menu.mode.label(), menu.bots) } else { menu.mode.label().to_string() })
            }
            Some("MENU_STAGE") if t.text == "Random" => Some(
                menu.stages
                    .get(menu.stage_index)
                    .map(|s| stage_label(s))
                    .unwrap_or_else(|| "Random".to_string()),
            ),
            Some("SETTINGS_AUDIO_MASTER") => Some(format!("Master: {}%", menu.vol_master * 10)),
            Some("SETTINGS_AUDIO_MUSIC") => Some(format!("Music: {}%", menu.vol_music * 10)),
            Some("SETTINGS_AUDIO_SFX") => Some(format!("Effects: {}%", menu.vol_sfx * 10)),
            Some("SETTINGS_GRAPHICS_RESOLUTION") | Some("RESOLUTION") => {
                let (w, h) = RESOLUTIONS[menu.resolution.min(RESOLUTIONS.len() - 1)];
                Some(format!("Resolution: {w} x {h}"))
            }
            Some("SETTINGS_GRAPHICS_SCREENMODE") | Some("SCREENMODE") =>
                Some(format!("Mode: {}", ["Windowed", "Borderless", "Fullscreen"][menu.screen_mode.min(2)])),
            Some("SETTINGS_GRAPHICS_VSYNC") | Some("VSYNC") =>
                Some(format!("V-sync: {}", if menu.vsync { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_AA") | Some("AA") =>
                Some(format!("Anti-aliasing: {}", ["Off", "2x", "4x", "8x"][menu.aa.min(3)])),
            Some("SETTINGS_GRAPHICS_ANISOTROPIC_FILTER") | Some("ANISOTROPIC") =>
                Some(format!("Anisotropic Filter: {}", ["Off", "2x", "4x", "8x", "16x"][menu.aniso.min(4)])),
            Some("SETTINGS_GRAPHICS_QUALITY") | Some("QUALITY") =>
                Some(format!("Quality: {}", ["Low", "Medium", "High", "Ultra"][menu.quality.min(3)])),
            Some("SETTINGS_GRAPHICS_TEXTURE_QUALITY") | Some("TEXTURE_QUALITY") =>
                Some(format!("Texture Quality: {}", ["Low", "Medium", "High"][menu.texture_quality.min(2)])),
            Some("SETTINGS_GRAPHICS_SHADOWS") | Some("SHADOWS") =>
                Some(format!("Shadows: {}", if menu.shadows { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_AMBIENT_OCCLUSION") | Some("AO") =>
                Some(format!("Ambient Occlusion: {}", if menu.ao { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_SCREEN_SPACE_REFLECTIONS") | Some("SSR") =>
                Some(format!("Screen Space Reflection: {}", if menu.ssr { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_BLOOM") | Some("BLOOM") =>
                Some(format!("Bloom: {}", if menu.bloom { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_DEPTH_OF_FIELD") | Some("DOF") =>
                Some(format!("Depth of Field: {}", ["Off", "Gaussian", "Bokeh"][menu.dof.min(2)])),
            Some("SETTINGS_GRAPHICS_CHROMATIC_ABERRATION") | Some("CA") =>
                Some(format!("Chromatic Aberration: {}", if menu.ca { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_GRAIN") | Some("GRAIN") =>
                Some(format!("Grain: {}", if menu.grain { "On" } else { "Off" })),
            Some("SETTINGS_GRAPHICS_VIGNETTE") | Some("VIGNETTE") =>
                Some(format!("Vignette: {}", if menu.vignette { "On" } else { "Off" })),
            _ => None,
        };
        text.0 = value.unwrap_or_else(|| t.text.clone());
        color.0 = match t.item {
            Some(_) if selected => SELECTED,
            Some(_) => NORMAL,
            None => t.color,
        };
        node.width = Val::Px(width_px);
        match projected {
            Some((p, _)) => {
                node.left = Val::Px(p.x - width_px * 0.5);
                node.top = Val::Px(p.y - size_px * 0.6);
            }
            None => {
                node.left = Val::Px(w * 0.5 + t.ui_position.x * scale - width_px * 0.5);
                node.top = Val::Px(h * 0.5 - t.ui_position.y * scale - size_px * 0.6);
            }
        }
        // Lobby: the match-option rows only appear once the host is ready, but plain labels
        // (item: None — e.g. the COLOUR / SUBMIT / BACK prompts from the reference) must show
        // as soon as a beast is in the lobby.
        let hide_wins = menu.current == LOBBY && menu.mode == crate::round::Mode::Waves && row == Some("MENU_WINS");
        *vis = if hide_wins || (menu.current == LOBBY && !menu.lobby_options && t.item.is_some()) {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}

/// Stages a mode can be played on. Waves needs the stage's wave entrance / `GamemodeEnabled` Waves objects
/// (rooftop, subway, grind, incinerator, chute, aquarium); Soccer is Alley's.
fn stage_allowed(mode: crate::round::Mode, stage: &str) -> bool {
    use crate::round::Mode;
    match mode {
        Mode::Waves => ["rooftop", "subway", "grind", "incinerator"].contains(&stage),
        Mode::Soccer => stage == "alley",
        _ => stage != "alley",
    }
}

/// Lobby choices (mode, stage, wins, AI count, colour) are remembered between launches in
/// `%APPDATA%/gb-rust/lobby.json` (or `$HOME/.gb-rust/lobby.json`).
fn prefs_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|p| p.join("gb-rust"))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".gb-rust")))?;
    Some(base.join("lobby.json"))
}

pub fn load_prefs() -> serde_json::Value {
    prefs_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Records a Waves result (waves survived); returns the best so far.
pub fn record_waves(survived: u32) -> u32 {
    let mut prefs = load_prefs();
    let best = prefs["waves_best"].as_u64().unwrap_or(0) as u32;
    if survived > best {
        if !prefs.is_object() {
            prefs = serde_json::json!({});
        }
        prefs["waves_best"] = survived.into();
        write_prefs(&prefs);
    }
    best.max(survived)
}

fn write_prefs(value: &serde_json::Value) {
    let Some(path) = prefs_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_vec_pretty(value).unwrap_or_default());
}

/// Default audio mix (master, music, effects) in tenths; the game is mixed quietly so music sits under the effects.
pub const DEFAULT_VOL: (u32, u32, u32) = (5, 3, 6);

/// Persist the mix from the in-match pause screen (tenths).
pub fn save_audio_tenths(master: u32, music: u32, sfx: u32) {
    let mut value = load_prefs();
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["vol_master"] = master.into();
    value["vol_music"] = music.into();
    value["vol_sfx"] = sfx.into();
    write_prefs(&value);
}

fn save_prefs_audio(menu: &Menu) {
    let mut value = load_prefs();
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["vol_master"] = menu.vol_master.into();
    value["vol_music"] = menu.vol_music.into();
    value["vol_sfx"] = menu.vol_sfx.into();
    write_prefs(&value);
}

fn save_prefs(menu: &Menu) {
    let mut value = load_prefs();
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let fresh = serde_json::json!({
        "mode": menu.mode.id(),
        "stage": menu.stages.get(menu.stage_index).map_or("random", String::as_str),
        "wins": menu.wins,
        "bots": menu.bots,
        "colour": menu.player_color,
    });
    if let (Some(dst), Some(src)) = (value.as_object_mut(), fresh.as_object()) {
        dst.extend(src.clone());
    }
    write_prefs(&value);
}

/// The costume editor shows the real (dressable) beast; the scene's static, head-less body rigs would stand in front of it.
fn hide_static_rigs(
    menu: Option<Res<Menu>>,
    rigs: Query<(Entity, &Name, &Visibility)>,
    parents: Query<&ChildOf>,
    scenes: Query<(), With<crate::play::PhysicsScene>>,
    mut commands: Commands,
    mut hidden: Local<Vec<Entity>>,
) {
    let Some(menu) = menu else { return };
    if menu.current != COSTUME_EDITOR {
        for e in hidden.drain(..) {
            if let Ok(mut c) = commands.get_entity(e) {
                c.insert(Visibility::Inherited);
            }
        }
        return;
    }
    if !hidden.is_empty() {
        return;
    }
    for (e, name, vis) in &rigs {
        if !(name.as_str() == "actor_body_skinnedMesh" || name.as_str() == "actor_head_skinnedMesh") || *vis == Visibility::Hidden {
            continue;
        }
        let mut cur = e;
        let mut in_sim = false;
        while let Ok(p) = parents.get(cur) {
            cur = p.parent();
            if scenes.contains(cur) {
                in_sim = true;
                break;
            }
        }
        if !in_sim {
            commands.entity(e).insert(Visibility::Hidden);
            hidden.push(e);
        }
    }
}

/// "rooftop" -> "Rooftop"; the export names that need more than a capital letter.
fn stage_label(name: &str) -> String {
    match name {
        "lighthouse" => "Lighthouse".into(),
        "wheel" => "Ferris Wheel".into(),
        "vents" => "Fans".into(),
        "grind" => "Grinders".into(),
        "containers" => "Containers".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}
