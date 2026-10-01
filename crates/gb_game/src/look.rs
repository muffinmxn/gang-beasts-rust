//! Global look: one set of grading / shadow / fill settings shared by every stage and the menu,
//! tuned against the reference screenshots (referen/). Unity's baked bounce light, sky reflections
//! and soft shadow filtering aren't reproduced 1:1, so this stands in for them globally instead
//! of per-stage hacks. Every value can be overridden with a GB_LOOK_* env var for tuning.
use bevy::prelude::*;

#[derive(Resource, Clone, Copy, Debug)]
pub struct Look {
    /// Share of the sun that still reaches shadowed surfaces (0 = black shadows).
    pub shadow_fill: f32,
    /// Ambient fill level (luminance x brightness), the same on every stage so dim-ambient
    /// scenes (Menu Alley) and bright ones (Rooftop) get the same shadow fill.
    pub ambient: f32,
    /// Ambient tint mixed into the stage ambient color (the references' shadows are blue-grey).
    pub ambient_tint: Vec3,
    pub ambient_tint_mix: f32,
    /// Extra EV on top of the stage's URP postExposure.
    pub exposure: f32,
    /// ASC-CDL gamma (>1 lifts the midtones).
    pub gamma: f32,
    /// Saturation multiplier (<1 softens the colors toward the references).
    pub saturation: f32,
    /// Bounce fill: share of the sun re-emitted, shadowless, from the ground on the far side
    /// (stands in for Unity's baked light probes / bounce that lights faces turned from the sun).
    pub bounce: f32,
    /// Warmth added to the sun's colour (0 = authored colour). Stands in for the warm baked GI
    /// the real game gets from its lightmaps/probes, which we don't reproduce yet.
    pub sun_warmth: f32,
    /// Warmth added to the ambient fill (0 = authored blue-grey sky tint). The real game's baked
    /// GI warms shadowed surfaces; our flat ambient is cool, so this stands in for it.
    pub ambient_warmth: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            shadow_fill: 0.45,
            ambient: 280.0,
            ambient_tint: Vec3::new(0.62, 0.70, 0.85),
            ambient_tint_mix: 0.0,
            exposure: 0.0,
            gamma: 1.1,
            saturation: 0.92,
            bounce: 0.35,
            // The real game's warm tone comes from baked GI we don't reproduce; warming the sun
            // is the closest data-driven stand-in (title match 77.97% -> 80.83%).
            sun_warmth: 1.0,
            ambient_warmth: 0.0,
        }
    }
}

