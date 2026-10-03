//! Asset viewer and headless validation for the Gang Beasts Rust reimplementation.
mod audio;
mod costume;
mod unseen;
mod devgui;
mod fracture;
#[allow(dead_code)] // opt-in experimental engine (GB_ENGINE=1)
mod gb_engine;
mod light_probes;
mod urp_post;
mod look;
mod menu;
mod mips;
mod play;
mod round;
mod scene;
mod sky;
mod vinyl;
mod water;

use bevy::gltf::GltfExtras;
use bevy::image::ImageLoaderSettings;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::pbr::{LightProbe, Lightmap};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bevy::scene::{SceneInstance, SceneSpawner};
use scene::SceneData;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Gang Beasts asset error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut stage_name = "rooftop".to_string();
    let mut headless = false;
    let mut view_only = false;
    let mut menu_mode = false;
    let mut spawn_index = 3usize;
    let mut players = 1usize;
    let mut bots = std::env::var("GB_BOTS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let mut wins = round::WINS_TO_WIN;
    let mut player_color = 0usize;
    let mut root = std::env::var_os("GB_ASSET_ROOT").map(PathBuf::from);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--headless" => headless = true,
            "--view" => view_only = true,
            "--menu" => menu_mode = true,
            "--assets" => {
                root = Some(PathBuf::from(
                    args.next().ok_or("--assets requires a directory")?,
                ))
            }
            "--spawn" => {
                spawn_index = args
                    .next()
                    .ok_or("--spawn requires an index")?
                    .parse()
                    .map_err(|_| "invalid spawn index")?
            }
            "--players" => {
                players = args
                    .next()
                    .ok_or("--players requires a count")?
                    .parse()
                    .map_err(|_| "invalid player count")?
            }
            "--bots" => {
                bots = args.next().ok_or("--bots requires a count")?.parse().map_err(|_| "invalid bot count")?;
            }
            "--wins" => {
                wins = args
                    .next()
                    .ok_or("--wins requires a count")?
                    .parse()
                    .map_err(|_| "invalid win count")?
            }
            "--player-color" => {
                player_color = args
                    .next()
                    .ok_or("--player-color requires a palette index")?
                    .parse()
                    .map_err(|_| "invalid player color index")?
            }
            "--mode" => {
                // Lobby game mode id ("melee" / "gang"); round::Round reads GB_MODE.
                let id = args.next().ok_or("--mode requires a mode id")?;
                std::env::set_var("GB_MODE", id);
            }
            "--costumes" => {
                let list = args.next().ok_or("--costumes requires a|b|c")?;
                std::env::set_var("GB_COSTUMES", list);
            }
            "--costume" => {
                // The lobby passes its chosen preset through; costume::preset_for reads GB_COSTUME.
                let name = args.next().ok_or("--costume requires a preset name")?;
                std::env::set_var("GB_COSTUME", name);
            }
            "--help" | "-h" => {
                println!("gb_game [stage] [--menu] [--headless] [--assets DIRECTORY] [--spawn INDEX] [--players COUNT] [--wins COUNT] [--player-color INDEX]\n\n--menu opens the local title and rooftop lobby.\n--headless validates exports without initializing a window, renderer, or audio.\n--players starts local fighters at successive stage spawn points (1-10).\n--player-color selects the first local player's unlocked source palette swatch.\nViewer: hold RMB to look; WASD/QE to fly; Shift speeds up; R resets the camera.\nGB_CAM=x,y,z,tx,ty,tz overrides the viewer camera; GB_ASSET_ROOT overrides assets.");
                return Ok(());
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => stage_name = arg,
        }
    }
    let humans = players;
    let players = (players + bots).min(10);
    // Alley (Soccer pitch) is a narrow walled court: the follow camera ends up inside the brick walls. The real game
    // uses a fixed pitch camera there; pin an overview unless the user pinned one.
    if stage_name == "alley" && std::env::var_os("GB_CAM_EYE").is_none() {
        std::env::set_var("GB_CAM_EYE", "9,12,22");
        std::env::set_var("GB_CAM_AT", "9,0,5");
        std::env::set_var("GB_CAM_FOV", "60");
    }
    menu_mode |= stage_name == "menu";
    if menu_mode {
        stage_name = "menu".into();
        view_only = true;
    }
    if stage_name.is_empty()
        || !stage_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("stage must be an exported asset name".into());
    }
    let root = root.unwrap_or_else(|| {
        // Work both from cargo run and when launching the built executable elsewhere.
        let adjacent = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from))
            .and_then(|dir| {
                dir.ancestors()
                    .map(|p| p.join("assets/export"))
                    .find(|p| p.is_dir())
            });
        adjacent.unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/export")
        })
    });
    let root = root
        .canonicalize()
        .map_err(|e| format!("asset directory {}: {e}", root.display()))?;
    let stage_data = SceneData::load(&root, &stage_name)?;
    let beast_data = SceneData::load(&root, "beast")?;
    let stage_graphics = root.join(format!("{stage_name}-graphics.json"));
    let graphics_path = if stage_graphics.is_file() {
        stage_graphics
    } else {
        root.join("graphics.json")
    };
    let graphics: Value = serde_json::from_slice(
        &fs::read(&graphics_path).map_err(|e| format!("{}: {e}", graphics_path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", graphics_path.display()))?;
    let reflection_probes = load_reflection_probes(&root, &graphics, &stage_data)?;
    let mut lightmap_paths: Vec<Option<String>> = graphics["lightmaps"]["color_images"]
        .as_array()
        .map(|paths| {
            paths
                .iter()
                .map(|path| path.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    // Drop degenerate exported lightmap atlases. tools/extract/validate_lightmaps.py
    // writes lightmap-quality.json: a defective export ships as a near-white page
    // (median texel ~0.96) whose baked value saturates whole stages to white once
    // the encode exposure is applied at bind time. Real atlases (Aquarium/Menu,
    // p50 ~0.08) pass and keep binding; flagged ones simply don't attach, so the
    // stage keeps its realtime lighting instead of garbage-white baked GI.
    let quality = fs::read(root.join("lightmap-quality.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(quality) = quality {
        lightmap_paths = lightmap_paths
            .into_iter()
            .map(|path| {
                let flagged = path
                    .as_deref()
                    .and_then(|name| quality.get(name))
                    .and_then(|entry| entry.get("degenerate"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && std::env::var_os("GB_KEEP_DEGENERATE_LIGHTMAPS").is_none();
                if flagged {
                    println!("skipping degenerate lightmap atlas (see lightmap-quality.json)");
                }
                path.filter(|_| !flagged)
            })
            .collect();
    }
    let mut lightmap_assignments = stage_data.lightmap_assignments();
    // Menu Alley renders like Rooftop (realtime sun + global look, no baked lightmaps): its
    // lightmaps lit the dark-painted metal far brighter than the references. GB_MENU_LIGHTMAPS=1
    // brings them back.
    if stage_name == "menu" && std::env::var_os("GB_MENU_LIGHTMAPS").is_none() {
        lightmap_assignments.clear();
    }
    if stage_name == "aquarium" {
        // The combined gallery renderer owns the arena floor and baked UV atlas. Keep the
        // experimental Bevy lightmap path to this validated UV1 mesh while other renderers
        // remain on the stable forward-material path.
        lightmap_assignments.retain(|assignment| {
            stage_data
                .nodes
                .get(assignment.node)
                .is_some_and(|node| node.path == "Aquarium/aquarium_enviroment")
        });
    }
    let lightmap_exposures = graphics["lightmaps"]["encodings"]
        .as_array()
        .map(|encodings| {
            encodings
                .iter()
                .map(|encoding| {
                    encoding["exposure"]
                        .as_f64()
                        .map(|x| x as f32)
                        .unwrap_or(1.0)
                })
                .collect()
        })
        .unwrap_or_default();
    let sun = stage_data.directional_sun();
    let surface_fog = sky::SurfaceFog::from_scene(&stage_data);
    let lights = stage_data.local_lights();
    println!("{}", stage_data.summary(&stage_name));
    println!("{} enabled local lights", lights.len());
    println!("{}", beast_data.summary("beast"));
    let mut spawns = stage_data.spawns();
    let authored_spawns = spawns.len();
    // Single-circle stages (Towers, Lighthouse, Trawler) mark ONE spawn area for everybody: fan the
    // extra fighters out around it so up to ten can start.
    if !spawns.is_empty() {
        let base = spawns.clone();
        let mut ring = 0usize;
        while spawns.len() < 10 {
            let b = base[spawns.len() % base.len()];
            let angle = ring as f32 * 0.9 + 0.4;
            let mut t = b;
            t.translation += Vec3::new(angle.cos(), 0.0, angle.sin()) * (1.4 + 0.5 * (ring / 6) as f32);
            spawns.push(t);
            ring += 1;
        }
    }
    if !menu_mode && (!(1..=10).contains(&players) || players > spawns.len()) {
        return Err(format!(
            "player count must be between 1 and {} active spawn points",
            spawns.len().min(10)
        ));
    }
    // Stages with fewer active spawn points than the default start index (e.g. Ring has 2).
    let spawn_index = if spawn_index >= authored_spawns { 0 } else { spawn_index };
    let spawn = if menu_mode {
        Transform::IDENTITY
    } else {
        *spawns.get(spawn_index).ok_or_else(|| {
            format!(
                "spawn {spawn_index} is unavailable; {} active spawn points found",
                spawns.len()
            )
        })?
    };
    if !menu_mode {
        println!("Selected spawn {spawn_index}: {:?}", spawn.translation);
    }
    let camera = parse_camera(
        &std::env::var("GB_CAM").unwrap_or_else(|_| "0.15,6,15.8,1.5,0.5,3.5".into()),
    )?;
    if headless {
        println!("Headless validation passed. No window, graphics device, or audio initialized.");
        return Ok(());
    }
    let sim = if menu_mode {
        // Menu Alley lobby: the scene is live physics; beasts join from the lobby screen.
        let mut sim = play::build(&root, &stage_name, spawns.clone(), 0, 0, wins, player_color)?;
        sim.round.players = humans;
        sim.lobby = true;
        // No rounds in the lobby: keep the round-start fade from covering the menu.
        sim.round.round_time = 1.0e6;
        Some(sim)
    } else if view_only {
        None
    } else {
        let mut built = play::build(
            &root,
            &stage_name,
            spawns.clone(),
            spawn_index,
            players,
            wins,
            player_color,
        )?;
        built.round.players = humans;
        Some(built)
    };
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: root.to_string_lossy().into_owned(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Gang Beasts".into(),
                    mode: bevy::window::WindowMode::Windowed,
                    decorations: true,
                    visible: true,
                    // GB_WINDOW_OFFSCREEN: harness renders open off-screen and unfocused so they
                    // don't flash over the user's desktop.
                    position: if std::env::var_os("GB_WINDOW_OFFSCREEN").is_some() {
                        bevy::window::WindowPosition::At(IVec2::new(-5000, 0))
                    } else {
                        bevy::window::WindowPosition::Automatic
                    },
                    focused: std::env::var_os("GB_WINDOW_OFFSCREEN").is_none(),
                    ..default()
                }),
                ..default()
            })
            // Unity's texture-quality setting samples GLB albedo/normal maps with anisotropic
            // filtering; Bevy's default sampler clamps to 1 (trilinear), which is why angled
            // surfaces (mortar joints, bevels) lose detail against the reference captures.
            .set(ImagePlugin {
                default_sampler: bevy::image::ImageSamplerDescriptor {
                    anisotropy_clamp: environment_u16("GB_ANISO", 8),
                    mag_filter: bevy::image::ImageFilterMode::Linear,
                    min_filter: bevy::image::ImageFilterMode::Linear,
                    mipmap_filter: bevy::image::ImageFilterMode::Linear,
                    ..default()
                },
            }),
    )
    .add_plugins(urp_post::UrpPostPlugin)
    // In-game tuning panel (F2). Reads the same GB_* keys the harness uses.
    .add_plugins(devgui::DevPanelPlugin)
    // gbrender's translated URP material path (handoff/GBRENDER_TRANSFER_PLAN.md). Registered only
    // when GB_ENGINE=1 so the default renderer is untouched until the A/B proves it.
    .add_plugins(gb_engine::GbEnginePlugin)
    .insert_resource(light_probes::LightProbes::load(&graphics))
    .insert_resource(vinyl::ProbeSource(Some(
        graphics_path.to_string_lossy().into_owned(),
    )))
    .add_systems(Update, light_probes::apply_to_beast)
    // Unity ambient sky color/intensity from RenderSettings, scaled to Bevy's lumen units.
    .insert_resource({
        // Outdoor stages have no baked lightmaps: the calibrated indoor fill left them near black.
        let mut ambient = ambient_light_from_settings(&graphics);
        if graphics["lightmaps"].is_null() {
            ambient.brightness = 700.0 * ambient.brightness / 500.0;
        }
        if stage_name == "menu" {
            look::Look::menu().ambient(ambient)
        } else if stage_name == "rooftop" {
            look::Look::rooftop().ambient(ambient)
        } else if stage_name == "ring" {
            // The Ring's authored sky ambient is a strong teal that read as a blue wash; keep its level, drop most of the tint
            // (GB_RING_TINT = how much of the teal stays, default 0.2).
            let mut a = look::Look::load().ambient(ambient);
            let keep: f32 = std::env::var("GB_RING_TINT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.2);
            let c = a.color.to_linear();
            let grey = c.red * 0.2126 + c.green * 0.7152 + c.blue * 0.0722;
            let mix = |v: f32| grey + (v - grey) * keep;
            a.color = Color::linear_rgb(mix(c.red) * 1.15, mix(c.green), mix(c.blue) * 0.9);
            a
        } else {
            look::Look::load().ambient(ambient)
        }
    })
    .insert_resource(ClearColor(Color::srgb(0.62, 0.78, 0.92)))
    .insert_resource(ViewerConfig {
        name: stage_name,
        spawn,
        camera,
        play: sim.is_some(),
        graphics,
        lightmap_paths,
        lightmap_exposures,
        lightmap_assignments,
        reflection_probes,
        sun,
        lights,
        surface_fog,
    })
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            fly_camera,
            sky::follow_camera,
            baked_static_shadows,
            mips::generate,
            mips::generate_tangents,
            mips::vinyl_roughness,
            attach_stage_lightmaps,
            attach_vinyl_lightmaps,
            lightmap_debug,
            refresh_lightmaps,
            material_debug,
            auto_screenshot,
            manual_screenshot,
        ),
    );
    audio::plugin(&mut app, &root);
    if let Some(sim) = sim {
        app.insert_non_send_resource(sim).add_plugins(play::plugin);
        costume::plugin(&mut app, &root);
        water::plugin(&mut app, &root);
    }
    if menu_mode {
        app.insert_resource(menu::MenuRoot(root.clone()))
            .add_plugins(menu::plugin);
    }
    // The VinylOrMetal material path applies to every stage, not just the menu: Rooftop's
    // scenery is 19x that shader in the export, so it needs the converter (and the material
    // plugin) installed or its graph — normal remap, _BumpScale, triplanar, SH ambient — is
    // silently dropped and the walls render flat. GB_STAGE_VINYL=0 keeps the old behaviour.
    if menu_mode || std::env::var_os("GB_STAGE_VINYL").is_none() {
        app.add_plugins(vinyl::plugin);
    }
    // The menu was tuned with the shader's URP-BRDF replacement ON; on stages that
    // correction cancels the sun (see vinyl::UrpDirect), so it stays off there.
    app.insert_resource(vinyl::UrpDirect(menu_mode));
    app.run();
    Ok(())
}

/// F12 saves the live rendered frame for visual review without relying on desktop capture APIs.
fn manual_screenshot(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/game-capture.png")
        .to_string_lossy()
        .into_owned();
    commands
        .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
        .observe(bevy::render::view::screenshot::save_to_disk(path));
}

fn parse_camera(value: &str) -> Result<(Vec3, Vec3), String> {
    let values: Vec<f32> = value
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| "GB_CAM requires six finite numbers")?;
    if values.len() != 6 || values.iter().any(|v| !v.is_finite()) {
        return Err("GB_CAM requires x,y,z,target_x,target_y,target_z".into());
    }
    let eye = Vec3::new(values[0], values[1], values[2]);
    let target = Vec3::new(values[3], values[4], values[5]);
    if (target - eye).length_squared() < 1e-8 {
        return Err("GB_CAM eye and target must differ".into());
    }
    Ok((eye, target))
}

#[derive(Resource)]
struct ViewerConfig {
    name: String,
    spawn: Transform,
    camera: (Vec3, Vec3),
    play: bool,
    graphics: Value,
    lightmap_paths: Vec<Option<String>>,
    lightmap_exposures: Vec<f32>,
    lightmap_assignments: Vec<scene::SceneLightmapAssignment>,
    reflection_probes: Vec<StageReflectionProbe>,
    sun: Option<scene::SceneSun>,
    lights: Vec<scene::SceneLight>,
    surface_fog: Option<sky::SurfaceFog>,
}

struct StageReflectionProbe {
    transform: Transform,
    intensity: f32,
    width: u32,
    height: u32,
    mip_count: u32,
    face_count: u32,
    face_mip_bytes: usize,
    data: Vec<u8>,
}

fn load_reflection_probes(
    root: &std::path::Path,
    graphics: &Value,
    stage: &SceneData,
) -> Result<Vec<StageReflectionProbe>, String> {
    let source_probes = stage.reflection_probes();
    let records = graphics["reflection_probes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut probes = Vec::new();
    for source in source_probes {
        let Some(record) = records
            .iter()
            .find(|record| record["cubemap_path_id"].as_u64() == Some(source.cubemap_path_id))
        else {
            continue;
        };
        if record["format"].as_u64() != Some(24) {
            continue;
        }
        let filename = record["data_file"]
            .as_str()
            .ok_or("reflection probe is missing data_file")?;
        let data = fs::read(root.join(filename)).map_err(|error| {
            format!(
                "reflection cubemap {}: {error}",
                root.join(filename).display()
            )
        })?;
        let width = record["width"].as_u64().unwrap_or(0) as u32;
        let height = record["height"].as_u64().unwrap_or(0) as u32;
        let mip_count = record["mip_count"].as_u64().unwrap_or(0) as u32;
        let face_count = record["face_count"].as_u64().unwrap_or(0) as u32;
        let face_mip_bytes = record["face_mip_bytes"].as_u64().unwrap_or(0) as usize;
        if width == 0
            || width != height
            || mip_count == 0
            || face_count != 6
            || face_mip_bytes * face_count as usize != data.len()
        {
            return Err(format!(
                "invalid cubemap metadata for {}: {}x{}, {} faces, {} mips, {} bytes",
                filename,
                width,
                height,
                face_count,
                mip_count,
                data.len()
            ));
        }
        let source_transform = source.transform;
        let transform = Transform {
            translation: gb_phys::source::mirror_position(source_transform.translation),
            rotation: gb_phys::source::mirror_rotation(source_transform.rotation).normalize(),
            scale: source_transform.scale,
        };
        probes.push(StageReflectionProbe {
            transform,
            intensity: record["intensity"].as_f64().unwrap_or(1.0) as f32,
            width,
            height,
            mip_count,
            face_count,
            face_mip_bytes,
            data,
        });
    }
    Ok(probes)
}

fn reflection_probe_images(probe: &StageReflectionProbe) -> Result<(Image, Image), String> {
    if probe.width != probe.height || probe.face_count != 6 {
        return Err("reflection cubemap must be square with six faces".into());
    }
    let face_count = probe.face_count as usize;
    let mut face_mips: Vec<Vec<Vec<u8>>> = Vec::with_capacity(face_count);
    for face in 0..face_count {
        let face_start = face * probe.face_mip_bytes;
        let mut offset = 0usize;
        let mut width = probe.width;
        let mut height = probe.height;
        let mut mips = Vec::with_capacity(probe.mip_count as usize);
        for _ in 0..probe.mip_count {
            let blocks_x = width.div_ceil(4).max(1);
            let blocks_y = height.div_ceil(4).max(1);
            let size = (blocks_x * blocks_y * 16) as usize;
            let start = face_start + offset;
            let end = start + size;
            let Some(bytes) = probe.data.get(start..end) else {
                return Err("reflection cubemap mip data is truncated".into());
            };
            mips.push(bytes.to_vec());
            offset += size;
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        if offset != probe.face_mip_bytes {
            return Err(format!(
                "reflection cubemap face has {offset} mip bytes, expected {}",
                probe.face_mip_bytes
            ));
        }
        face_mips.push(mips);
    }
    let mut specular_data = Vec::with_capacity(probe.data.len());
    for mip in 0..probe.mip_count as usize {
        for face in &face_mips {
            specular_data.extend_from_slice(&face[mip]);
        }
    }
    let diffuse_data = face_mips.iter().filter_map(|face| face.last()).fold(
        Vec::with_capacity(face_count * 16),
        |mut data, mip| {
            data.extend_from_slice(mip);
            data
        },
    );
    if specular_data.len() != probe.data.len() || diffuse_data.len() != face_count * 16 {
        return Err("reflection cubemap mip layout is inconsistent".into());
    }
    let format = TextureFormat::Bc6hRgbUfloat;
    let cube_view = || {
        Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        })
    };
    let mut diffuse = Image::new_uninit(
        Extent3d {
            // BC6H blocks are 4x4; repeating the source's 1x1 final-mip block
            // into a 4x4 face preserves the constant diffuse irradiance.
            width: 4,
            height: 4,
            depth_or_array_layers: probe.face_count,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    diffuse.data = Some(diffuse_data);
    diffuse.texture_view_descriptor = cube_view();
    let mut specular = Image::new_uninit(
        Extent3d {
            width: probe.width,
            height: probe.height,
            depth_or_array_layers: probe.face_count,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    specular.data = Some(specular_data);
    specular.texture_descriptor.mip_level_count = probe.mip_count;
    specular.texture_view_descriptor = cube_view();
    Ok((diffuse, specular))
}

#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
}

fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    stage: Res<ViewerConfig>,
) {
    if let Some(fog) = &stage.surface_fog {
        // The Alley is walled in by black void boxes; its daytime sky dome only shows through the gaps.
        if stage.name == "alley" {
            commands.insert_resource(ClearColor(Color::BLACK));
        } else {
            sky::spawn_dome(&mut commands, &mut meshes, &mut materials, fog);
            commands.insert_resource(ClearColor(fog.sky));
        }
    }
    let mut lightmaps = HashMap::new();
    for assignment in &stage.lightmap_assignments {
        let Some(path) = stage
            .lightmap_paths
            .get(assignment.index)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        // Unity encodes lightmaps as LINEAR 8-bit HDR / exposure (the exporter
        // pre-flattened RGBM into RGB, alpha = 255). Bevy's PNG loader applies an
        // sRGB curve by default, which mis-decodes those bytes and lit dark metal
        // far too bright. The menu re-enables its baked GI on the correct linear
        // decode; other stages keep their legacy visually-calibrated load until
        // their exposure constants are re-tuned on the linear path too.
        let image: Handle<Image> = if (stage.name == "menu" && std::env::var_os("GB_SRGB_LIGHTMAPS").is_none()) || std::env::var_os("GB_LINEAR_LIGHTMAPS").is_some()
        {
            assets.load_with_settings(path.clone(), |settings: &mut ImageLoaderSettings| {
                settings.is_srgb = false;
            })
        } else {
            assets.load(path.clone())
        };
        lightmaps.insert(
            assignment.node,
            // Aquarium/Menu were calibrated at 1.0. Other stages' atlases (now decoded as RGBM like the Aquarium's) read
            // ~4x too bright at the same constant, so scale them down (matches Grind; Subway/Vents stop blowing out).
            (
                image,
                assignment.scale_offset,
                stage.lightmap_exposures.get(assignment.index).copied().unwrap_or(1.0)
                    * if stage.name == "menu" || stage.name == "aquarium" {
                        1.0
                    } else {
                        std::env::var("GB_LIGHTMAP_STAGE_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.25)
                    },
            ),
        );
    }
    println!(
        "{} baked lightmaps mapped to {} rendered nodes",
        stage.lightmap_paths.iter().flatten().count(),
        lightmaps.len()
    );
    commands.insert_resource(StageLightmaps(lightmaps));
    for probe in &stage.reflection_probes {
        if std::env::var_os("GB_NO_PROBES").is_some() {
            break;
        }
        match reflection_probe_images(probe) {
            Ok((diffuse, specular)) => {
                commands.spawn((
                    LightProbe,
                    EnvironmentMapLight {
                        diffuse_map: images.add(diffuse),
                        specular_map: images.add(specular),
                        intensity: probe.intensity,
                        rotation: Quat::IDENTITY,
                        affects_lightmapped_mesh_diffuse: false,
                    },
                    probe.transform,
                ));
            }
            Err(error) => error!("could not load reflection probe: {error}"),
        }
    }
    println!(
        "{} Unity reflection probes loaded",
        stage.reflection_probes.len()
    );
    let stage_root = commands
        .spawn((
            SceneRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(format!("{}.glb", stage.name)))),
            play::PhysicsScene(0),
        ))
        .id();
    // Without shadowmask sampling, static scenery casts the key light's shadows in realtime
    // (like Unity's Distance Shadowmask). GB_MENU_BAKED_SHADOWS turns that off.
    // Rooftop is 19x GangBeasts/Surface/VinylOrMetal in the export: the same material path the
    // menu uses, so its scenery converts too (GB_STAGE_VINYL=0 opts out for A/B). Without this
    // the stage's walls keep plain StandardMaterial and lose the graph's normal remap,
    // _BumpScale strength and SH ambient — the "flat, less detailed bricks" gap.
    let stage_vinyl = std::env::var_os("GB_STAGE_VINYL").is_none();
    if stage.name == "menu" || stage_vinyl {
        commands.entity(stage_root).insert(vinyl::MenuScenery);
    }
    if stage.name == "menu" && std::env::var_os("GB_MENU_BAKED_SHADOWS").is_some() {
        commands.entity(stage_root).insert(StaticShadowsBaked);
    }
    if !stage.play && stage.name != "menu" {
        commands.spawn((
            SceneRoot(assets.load(GltfAssetLabel::Scene(0).from_asset("beast.glb"))),
            stage.spawn,
        ));
    }
    if let Some(sun) = stage.sun {
        let color = Color::srgb(sun.color[0], sun.color[1], sun.color[2]);
        // Outdoor (no usable baked lightmaps) stages get 2,600 lux per Unity unit with the
        // source +1.4 EV post exposure; 1,000 is only right for a lightmapped interior
        // (Aquarium). Decide from the lightmaps that actually ATTACH after the degenerate
        // atlas filter, not from the JSON key: Rooftop's export lists atlases that are all
        // filtered out, so keying on the JSON left it under-lit as an "indoor" stage.
        let usable_lightmaps = stage.lightmap_paths.iter().flatten().count();
        let illuminance = sun.intensity
            * if usable_lightmaps == 0
                || (stage.name == "menu" && std::env::var_os("GB_MENU_LIGHTMAPS").is_none())
            {
                2_600.0
            } else {
                1_000.0
            }
            // Stage sun scale: the source's 2,600-lux sun alone leaves the deck dark because the
            // baked bounce that fills the floor is not reproduced, while near-white materials
            // (the skylight frame) already clip. Rooftop therefore runs a stronger sun with the
            // exposure pulled down (Look::rooftop) so the range compresses through ACES instead
            // of clipping. Other stages keep 1.0 until they get the same treatment.
            * environment_f32("GB_SUN_SCALE", if stage.name == "rooftop" { 2.35 } else { 1.0 });
        let shadows = sun.casts_shadows && std::env::var_os("GB_NO_SHADOWS").is_none();
        // Use the SAME stage-scoped look as the ambient resource. The menu is deliberately
        // excluded: its sun was tuned with the shared default (bounce 0.35 / fill 0.45) while
        // its ambient uses Look::menu(), so scoping the sun there regressed the title
        // (74.4% -> 72.8%). Rooftop gets its own scoped sun.
        let stage_look = if stage.name == "rooftop" {
            look::Look::rooftop()
        } else {
            look::Look::load()
        };
        stage_look.spawn_sun(
            &mut commands,
            color,
            illuminance,
            shadows,
            sun.shadow_strength,
            sun.transform,
        );
    }
    for light in &stage.lights {
        let color = Color::srgb(light.color[0], light.color[1], light.color[2]);
        // Match the source-to-Bevy intensity scale used for the stage directional light;
        // retain the authored local falloff range and color.
        // Unity's serialized indoor-light values were calibrated against the local Aquarium
        // lights at 500 lumens per source intensity. The 6,000 multiplier blew out this gallery.
        let scale = std::env::var("GB_LOCAL_LIGHT_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            // Physically derived from the calibrated sun (2,600 lux per Unity intensity): a point light of
            // Unity intensity I has I * 2600 candela = I * 2600 * 4 pi lumens. Menu/Aquarium keep the
            // older 500 because their baked lightmaps already contain these lights' contribution.
            .unwrap_or(if stage.name == "menu" || stage.name == "aquarium" { 500.0 } else { 32_670.0 });
        let intensity = light.intensity * scale;
        let shadows_enabled = light.casts_shadows;
        match light.kind {
            scene::SceneLightKind::Point | scene::SceneLightKind::Area => {
                commands.spawn((
                    PointLight {
                        color,
                        intensity,
                        range: light.range,
                        shadows_enabled,
                        ..default()
                    },
                    light.transform,
                ));
            }
            scene::SceneLightKind::Spot => {
                commands.spawn((
                    SpotLight {
                        color,
                        intensity,
                        range: light.range,
                        // Unity stores the full cone width; Bevy expects its half angle.
                        // Keep existing stage calibration intact while correcting Alley lights.
                        inner_angle: (light.inner_angle
                            * if stage.name == "aquarium" { 1.0 } else { 0.5 })
                            .to_radians(),
                        outer_angle: (light.outer_angle
                            * if stage.name == "aquarium" { 1.0 } else { 0.5 })
                            .to_radians(),
                        shadows_enabled,
                        ..default()
                    },
                    light.transform,
                ));
            }
        }
    }
    let (eye, target) = stage.camera;
    let look = (target - eye).normalize();
    if stage.play {
        let at = stage.spawn.translation;
        let mut camera = commands.spawn((
            Camera3d::default(),
            Camera {
                hdr: true,
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                far: 30_000.0,
                ..default()
            }),
            bevy::render::view::NoIndirectDrawing,
            bloom_from_settings(&stage.graphics),
            // The graphics settings screen toggles bloom; remember the authored (tuned) value.
            menu::AuthoredBloom(bloom_from_settings(&stage.graphics).intensity),
            post_stack(&stage.graphics, &stage.name),
            Msaa::Sample4,
            Transform::from_translation(at + Vec3::new(0.0, 3.5, -6.5)).looking_at(at, Vec3::Y),
            play::FollowCam,
        ));
        // The sea shader reads the opaque depth for shoreline / contact foam (GB_NO_WATER_DEPTH=1 skips it).
        if matches!(stage.name.as_str(), "buoy" | "lighthouse" | "trawler" | "containers" | "crane" | "wheel")
            && std::env::var_os("GB_NO_WATER_DEPTH").is_none()
        {
            camera.insert(bevy::core_pipeline::prepass::DepthPrepass);
        }
        if stage.name == "menu" {
            // Menu lobby: the menu drives this camera; no ground-fog approximation (see below).
        } else if let (Some(fog), true) = (&stage.surface_fog, matches!(stage.name.as_str(), "rooftop" | "towers" | "girders" | "billboard")) {
            camera.insert(fog.distance_fog());
        } else if stage.name == "trawler" {
        // Very light sea haze (user request): far water/horizon melts into a pale blue.
        camera.insert(DistanceFog {
            color: Color::srgb(0.78, 0.88, 0.96),
            falloff: FogFalloff::Linear { start: 90.0, end: 700.0 },
            ..default()
        });
} else if stage.graphics["render_settings"]["fog"].as_bool() == Some(true) && stage.name != "trawler" {
            camera.insert(fog_from_settings(&stage.graphics));
        }
        return;
    }
    let mut camera = commands.spawn((
        Camera3d::default(),
        Camera {
            hdr: true,
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            far: 30_000.0,
            ..default()
        }),
        // Bevy 0.16 bug: skinned meshes + multi-draw-indirect prepass fail wgpu validation.
        bevy::render::view::NoIndirectDrawing,
        bloom_from_settings(&stage.graphics),
        // The graphics settings screen toggles bloom; remember the authored (tuned) intensity.
        menu::AuthoredBloom(bloom_from_settings(&stage.graphics).intensity),
        post_stack(&stage.graphics, &stage.name),
        Transform::from_translation(eye),
        FlyCam {
            yaw: (-look.x).atan2(-look.z),
            pitch: look.y.asin(),
        },
    ));
    if stage.name == "menu" {
        // Menu Alley disables nearby ground fog (10,000 m depth / -50 degree elevation).
        // The outdoor approximation uses skyDepth * 3 as its end, which is only 613 m here:
        // that inverted interval fogged every Alley surface completely white.
        camera.insert(Msaa::Sample4);
    } else if let (Some(fog), true) = (&stage.surface_fog, matches!(stage.name.as_str(), "rooftop" | "towers" | "girders" | "billboard")) {
        camera.insert(fog.distance_fog());
    } else if stage.name == "trawler" {
        // Very light sea haze (user request): far water/horizon melts into a pale blue.
        camera.insert(DistanceFog {
            color: Color::srgb(0.78, 0.88, 0.96),
            falloff: FogFalloff::Linear { start: 90.0, end: 700.0 },
            ..default()
        });
} else if stage.graphics["render_settings"]["fog"].as_bool() == Some(true) && stage.name != "trawler" {
        camera.insert(fog_from_settings(&stage.graphics));
    }
    // Ambient occlusion: the source scene's materials get their crevice/contact depth from
    // baked GI we cannot reproduce yet (the exported atlases are degenerate). Screen-space AO
    // restores that grounding on props/crevices. Bevy's SSAO requires MSAA off on the camera.
    if std::env::var_os("GB_NO_SSAO").is_none() {
        camera.insert((
            bevy::pbr::ScreenSpaceAmbientOcclusion {
                quality_level: bevy::pbr::ScreenSpaceAmbientOcclusionQualityLevel::High,
                // Thicker constant object thickness makes the AO reach further into the vents,
                // ducts and wall/floor joins, which is where the references show contact depth.
                constant_object_thickness: ssao_thickness(&stage.name),
                ..default()
            },
            Msaa::Off,
        ));
    }
}

/// `GB_MAT_DEBUG=1` logs the material state the renderer actually sees for beast/costume
/// materials (albedo, metallic, roughness, emissive, lightmap exposure). Harness tooling.
/// Writes `target/matdump.txt` so a capture run can be inspected afterwards.
fn material_debug(
    named: Query<(&bevy::gltf::GltfMaterialName, &MeshMaterial3d<StandardMaterial>)>,
    meshes: Query<(&Name, &MeshMaterial3d<StandardMaterial>)>,
    positions: Query<(&Name, &GlobalTransform)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    if std::env::var_os("GB_MAT_DEBUG").is_none() {
        return;
    }
    // Sample in the window before the harness screenshot (GB_SCREENSHOT_AFTER, default 300
    // frames ~ 5 s): dump a few frames' state as JSON lines.
    let t = time.elapsed_secs();
    if !((3.0..5.5).contains(&t) && (t * 30.0) as u32 % 30 == 0) {
        return;
    }
    let mut out = String::new();
    // GB_MAT_DEBUG_BODY=1: paint each beast mesh an unmistakable flat colour (body magenta,
    // head green, eyes yellow) to identify which surface owns each screen region.
    if std::env::var_os("GB_MAT_DEBUG_BODY").is_some() {
        // Two beast rigs exist in the Alley: the costume editor's static rig (far x) and the
        // sim beast. Paint them different colours by world X so a capture names the visible one.
        let world_x: std::collections::HashMap<&str, f32> = positions
            .iter()
            .map(|(n, p)| (n.as_str(), p.translation().x))
            .collect();
        for (name, handle) in &meshes {
            let n = name.as_str();
            let is_actor = n.starts_with("actor_")
                && (n.contains("skinnedMesh") || n.contains("humanoid"));
            if !is_actor {
                continue;
            }
            let far = world_x.get(n).copied().unwrap_or(0.0) < -10.0;
            let color = match (far, n.contains("head")) {
                (true, _) => (1.0, 0.0, 0.0),      // costume-editor rig: red
                (false, true) => (0.0, 1.0, 0.0),  // sim beast head: green
                (false, false) => (0.0, 0.0, 1.0), // sim beast body: blue
            };
            if let Some(m) = materials.get_mut(&handle.0) {
                m.base_color = Color::linear_rgb(color.0, color.1, color.2);
                m.emissive = LinearRgba::BLACK;
                m.metallic = 0.0;
                m.perceptual_roughness = 1.0;
            }
        }
    }
    for (name, pos) in &positions {
        if name.as_str().starts_with("actor_") {
            out.push_str(&format!("pos {:28} {:?}\n", name.as_str(), pos.translation()));
        }
    }
    for (name, handle) in &meshes {
        let n = name.as_str();
        if !n.starts_with("actor_") {
            continue;
        }
        let Some(m) = materials.get(&handle.0) else { continue };
        let l = m.base_color.to_linear();
        out.push_str(&format!(
            "ent {:32} id {:?} base ({:.3},{:.3},{:.3}) e({:.3},{:.3},{:.3}) m {:.2} r {:.2} refl {:.2}\n",
            n, handle.0.id(), l.red, l.green, l.blue,
            m.emissive.red, m.emissive.green, m.emissive.blue,
            m.metallic, m.perceptual_roughness, m.reflectance
        ));
    }
    for (name, handle) in &named {
        let Some(m) = materials.get(&handle.0) else { continue };
        let l = m.base_color.to_linear();
        out.push_str(&format!(
            "named {:24} base ({:.3},{:.3},{:.3}) e({:.3},{:.3},{:.3})\n",
            name.0, l.red, l.green, l.blue,
            m.emissive.red, m.emissive.green, m.emissive.blue
        ));
    }
    let _ = std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/matdump.txt"),
        out,
    );
}

/// Mixed lights in Shadowmask mode: static scenery's shadows are baked (not drawn realtime);
/// only moving objects cast realtime shadows. Scenes marked with this don't cast realtime shadows.
#[derive(Component)]
struct StaticShadowsBaked;

fn baked_static_shadows(
    mut commands: Commands,
    roots: Query<Entity, With<StaticShadowsBaked>>,
    children: Query<&Children>,
    meshes: Query<(), (With<Mesh3d>, Without<bevy::pbr::NotShadowCaster>)>,
) {
    for root in &roots {
        for e in children.iter_descendants(root) {
            if meshes.contains(e) {
                commands.entity(e).insert(bevy::pbr::NotShadowCaster);
            }
        }
    }
}

#[derive(Resource)]
struct StageLightmaps(HashMap<usize, (Handle<Image>, [f32; 4], f32)>);

fn attach_stage_lightmaps(
    mut commands: Commands,
    mut stage: ResMut<StageLightmaps>,
    roots: Query<&SceneInstance, With<play::PhysicsScene>>,
    spawner: Res<SceneSpawner>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    entities: Query<(
        Option<&GltfExtras>,
        Option<&ChildOf>,
        Option<&Mesh3d>,
        Option<&Lightmap>,
        Option<&MeshMaterial3d<StandardMaterial>>,
    )>,
    mut checked: Local<HashSet<Entity>>,
) {
    if stage.0.is_empty() {
        return;
    }
    // Debug isolation knob: render the stage with no baked lightmaps at all
    // (realtime lighting only). Used to A/B a broken decode; not part of the
    // shipping path.
    if std::env::var_os("GB_NO_LIGHTMAPS").is_some() {
        stage.0.clear();
    }
    for instance in &roots {
        for entity in spawner.iter_instance_entities(**instance) {
            if checked.contains(&entity) {
                continue;
            }
            let Ok((_, _, Some(mesh_handle), existing_lightmap, material_handle)) =
                entities.get(entity)
            else {
                continue;
            };
            if existing_lightmap.is_some() {
                checked.insert(entity);
                continue;
            }
            let Some(mesh) = meshes.get(&mesh_handle.0) else {
                continue;
            };
            if mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_none()
                || mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_none()
            {
                checked.insert(entity);
                continue;
            }
            let Some(material_handle) = material_handle else {
                continue;
            };
            let Some(material) = materials.get_mut(&material_handle.0) else {
                continue;
            };

            let mut ancestor = Some(entity);
            let mut node_index = None;
            for _ in 0..16 {
                let Some(current) = ancestor else { break };
                let Ok((extras, child_of, _, _, _)) = entities.get(current) else {
                    break;
                };
                if let Some(extras) = extras {
                    if let Ok(value) = serde_json::from_str::<Value>(&extras.value) {
                        if let Some(index) = value["gb_node"].as_u64() {
                            node_index = Some(index as usize);
                            break;
                        }
                    }
                }
                ancestor = child_of.map(ChildOf::parent);
            }
            let Some((image, scale_offset, exposure)) =
                node_index.and_then(|index| stage.0.get(&index))
            else {
                checked.insert(entity);
                continue;
            };
            // A lightmap whose texture is still loading can be bound as a blank one and stay dark: wait for the image.
            if images.get(image).is_none() {
                continue;
            }
            // Unity lightmap values are in Unity light units; Bevy wants them in its own light units
            // (its lightmap example uses ~10,000). Calibrated visually on the Aquarium: 500 left
            // it flat gray, 50,000 blew it out, 5,000 gives the warm floor of the reference.
            material.lightmap_exposure = *exposure * UNITY_TO_BEVY_LUMINANCE * lightmap_exposure_scale();
            let [scale_x, scale_y, offset_x, offset_y] = *scale_offset;
            commands.entity(entity).insert(Lightmap {
                image: image.clone(),
                uv_rect: Rect::new(
                    offset_x,
                    1.0 - (offset_y + scale_y),
                    offset_x + scale_x,
                    1.0 - offset_y,
                ),
                bicubic_sampling: false,
            });
            checked.insert(entity);
        }
    }
}

/// Same as [`attach_stage_lightmaps`] for surfaces already swapped to the VinylOrMetal extended material
/// (they no longer hold a `StandardMaterial` handle, so the baked GI never reached them).
fn attach_vinyl_lightmaps(
    mut commands: Commands,
    stage: Res<StageLightmaps>,
    roots: Query<&SceneInstance, With<play::PhysicsScene>>,
    spawner: Res<SceneSpawner>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    mut vinyl: ResMut<Assets<vinyl::VinylMaterial>>,
    entities: Query<(
        Option<&GltfExtras>,
        Option<&ChildOf>,
        Option<&Mesh3d>,
        Option<&Lightmap>,
        Option<&MeshMaterial3d<vinyl::VinylMaterial>>,
    )>,
    mut checked: Local<HashSet<Entity>>,
) {
    if stage.0.is_empty() || std::env::var_os("GB_NO_LIGHTMAPS").is_some() {
        return;
    }
    for instance in &roots {
        for entity in spawner.iter_instance_entities(**instance) {
            if checked.contains(&entity) {
                continue;
            }
            let Ok((_, _, Some(mesh_handle), existing, Some(material_handle))) = entities.get(entity) else {
                continue;
            };
            if existing.is_some() {
                checked.insert(entity);
                continue;
            }
            let Some(mesh) = meshes.get(&mesh_handle.0) else { continue };
            if mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_none() || mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_none() {
                checked.insert(entity);
                continue;
            }
            let Some(material) = vinyl.get_mut(&material_handle.0) else { continue };
            let mut ancestor = Some(entity);
            let mut node_index = None;
            for _ in 0..16 {
                let Some(current) = ancestor else { break };
                let Ok((extras, child_of, _, _, _)) = entities.get(current) else { break };
                if let Some(extras) = extras {
                    if let Ok(value) = serde_json::from_str::<Value>(&extras.value) {
                        if let Some(index) = value["gb_node"].as_u64() {
                            node_index = Some(index as usize);
                            break;
                        }
                    }
                }
                ancestor = child_of.map(ChildOf::parent);
            }
            let Some((image, scale_offset, exposure)) = node_index.and_then(|index| stage.0.get(&index)) else {
                checked.insert(entity);
                continue;
            };
            // Wait for the texture (see `attach_stage_lightmaps`).
            if images.get(image).is_none() {
                continue;
            }
            material.base.lightmap_exposure = *exposure * UNITY_TO_BEVY_LUMINANCE * lightmap_exposure_scale();
            let [scale_x, scale_y, offset_x, offset_y] = *scale_offset;
            commands.entity(entity).insert(Lightmap {
                image: image.clone(),
                uv_rect: Rect::new(offset_x, 1.0 - (offset_y + scale_y), offset_x + scale_x, 1.0 - offset_y),
                bicubic_sampling: false,
            });
            checked.insert(entity);
        }
    }
}

fn setting_number(settings: &Value, path: &[&str], default: f32) -> f32 {
    path.iter()
        .fold(settings, |value, key| &value[*key])
        .as_f64()
        .map(|value| value as f32)
        .unwrap_or(default)
}

fn settings_color(value: &Value) -> Color {
    Color::linear_rgba(
        value["r"].as_f64().unwrap_or(0.47) as f32,
        value["g"].as_f64().unwrap_or(0.47) as f32,
        value["b"].as_f64().unwrap_or(0.48) as f32,
        value["a"].as_f64().unwrap_or(1.0) as f32,
    )
}

fn bloom_from_settings(settings: &Value) -> bevy::core_pipeline::bloom::Bloom {
    use bevy::core_pipeline::bloom::{Bloom, BloomPrefilter};
    let intensity = setting_number(
        settings,
        &["global_volume", "overrides", "bloom", "intensity", "value"],
        1.0,
    );
    let threshold = setting_number(
        settings,
        &["global_volume", "overrides", "bloom", "threshold", "value"],
        2.0,
    );
    let scatter = setting_number(
        settings,
        &["global_volume", "overrides", "bloom", "scatter", "value"],
        0.8,
    );
    Bloom {
        // Keep the prior low-intensity calibration while respecting Bevy's energy-conserving
        // range. Large Unity intensities otherwise exceed 1.0 and wash the whole Aquarium out.
        intensity: (intensity * 0.08).clamp(0.0, Bloom::NATURAL.intensity),
        low_frequency_boost: scatter,
        prefilter: BloomPrefilter {
            threshold,
            threshold_softness: 0.8,
        },
        ..Bloom::NATURAL
    }
}

fn color_grading_from_settings(settings: &Value) -> bevy::render::view::ColorGrading {
    use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
    let exposure = setting_number(
        settings,
        &[
            "global_volume",
            "overrides",
            "color_adjustments",
            "postExposure",
            "value",
        ],
        0.0,
    );
    let contrast = setting_number(
        settings,
        &[
            "global_volume",
            "overrides",
            "color_adjustments",
            "contrast",
            "value",
        ],
        0.0,
    );
    let saturation = setting_number(
        settings,
        &[
            "global_volume",
            "overrides",
            "color_adjustments",
            "saturation",
            "value",
        ],
        0.0,
    );
    ColorGrading::with_identical_sections(
        ColorGradingGlobal {
            // Unity URP stores this in EV stops; Bevy's global grading exposure uses the same unit.
            exposure,
            // URP stores contrast and saturation as percentage adjustments around neutral.
            post_saturation: 1.0 + saturation / 100.0,
            ..default()
        },
        ColorGradingSection {
            contrast: 1.0 + contrast / 100.0,
            ..default()
        },
    )
}

/// Camera colour pipeline. Default: the exact URP 12 pass (urp_post.rs) fed by the stage's URP volume
/// (postExposure / contrast / saturation) plus the global look's EV offset; Bevy's tonemapper and
/// grading are disabled. GB_TONEMAP=<bevy tonemapper> falls back to Bevy's for A/B.
fn post_stack(
    settings: &Value,
    stage_name: &str,
) -> (
    bevy::core_pipeline::tonemapping::Tonemapping,
    bevy::render::view::ColorGrading,
    urp_post::UrpPost,
) {
    let look = if stage_name == "menu" {
        look::Look::menu()
    } else if stage_name == "rooftop" {
        look::Look::rooftop()
    } else {
        look::Look::load()
    };
    let v = |key: &str| {
        setting_number(settings, &["global_volume", "overrides", "color_adjustments", key, "value"], 0.0)
    };
    // Incinerator's fog/sky are saturated orange-red and the whole room read as a red wash; pull
    // the colour back a little and lift it (per-stage stand-in until its lightmaps are ported).
    let (sat_adj, ev_adj) = if stage_name == "ring" {
        (std::env::var("GB_RING_SAT").ok().and_then(|x| x.parse().ok()).unwrap_or(0.0), std::env::var("GB_RING_EV").ok().and_then(|x| x.parse().ok()).unwrap_or(-1.0))
    } else if stage_name == "subway" {
        // Subway reads far too dark against the retail platform; lift exposure (stand-in until its lightmaps match).
        (0.0, std::env::var("GB_SUBWAY_EV").ok().and_then(|x| x.parse().ok()).unwrap_or(1.0))
    } else if stage_name == "girders" {
        // The unfinished tower read as dull grey concrete in a pale haze: lift it and bring the warmth back.
        (
            std::env::var("GB_GIRDERS_SAT").ok().and_then(|x| x.parse().ok()).unwrap_or(22.0),
            std::env::var("GB_GIRDERS_EV").ok().and_then(|x| x.parse().ok()).unwrap_or(0.6),
        )
    } else if stage_name == "towers" {
        (
            std::env::var("GB_TOWERS_SAT").ok().and_then(|x| x.parse().ok()).unwrap_or(18.0),
            std::env::var("GB_TOWERS_EV").ok().and_then(|x| x.parse().ok()).unwrap_or(0.35),
        )
    } else if stage_name == "incinerator" {
        (
            std::env::var("GB_INC_SAT").ok().and_then(|x| x.parse().ok()).unwrap_or(0.0),
            std::env::var("GB_INC_EV").ok().and_then(|x| x.parse().ok()).unwrap_or(0.0),
        )
    } else {
        (0.0, 0.0)
    };
    let urp = urp_post::UrpPost::new(v("postExposure") + look.exposure + ev_adj, v("contrast"), v("saturation") + sat_adj);
    match std::env::var("GB_TONEMAP").as_deref() {
        Ok("urp") | Err(_) => (
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            bevy::render::view::ColorGrading::default(),
            urp,
        ),
        // Bevy tonemapper path: the URP pass passes the image through untouched.
        _ => (
            tonemapping(),
            look.grade(color_grading_from_settings(settings)),
            urp_post::UrpPost::bypass(),
        ),
    }
}

/// URP "ACES" tonemapping stand-in; GB_TONEMAP=aces|agx|tony|filmic|reinhard|none (harness A/B).
fn tonemapping() -> bevy::core_pipeline::tonemapping::Tonemapping {
    use bevy::core_pipeline::tonemapping::Tonemapping as T;
    match std::env::var("GB_TONEMAP").as_deref() {
        Ok("agx") => T::AgX,
        Ok("tony") => T::TonyMcMapface,
        Ok("filmic") => T::BlenderFilmic,
        Ok("reinhard") => T::ReinhardLuminance,
        Ok("none") => T::None,
        _ => T::AcesFitted,
    }
}

fn ambient_light_from_settings(settings: &Value) -> AmbientLight {
    let sky = &settings["render_settings"]["ambient_sky_color"];
    let channel =
        |key: &str, fallback: f32| sky[key].as_f64().map_or(fallback, |value| value as f32);
    let intensity = settings["render_settings"]["ambient_intensity"]
        .as_f64()
        .unwrap_or(1.0) as f32;
    AmbientLight {
        color: Color::srgb(channel("r", 0.47), channel("g", 0.47), channel("b", 0.48)),
        // Unity stores this as a multiplier; convert to a restrained indoor fill level.
        brightness: 500.0 * intensity,
        ..default()
    }
}

fn fog_from_settings(settings: &Value) -> DistanceFog {
    let render = &settings["render_settings"];
    DistanceFog {
        color: settings_color(&render["fog_color"]),
        falloff: FogFalloff::Linear {
            start: render["fog_start"].as_f64().unwrap_or(0.0) as f32,
            end: render["fog_end"].as_f64().unwrap_or(300.0) as f32,
        },
        ..default()
    }
}

/// Hold right mouse to look, WASD/QE to move, Shift for speed.
fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    config: Res<ViewerConfig>,
    mut cams: Query<(&mut Transform, &mut FlyCam)>,
) {
    for (mut tf, mut cam) in &mut cams {
        if keys.just_pressed(KeyCode::KeyR) {
            let (eye, target) = config.camera;
            let look = (target - eye).normalize();
            tf.translation = eye;
            cam.yaw = (-look.x).atan2(-look.z);
            cam.pitch = look.y.asin();
        }
        if mouse.pressed(MouseButton::Right) {
            cam.yaw -= motion.delta.x * 0.003;
            cam.pitch = (cam.pitch - motion.delta.y * 0.003).clamp(-1.5, 1.5);
        }
        tf.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
        let mut dir = Vec3::ZERO;
        for (k, d) in [
            (KeyCode::KeyW, *tf.forward()),
            (KeyCode::KeyS, *tf.back()),
            (KeyCode::KeyA, *tf.left()),
            (KeyCode::KeyD, *tf.right()),
            (KeyCode::KeyE, Vec3::Y),
            (KeyCode::KeyQ, Vec3::NEG_Y),
        ] {
            if keys.pressed(k) {
                dir += d;
            }
        }
        let speed = if keys.pressed(KeyCode::ShiftLeft) {
            25.0
        } else {
            6.0
        };
        tf.translation += dir.normalize_or_zero() * speed * time.delta_secs();
    }
}

