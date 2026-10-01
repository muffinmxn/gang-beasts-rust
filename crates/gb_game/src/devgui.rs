//! In-game dev panel for the tuning knobs (`GB_*` environment variables).
//!
//! Every knob is a `f32` in a process-global table that the renderer/physics read through
//! [`knob`]. The table starts from the environment (so every existing harness invocation keeps
//! working) and the panel edits it live.
//!
//! Panel (F2): up/down select, left/right adjust (hold Shift for 10x), `R` reset to the default,
//! `A` re-apply the knobs that are consumed at build time (beast/costume colours, material
//! conversion, sun scale), `F2` hide. Values are printed on the panel and also logged when
//! changed, so a capture can be traced back to its settings.
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// What has to happen when a knob changes (besides the live value).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// Read every frame: nothing else to do.
    Live,
    /// Beast body colour / costume tint: re-tint the actors.
    Retint,
    /// Stage material conversion (vinyl settings): rebuild the converted materials.
    Materials,
}

pub struct Knob {
    pub key: &'static str,
    pub label: &'static str,
    pub value: f32,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub apply: Apply,
}

/// The knob table. Defaults here must match the code's own defaults (they are the value used
/// when neither the panel nor the environment sets the key).
const TABLE: &[(&str, &str, f32, f32, f32, f32, Apply)] = &[
    ("GB_PUNCH_SCALE", "punch strength", 0.7, 0.0, 3.0, 0.05, Apply::Live),
    ("GB_LIFT_SCALE", "lift/throw strength", 0.6, 0.0, 3.0, 0.05, Apply::Live),
    ("GB_TARGET_FOV", "targeting FOV", 75.0, 20.0, 120.0, 5.0, Apply::Live),
    ("GB_TARGET_RANGE", "targeting range", 1.6, 0.5, 3.0, 0.1, Apply::Live),
    ("GB_TARGET_PULL", "grab pull", 0.6, 0.0, 2.0, 0.05, Apply::Live),
    ("GB_CAM_ZOOM", "camera zoom", 1.06, 0.5, 3.0, 0.02, Apply::Live),
    ("GB_SSAO_THICKNESS", "SSAO thickness", 2.0, 0.0, 6.0, 0.1, Apply::Live),
    ("GB_POST_SHADOW", "shadow depth", 1.0, 1.0, 3.0, 0.05, Apply::Live),
    ("GB_POST_SHADOW_KNEE", "shadow knee", 0.45, 0.05, 1.0, 0.01, Apply::Live),
    ("GB_POST_VIGNETTE", "vignette", 0.0, 0.0, 1.0, 0.05, Apply::Live),
    ("GB_POST_CA", "chromatic aberration", 0.0, 0.0, 1.0, 0.05, Apply::Live),
    ("GB_POST_GRAIN", "film grain", 0.0, 0.0, 1.0, 0.05, Apply::Live),
    ("GB_SH_SCALE", "SH ambient", 0.45, 0.0, 2.0, 0.05, Apply::Materials),
    ("GB_SMOOTHNESS_SCALE", "vinyl smoothness", 1.0, 0.0, 2.0, 0.05, Apply::Materials),
    ("GB_SUN_SCALE", "sun scale", 2.35, 0.0, 5.0, 0.05, Apply::Live),
    ("GB_AMBIENT_SCALE", "ambient scale", 1.0, 0.0, 3.0, 0.05, Apply::Live),
    ("GB_BEAST_TINT", "beast albedo (x)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
    ("GB_BEAST_TINT_Y", "beast albedo (y)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
    ("GB_BEAST_TINT_Z", "beast albedo (z)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
    ("GB_COSTUME_TINT", "costume albedo (x)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
    ("GB_COSTUME_TINT_Y", "costume albedo (y)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
    ("GB_COSTUME_TINT_Z", "costume albedo (z)", 1.0, 0.0, 2.0, 0.02, Apply::Retint),
];

/// Process-global knob values (started from the environment on first read).
fn store() -> &'static Mutex<HashMap<&'static str, f32>> {
    static STORE: OnceLock<Mutex<HashMap<&'static str, f32>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn env_f32(key: &str) -> Option<f32> {
    std::env::var(key).ok().and_then(|v| v.parse::<f32>().ok())
}

/// Reads a knob: the panel value if it was set, else the environment, else `default`.
pub fn knob(key: &str, default: f32) -> f32 {
    let key: &'static str = match TABLE.iter().find(|(k, ..)| *k == key) {
        Some((k, ..)) => k,
        None => Box::leak(key.to_string().into_boxed_str()),
    };
    if let Ok(store) = store().lock() {
        if let Some(value) = store.get(key) {
            return *value;
        }
    }
    if let Some(value) = env_f32(key) {
        // First read: seed the table so the panel shows the environment value.
        if let Ok(mut store) = store().lock() {
            store.insert(key, value);
        }
        return value;
    }
    default
}

/// Same as [`knob`] but returns all three components of a `"r,g,b"` knob family
/// (`GB_BEAST_TINT` + `GB_BEAST_TINT_Y/Z`, `GB_COSTUME_TINT` + `_Y/_Z`).
pub fn knob_rgb(key: &str, default: [f32; 3]) -> [f32; 3] {
    let parse = |raw: String| -> Vec<f32> {
        raw.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect()
    };
    // The panel stores each component separately; the environment stores "r,g,b".
    let from_panel = |_i: usize, suffix: &str| -> Option<f32> {
        let k = if suffix.is_empty() { key.to_string() } else { format!("{key}{suffix}") };
        let store = store().lock().ok()?;
        store.get(k.as_str()).copied()
    };
    let env = std::env::var(key).ok().map(parse).unwrap_or_default();
    let mut out = default;
    for (i, suffix) in ["", "_Y", "_Z"].iter().enumerate() {
        if let Some(v) = from_panel(i, suffix) {
            out[i] = v;
        } else if let Some(v) = env.get(i) {
            out[i] = *v;
        }
    }
    out
}

/// `u16` knob (e.g. `GB_ANISO`).
pub fn knob_u16(key: &str, default: u16) -> u16 {
    knob(key, default as f32).clamp(0.0, u16::MAX as f32) as u16
}

/// Sets a knob from code (the panel and the harness both go through here).
pub fn set_knob(key: &str, value: f32) {
    let key: &'static str = match TABLE.iter().find(|(k, ..)| *k == key) {
        Some((k, ..)) => k,
        None => Box::leak(key.to_string().into_boxed_str()),
    };
    if let Ok(mut store) = store().lock() {
        store.insert(key, value);
    }
}

#[derive(Resource)]
pub struct DevPanel {
    pub knobs: Vec<Knob>,
    pub selected: usize,
    pub visible: bool,
    /// Set when a knob changed in a way that needs the world rebuilt (see [`Apply`]).
    pub reapply: bool,
}

impl Default for DevPanel {
    fn default() -> Self {
        let knobs = TABLE
            .iter()
            .map(|(key, label, default, min, max, step, apply)| Knob {
                key,
                label,
                value: knob(key, *default),
                default: *default,
                min: *min,
                max: *max,
                step: *step,
                apply: *apply,
            })
            .collect();
        Self { knobs, selected: 0, visible: false, reapply: false }
    }
}

impl DevPanel {
    pub fn value(&self, key: &str) -> f32 {
        self.knobs
            .iter()
            .find(|k| k.key == key)
            .map_or_else(|| knob(key, 0.0), |k| k.value)
    }
}

pub struct DevPanelPlugin;

impl Plugin for DevPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DevPanel>()
            .add_systems(Startup, spawn_panel)
            .add_systems(Update, (panel_input, push_scales, panel_text).chain());
    }
}

/// Pushes the gameplay knobs into `gb_logic` (which cannot see Bevy resources).
fn push_scales(panel: Res<DevPanel>) {
    let punch = panel.value("GB_PUNCH_SCALE");
    let lift = panel.value("GB_LIFT_SCALE");
    gb_logic::actor::set_scale_knobs(punch.max(0.0), lift.max(0.0));
    gb_logic::actor::set_target_knobs(
        panel.value("GB_TARGET_FOV"),
        panel.value("GB_TARGET_RANGE"),
        panel.value("GB_TARGET_PULL"),
    );
}

#[derive(Component)]
struct PanelText;

fn spawn_panel(mut commands: Commands, mut panel: ResMut<DevPanel>) {
    // `GB_PANEL=1` starts with the panel open (captures cannot press F2).
    if std::env::var_os("GB_PANEL").is_some() {
        panel.visible = true;
    }
    commands.spawn((
        PanelText,
        Text::new(""),
        TextFont { font_size: 14.0, ..default() },
        TextColor(Color::srgb(0.9, 0.95, 1.0)),
        // Above the menu's UI.
        GlobalZIndex(1000),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            top: Val::Px(80.0),
            max_width: Val::Px(560.0),
            ..default()
        },
    ));
    info!("dev panel: F2 toggles ({} knobs)", panel.knobs.len());
}