fn env(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Look {
    /// Menu Alley's tuned look (title match 80.83%). Scoped to the menu so Rooftop keeps its
    /// own baseline; the values come from the off-screen tuner against the title reference.
    pub fn menu() -> Self {
        let mut look = Self::load();
        look.exposure = env("GB_LOOK_EV", -0.125);
        look.ambient = env("GB_LOOK_AMBIENT", 240.0);
        look.bounce = env("GB_LOOK_BOUNCE", 0.05);
        look.shadow_fill = env("GB_LOOK_SHADOW_FILL", 0.2).clamp(0.0, 1.0);
        look.ambient_tint_mix = env("GB_LOOK_TINT", 1.0).clamp(0.0, 1.0);
        look.sun_warmth = env("GB_LOOK_SUN_WARMTH", 1.0).clamp(0.0, 2.0);
        look
    }

    /// Rooftop's tuned look. The stage now runs the VinylOrMetal material path (its export is
    /// 19x that shader), which carries its own BRDF and SH ambient, so the shared default fill
    /// leaves it ~30% dark. Measured against the verified Sept-28 Rooftop baseline
    /// (ground 82,62,45 / brick 140,83,47 / AC 102,95,82): ambient 620-700 matches brick and AC,
    /// with the residual split between ground (wants more sky) and AC top (wants less) left for
    /// the directional SH term. Scoped here so Aquarium and every other stage keep their look.
    pub fn rooftop() -> Self {
        let mut look = Self::load();
        // The roof is an OUTDOOR stage: its sun runs at 2,600 lux (the export lists lightmap
        // atlases, but every one is filtered as degenerate, so nothing bakes). With the
        // correct sun the shared default fill (280) is close; the stage needed a little more
        // to keep the AC cream and the brick warm rather than grey.
        look.ambient = env("GB_LOOK_AMBIENT", 380.0);
        // The shared default keeps 45% of the sun in shadowed areas, which reads as "no
        // shadows" on the open roof (stairs/duct/AC should cast defined shadows). The menu
        // runs 0.2; the roof wants the same readable shade.
        look.shadow_fill = env("GB_LOOK_SHADOW_FILL", 0.2).clamp(0.0, 1.0);
        // The sun stays a little warm so the AC units read cream, while the ambient takes the
        // source's cool sky tint (Unity's sky ambient is blue) so the deck floor reads mauve
        // rather than orange-brown. Both are stand-ins until the SH directional term lands.
        look.ambient_tint_mix = env("GB_LOOK_TINT", 1.0).clamp(0.0, 1.0);
        // Less warm sun so the deck floor reads mauve rather than orange; the AC units give up a
        // little cream. The real fix is the SH directional term (warm sun on up-facing tops,
        // cool sky on floors), which the flat ambient+tint can only approximate.
        look.sun_warmth = env("GB_LOOK_SUN_WARMTH", 0.25).clamp(0.0, 2.0);
        // The stronger rooftop sun (main.rs) is balanced by pulling the exposure down, so the
        // near-white skylight frame rolls off through ACES instead of clipping while the deck
        // keeps its light. Stand-in until the baked GI supplies the floor's bounce.
        look.exposure = env("GB_LOOK_EV", -0.75);
        look
    }

    pub fn load() -> Self {
        let d = Self::default();
        Self {
            shadow_fill: env("GB_LOOK_SHADOW_FILL", d.shadow_fill).clamp(0.0, 1.0),
            ambient: env("GB_LOOK_AMBIENT", d.ambient),
            ambient_tint: d.ambient_tint,
            ambient_tint_mix: env("GB_LOOK_TINT", d.ambient_tint_mix).clamp(0.0, 1.0),
            exposure: env("GB_LOOK_EV", d.exposure),
            gamma: env("GB_LOOK_GAMMA", d.gamma),
            saturation: env("GB_LOOK_SAT", d.saturation),
            bounce: env("GB_LOOK_BOUNCE", d.bounce),
            sun_warmth: env("GB_LOOK_SUN_WARMTH", d.sun_warmth).clamp(0.0, 2.0),
            ambient_warmth: env("GB_LOOK_AMBIENT_WARMTH", d.ambient_warmth).clamp(0.0, 2.0),
        }
    }

    /// Warm the sun toward a low-sun amber, standing in for the baked GI the real game has.
    pub fn warm_sun(&self, color: Color) -> Color {
        if self.sun_warmth <= 0.0 {
            return color;
        }
        let c = color.to_linear();
        // Extrapolate past 1.0 so the sweep can find the true optimum.
        let warm = Vec3::new(c.red * 1.12, c.green * 1.0, c.blue * 0.82);
        let base = Vec3::new(c.red, c.green, c.blue);
        let mixed = base + (warm - base) * self.sun_warmth;
        Color::linear_rgb(mixed.x.max(0.0), mixed.y.max(0.0), mixed.z.max(0.0))
    }

    pub fn ambient(&self, mut a: AmbientLight) -> AmbientLight {
        let c = a.color.to_linear();
        let base = Vec3::new(c.red, c.green, c.blue);
        let tinted = base.lerp(self.ambient_tint * base.max_element().max(0.2), self.ambient_tint_mix);
        // Warm the ambient toward the sun's amber to stand in for baked GI bounce.
        let warm = Vec3::new(tinted.x * 1.15, tinted.y * 1.0, tinted.z * 0.78);
        let tinted = tinted + (warm - tinted) * self.ambient_warmth;
        a.color = Color::linear_rgb(tinted.x.max(0.0), tinted.y.max(0.0), tinted.z.max(0.0));
        let luminance = tinted.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(0.01);
        a.brightness = self.ambient / luminance;
        a
    }

    pub fn grade(&self, mut g: bevy::render::view::ColorGrading) -> bevy::render::view::ColorGrading {
        g.global.exposure += self.exposure;
        g.global.post_saturation *= self.saturation;
        for s in [&mut g.shadows, &mut g.midtones, &mut g.highlights] {
            s.gamma *= self.gamma;
        }
        g
    }

    /// Spawns a directional light split into a shadow-casting part and a shadowless part so
    /// shadowed surfaces keep `max(1 - strength, shadow_fill)` of it (Unity Light.shadowStrength).
    pub fn spawn_sun(
        &self,
        commands: &mut Commands,
        color: Color,
        illuminance: f32,
        shadows: bool,
        shadow_strength: f32,
        transform: Transform,
    ) {
        let color = self.warm_sun(color);
        let strength = if shadows {
            shadow_strength.clamp(0.0, 1.0).min(1.0 - self.shadow_fill)
        } else {
            0.0
        };
        if strength > 0.0 {
            commands.spawn((
                DirectionalLight {
                    color,
                    illuminance: illuminance * strength,
                    shadows_enabled: true,
                    shadow_depth_bias: 0.04,
                    // Lights use the URP asset's shadow biases ("Use Pipeline Settings"): normal
                    // bias 1. Zero left speckled acne across the menu wall.
                    shadow_normal_bias: 1.0,
                    ..default()
                },
                bevy::pbr::CascadeShadowConfigBuilder {
                    num_cascades: 4,
                    maximum_distance: 80.0,
                    ..default()
                }
                .build(),
                transform,
            ));
        }
        if self.bounce > 0.0 {
            // Light travels along -Z; mirror the sun's travel direction horizontally and send it
            // upward, as if reflected off the sunlit ground toward the far side.
            let d = transform.forward().as_vec3();
            let bounce = Vec3::new(-d.x, d.y.abs().max(0.2), -d.z).normalize();
            commands.spawn((
                DirectionalLight {
                    color,
                    illuminance: illuminance * self.bounce,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::default().looking_to(bounce, Vec3::Y),
            ));
        }
        if strength < 1.0 {
            commands.spawn((
                DirectionalLight {
                    color,
                    illuminance: illuminance * (1.0 - strength),
                    shadows_enabled: false,
                    ..default()
                },
                transform,
            ));
        }
    }
}