/// `GB_SCREENSHOT=out.png` saves a frame once every scene has loaded, then quits (for automated checks).
fn auto_screenshot(
    mut commands: Commands,
    mut settled: Local<Option<u32>>,
    mut waited: Local<u32>,
    assets: Res<AssetServer>,
    roots: Query<(&SceneRoot, Option<&SceneInstance>)>,
    meshes: Query<(), With<Mesh3d>>,
    spawner: Res<SceneSpawner>,
    mut exit: EventWriter<AppExit>,
) {
    let Ok(path) = std::env::var("GB_SCREENSHOT") else {
        return;
    };
    if settled.is_none() {
        for (root, _) in &roots {
            if let Some(bevy::asset::LoadState::Failed(e)) = assets.get_load_state(&root.0) {
                error!("scene failed: {e}");
                exit.write(AppExit::error());
                return;
            }
        }
        *waited += 1;
        // Small stages (Alley exports 15 meshes) never reach the 100-mesh heuristic: after ~10 s accept any count.
        if !roots.is_empty()
            && (meshes.iter().count() >= 100 || *waited > 600)
            && roots.iter().all(|(root, instance)| {
                assets.is_loaded_with_dependencies(&root.0)
                    && instance.is_some_and(|id| spawner.instance_is_ready(**id))
            })
        {
            *settled = Some(0);
        }
        return;
    }
    let n = settled.as_mut().unwrap();
    *n += 1;
    let capture_after = std::env::var("GB_SCREENSHOT_AFTER")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(600);
    let exit_after = std::env::var("GB_SCREENSHOT_EXIT_AFTER")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(90);
    if *n == capture_after {
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
    if *n >= capture_after.saturating_add(exit_after) {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_camera_does_not_panic_or_accept_nan() {
        for input in ["", "1,2,3", "1,2,x,4,5,6", "NaN,0,1,0,0,0", "0,0,0,0,0,0"] {
            assert!(parse_camera(input).is_err(), "{input}");
        }
        let (eye, target) = parse_camera("0, 2, 4, 0, 0, 0").unwrap();
        assert_eq!(eye, Vec3::new(0.0, 2.0, 4.0));
        assert_eq!(target, Vec3::ZERO);
    }

    #[test]
    fn maps_urp_post_exposure_to_bevy_exposure_stops() {
        let settings = serde_json::json!({
            "global_volume": {"overrides": {"color_adjustments": {
                "postExposure": {"override": true, "value": 1.4}
            }}}
        });
        let grading = color_grading_from_settings(&settings);
        assert!((grading.global.exposure - 1.4).abs() < f32::EPSILON);
    }

    #[test]
    fn maps_extreme_unity_bloom_to_bevys_energy_conserving_range() {
        let settings = serde_json::json!({
            "global_volume": {"overrides": {"bloom": {"intensity": {"value": 14.75}}}}
        });
        let bloom = bloom_from_settings(&settings);
        assert_eq!(
            bloom.intensity,
            bevy::core_pipeline::bloom::Bloom::NATURAL.intensity
        );
    }

    #[test]
    fn converts_unity_ambient_intensity_to_indoor_fill() {
        let settings = serde_json::json!({
            "render_settings": {"ambient_intensity": 1.68}
        });
        let ambient = ambient_light_from_settings(&settings);
        assert!((ambient.brightness - 840.0).abs() < f32::EPSILON);
    }

    #[test]
    fn converts_face_major_probe_mips_to_bevy_cube_layout() {
        let mut data = Vec::new();
        for face in 0..6u8 {
            for mip in 0..3u8 {
                data.extend(std::iter::repeat_n(face * 10 + mip, 16));
            }
        }
        let probe = StageReflectionProbe {
            transform: Transform::IDENTITY,
            intensity: 1.0,
            width: 4,
            height: 4,
            mip_count: 3,
            face_count: 6,
            face_mip_bytes: 48,
            data,
        };
        let (diffuse, specular) = reflection_probe_images(&probe).unwrap();
        assert_eq!(diffuse.texture_descriptor.size.width, 4);
        let specular = specular.data.unwrap();
        let diffuse = diffuse.data.unwrap();
        for mip in 0..3usize {
            for face in 0..6usize {
                let chunk = &specular[(mip * 6 + face) * 16..(mip * 6 + face + 1) * 16];
                assert!(chunk.iter().all(|byte| *byte == (face * 10 + mip) as u8));
            }
        }
        for face in 0..6usize {
            let chunk = &diffuse[face * 16..(face + 1) * 16];
            assert!(chunk.iter().all(|byte| *byte == (face * 10 + 2) as u8));
        }
    }
}

/// Bevy lightmap exposure per Unity light unit (calibrated on the Aquarium, see above).
const UNITY_TO_BEVY_LUMINANCE: f32 = 5000.0;

/// Reads a `u16` env override (defaults if unset/invalid).
fn environment_u16(key: &str, default: u16) -> u16 {
    // Through the knob store so the in-game panel (F2) can change it at runtime.
    devgui::knob_u16(key, default)
}

/// Screen-space AO constant object thickness (`GB_SSAO_THICKNESS`). Thicker objects make the AO
/// reach further into vents, ducts and wall/floor joins, which is where the references show
/// contact depth. Stages default to 2.0 (raised from 1.0 — the rooftop vents/ducts read flat);
/// the menu keeps 1.0 because its title/lobby look was tuned at that value (2.0 costs 0.1% of the
/// title match).
pub fn ssao_thickness(stage: &str) -> f32 {
    let default = if stage == "menu" { 1.0 } else { 2.0 };
    environment_f32("GB_SSAO_THICKNESS", default)
}

/// Reads a `f32` env override (defaults if unset/invalid).
fn environment_f32(key: &str, default: f32) -> f32 {
    // Through the knob store so the in-game panel (F2) can change it at runtime.
    devgui::knob(key, default)
}

/// Optional runtime scaling of the baked lightmap exposure
/// (`GB_LIGHTMAP_EXPOSURE_SCALE`). 1.0 means "the Unity-authored decode": the
/// stored PNG carries HDR / encoding-exposure, already in Unity's light units,
/// so nothing should need to move. The knob exists so a capture sweep can settle
/// the exact value; when it lands, encode the winner in the extraction rather
/// than leaving the env var set.
fn lightmap_exposure_scale() -> f32 {
    std::env::var("GB_LIGHTMAP_EXPOSURE_SCALE")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(1.0)
}

/// Debug (`GB_LM_DEBUG=1`): once, at frame 400, count the lightmapped entities per material kind.
fn lightmap_debug(
    with_lightmap: Query<(Option<&MeshMaterial3d<StandardMaterial>>, Option<&MeshMaterial3d<vinyl::VinylMaterial>>), With<Lightmap>>,
    all: Query<(Option<&MeshMaterial3d<StandardMaterial>>, Option<&MeshMaterial3d<vinyl::VinylMaterial>>), With<Mesh3d>>,
    mut frames: Local<u32>,
    spots: Query<(&SpotLight, &GlobalTransform)>,
    points: Query<(&PointLight, &GlobalTransform)>,
    suns: Query<&DirectionalLight>,
    ambient: Option<Res<AmbientLight>>,
    vinyl_assets: Res<Assets<vinyl::VinylMaterial>>,
    stage_lm: Res<StageLightmaps>,
    images: Res<Assets<Image>>,
) {
    if std::env::var_os("GB_LM_DEBUG").is_none() {
        return;
    }
    *frames += 1;
    if *frames == 400 {
        let mut seen = std::collections::HashSet::new();
        for (handle, _, _) in stage_lm.0.values() {
            if seen.insert(handle.id()) {
                match images.get(handle) {
                    Some(img) => {
                        let d = img.data.as_ref();
                        let mean = d.map(|d| d.iter().map(|b| *b as f64).sum::<f64>() / d.len().max(1) as f64);
                        info!("lightmap image {:?}: {}x{} mips {} format {:?} mean byte {:?}", handle.id(), img.width(), img.height(), img.texture_descriptor.mip_level_count, img.texture_descriptor.format, mean);
                    }
                    None => info!("lightmap image {:?}: NOT LOADED", handle.id()),
                }
            }
        }
        let mut zero = 0;
        let mut nonzero = 0;
        for (_, v) in with_lightmap.iter() {
            if let Some(h) = v {
                match vinyl_assets.get(&h.0) {
                    Some(m) if m.base.lightmap_exposure > 0.0 => nonzero += 1,
                    Some(_) => zero += 1,
                    None => {}
                }
            }
        }
        info!("lightmap exposure: vinyl lightmapped with exposure>0: {nonzero}, zero: {zero}");
    }
    if *frames % 100 == 0 {
        let s: Vec<String> = spots.iter().map(|(l, t)| format!("spot {:.0}@{:.0},{:.0},{:.0}", l.intensity, t.translation().x, t.translation().y, t.translation().z)).collect();
        let p: Vec<String> = points.iter().map(|(l, t)| format!("point {:.0}@{:.0},{:.0},{:.0}", l.intensity, t.translation().x, t.translation().y, t.translation().z)).collect();
        let d: Vec<String> = suns.iter().map(|l| format!("sun {:.0}", l.illuminance)).collect();
        info!("lights @{}: {:?} {:?} {:?} ambient {:?}", *frames, s, p, d, ambient.map(|a| a.brightness));
    }
    if *frames == 400 {
        let lm_std = with_lightmap.iter().filter(|(s, _)| s.is_some()).count();
        let lm_vin = with_lightmap.iter().filter(|(_, v)| v.is_some()).count();
        let all_std = all.iter().filter(|(s, _)| s.is_some()).count();
        let all_vin = all.iter().filter(|(_, v)| v.is_some()).count();
        info!("lightmap debug: lightmapped std {lm_std} vinyl {lm_vin}; meshes std {all_std} vinyl {all_vin}");
    }
}

/// Re-inserts every `Lightmap` and its material handle a couple of times after load. Some runs rendered the baked GI as if
/// it were absent (walls, pipes and ceilings dark) although the component and its exposure were set: when the lightmap
/// arrives after the mesh was first specialised without it, Bevy does not re-specialise the pipeline. Re-inserting the
/// material handle forces that. `GB_NO_LM_REFRESH=1` disables (A/B).
fn refresh_lightmaps(
    mut commands: Commands,
    lightmapped: Query<(
        Entity,
        &Lightmap,
        Option<&MeshMaterial3d<StandardMaterial>>,
        Option<&MeshMaterial3d<vinyl::VinylMaterial>>,
    )>,
    mut frames: Local<u32>,
) {
    *frames += 1;
    if !matches!(*frames, 90 | 240 | 600) || std::env::var_os("GB_NO_LM_REFRESH").is_some() {
        return;
    }
    for (entity, lightmap, standard, vinyl) in &lightmapped {
        let mut e = commands.entity(entity);
        e.insert(lightmap.clone());
        if let Some(m) = standard {
            e.insert(m.clone());
        }
        if let Some(m) = vinyl {
            e.insert(m.clone());
        }
    }
}