fn panel_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut panel: ResMut<DevPanel>,
    mut applied: Local<Vec<String>>,
) {
    if keys.just_pressed(KeyCode::F2) || keys.just_pressed(KeyCode::Backquote) {
        panel.visible = !panel.visible;
    }
    if !panel.visible || panel.knobs.is_empty() {
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let n = panel.knobs.len();
    if keys.just_pressed(KeyCode::ArrowDown) {
        panel.selected = (panel.selected + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        panel.selected = (panel.selected + n - 1) % n;
    }
    let mut delta = 0.0;
    if keys.just_pressed(KeyCode::ArrowRight) {
        delta = panel.knobs[panel.selected].step;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        delta = -panel.knobs[panel.selected].step;
    }
    if delta != 0.0 {
        let selected = panel.selected;
        let knob = &mut panel.knobs[selected];
        knob.value = (knob.value + delta * if shift { 10.0 } else { 1.0 }).clamp(knob.min, knob.max);
        let (key, value, apply) = (knob.key, knob.value, knob.apply);
        set_knob(key, value);
        if apply != Apply::Live {
            panel.reapply = true;
        }
        applied.push(format!("{key}={value}"));
        info!("dev panel: {key} = {value}");
    }
    if keys.just_pressed(KeyCode::KeyR) {
        let selected = panel.selected;
        let knob = &mut panel.knobs[selected];
        knob.value = knob.default;
        let (key, value, apply) = (knob.key, knob.value, knob.apply);
        set_knob(key, value);
        if apply != Apply::Live {
            panel.reapply = true;
        }
        info!("dev panel: {key} reset to {value}");
    }
    // A: re-apply everything consumed at build time.
    if keys.just_pressed(KeyCode::KeyA) {
        for knob in &panel.knobs {
            set_knob(knob.key, knob.value);
        }
        panel.reapply = true;
        info!("dev panel: re-applied");
    }
}

fn panel_text(panel: Res<DevPanel>, mut texts: Query<&mut Text, With<PanelText>>) {
    let Ok(mut text) = texts.single_mut() else { return };
    if !panel.visible {
        if !text.0.is_empty() {
            text.0.clear();
        }
        return;
    }
    let mut out = String::from("F2 / ` hide   arrows adjust   Shift=10x   R reset   A re-apply\n");
    for (i, knob) in panel.knobs.iter().enumerate() {
        let marker = if i == panel.selected { ">" } else { " " };
        let changed = (knob.value - knob.default).abs() > 1e-6;
        out.push_str(&format!(
            "{marker} {:22} {:8.2}{}\n",
            knob.label,
            knob.value,
            if changed { "  *" } else { "" }
        ));
    }
    text.0 = out;
}
