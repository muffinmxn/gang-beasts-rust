//! `Fracture` (0x7E3E60 OnCollisionEnter / Damage): breakable glass. Each collision takes the
//! impact speed (|Collision.relativeVelocity|) off `FractureSettingsData.Health`; at zero the
//! pane shatters (EnableFracture). The game Voronoi-splits the mesh into 0.1-0.3 m shards (at most
//! MaxShards); this port removes the pane and spawns box shards of those sizes in its bounds.
use bevy::prelude::*;
use gb_phys::source::{mirror_position, mirror_rotation};
use gb_phys::{ContactKind, Iso, World};

pub struct Glass {
    pub body: usize,
    pub node: usize,
    pub collider: Option<usize>,
    pub health: f32,
    pub min_shard: f32,
    pub max_shard: f32,
    pub max_shards: usize,
    pub broken: bool,
}

pub struct Shard {
    pub body: usize,
    pub half: Vec3,
    pub age: f32,
    pub entity: Option<Entity>,
}

#[derive(Default)]
pub struct Fractures {
    pub glass: Vec<Glass>,
    pub shards: Vec<Shard>,
    /// Scene nodes whose render entity must be hidden (scene index, node).
    pub hide: Vec<(usize, usize)>,
    /// Expired shard visuals to despawn.
    pub hide_entities: Vec<Entity>,
    /// Scene nodes to show again (stage reload).
    pub show: Vec<(usize, usize)>,
    initial_health: f32,
    seed: u32,
}

const SHARD_LIFETIME: f32 = 15.0;

impl Fractures {
    /// Every active node with a `Fracture` script and a simulated Rigidbody.
    pub fn from_scene(world: &World, instance: usize, src: &gb_phys::Sidecar) -> Self {
        let mut f = Fractures {
            seed: 0x9e37_79b9,
            ..default()
        };
        for (i, n) in src.nodes.iter().enumerate() {
            let Some(c) = n
                .components
                .iter()
                .find(|c| c.script.as_deref() == Some("Fracture"))
            else {
                continue;
            };
            let Some(&body) = world.instances[instance].bodies.get(&i) else {
                continue;
            };
            let d = &c.data["FractureSettingsData"];
            let num = |k: &str, def: f32| d[k].as_f64().map_or(def, |v| v as f32);
            f.glass.push(Glass {
                body,
                node: i,
                collider: world
                    .colliders
                    .iter()
                    .position(|c| c.body == Some(body) && !c.trigger),
                health: num("Health", 1.0),
                min_shard: num("MinShardSize", 0.1),
                max_shard: num("MaxShardSize", 0.3),
                max_shards: d["MaxShards"].as_u64().unwrap_or(75) as usize,
                broken: false,
            });
        }
        f.initial_health = f.glass.first().map_or(1.0, |g| g.health);
        f
    }

    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed & 0xff_ffff) as f32 / 16_777_216.0
    }

    /// Stage reload: panes whole again, shards gone.
    pub fn reset(&mut self, world: &mut World) {
        for g in &mut self.glass {
            if g.broken {
                g.broken = false;
                self.show.push((0, g.node));
            }
            g.health = self.initial_health;
        }
        for s in self.shards.drain(..) {
            world.remove_body(s.body);
            if let Some(e) = s.entity {
                self.hide_entities.push(e);
            }
        }
    }

    /// After a physics step: apply impacts, shatter panes, age shards.
    pub fn step(&mut self, world: &mut World, dt: f32) {
        for k in 0..self.glass.len() {
            if self.glass[k].broken {
                continue;
            }
            let body = self.glass[k].body;
            let mut hit: Option<(Vec3, Vec3)> = None;
            for c in &world.contacts {
                if c.kind != ContactKind::Enter || world.actors[c.this].body != Some(body) {
                    continue;
                }
                self.glass[k].health -= c.relative_velocity.length();
                let at = c
                    .points
                    .first()
                    .map_or(world.pose(body).position, |p| p.position);
                hit = Some((at, c.relative_velocity));
            }
            if self.glass[k].health <= 0.0 {
                let (at, rel) = hit.unwrap_or((world.pose(body).position, Vec3::ZERO));
                self.shatter(world, k, at, rel);
            }
        }
        let mut k = 0;
        while k < self.shards.len() {
            self.shards[k].age += dt;
            if self.shards[k].age > SHARD_LIFETIME {
                world.remove_body(self.shards[k].body);
                if let Some(e) = self.shards[k].entity {
                    // Despawned by the render system when it sees the shard gone.
                    self.hide_entities.push(e);
                }
                self.shards.swap_remove(k);
            } else {
                k += 1;
            }
        }
    }

    fn shatter(&mut self, world: &mut World, k: usize, at: Vec3, rel: Vec3) {
        let g = &self.glass[k];
        let (body, node) = (g.body, g.node);
        let (min_s, max_s, max_n) = (g.min_shard, g.max_shard, g.max_shards);
        let (center, ext) = g
            .collider
            .map_or((world.pose(body).position, Vec3::splat(0.5)), |c| {
                world.collider_bounds(c)
            });
        let velocity = world.linear_velocity(body);
        let mass = world.mass(body);
        world.remove_body(body);
        self.glass[k].broken = true;
        self.hide.push((0, node));
        // Thin axis of the pane's bounds is the glass thickness.
        let thin = if ext.x <= ext.y && ext.x <= ext.z {
            0
        } else if ext.y <= ext.z {
            1
        } else {
            2
        };
        let area: f32 = (0..3)
            .filter(|&a| a != thin)
            .map(|a| ext[a] * 2.0)
            .product();
        let avg = (min_s + max_s) * 0.5;
        let count = ((area / (avg * avg)).ceil() as usize).clamp(1, max_n);
        let shard_mass = mass / count as f32;
        for _ in 0..count {
            let size = min_s + (max_s - min_s) * self.rand();
            let mut half = Vec3::splat(size * 0.5);
            half[thin] = (ext[thin]).clamp(0.005, 0.02);
            let offset = Vec3::new(
                (self.rand() * 2.0 - 1.0) * ext.x,
                (self.rand() * 2.0 - 1.0) * ext.y,
                (self.rand() * 2.0 - 1.0) * ext.z,
            );
            let pos = center + offset;
            // Shards near the impact carry some of the hitter's motion.
            let near = (1.0 - (pos - at).length() / (ext.length() + 0.01)).clamp(0.0, 1.0);
            let v = velocity - rel * 0.3 * near
                + Vec3::new(self.rand() - 0.5, self.rand() - 0.5, self.rand() - 0.5);
            let rot = Quat::from_euler(
                EulerRot::XYZ,
                self.rand() * 0.6,
                self.rand() * 0.6,
                self.rand() * 0.6,
            );
            let b = world.add_box(Iso::new(pos, rot), half, shard_mass, 0, v);
            self.shards.push(Shard {
                body: b,
                half,
                age: 0.0,
                entity: None,
            });
        }
    }
}

#[derive(Component)]
pub struct ShardVisual;

/// Hide shattered panes, spawn shard visuals and keep them on their bodies.
pub fn render(
    mut commands: Commands,
    mut sim: NonSendMut<crate::play::Sim>,
    map: Res<crate::play::NodeEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    mut transforms: Query<&mut Transform, With<ShardVisual>>,
) {
    let sim = &mut *sim;
    for key in std::mem::take(&mut sim.fractures.hide) {
        if let Some(&e) = map.0.get(&key) {
            commands.entity(e).insert(Visibility::Hidden);
        }
    }
    for key in std::mem::take(&mut sim.fractures.show) {
        if let Some(&e) = map.0.get(&key) {
            commands.entity(e).insert(Visibility::Inherited);
        }
    }
    for e in std::mem::take(&mut sim.fractures.hide_entities) {
        commands.entity(e).despawn();
    }
    let mat = material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.55, 0.82, 0.85, 0.6),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.2,
                ..default()
            })
        })
        .clone();
    for s in &mut sim.fractures.shards {
        let pose = sim.world.pose(s.body);
        let tf = Transform {
            translation: mirror_position(pose.position),
            rotation: mirror_rotation(pose.rotation),
            scale: Vec3::ONE,
        };
        match s.entity {
            Some(e) => {
                if let Ok(mut t) = transforms.get_mut(e) {
                    *t = tf;
                }
            }
            None => {
                let e = commands
                    .spawn((
                        Mesh3d(meshes.add(Cuboid::new(
                            s.half.x * 2.0,
                            s.half.y * 2.0,
                            s.half.z * 2.0,
                        ))),
                        MeshMaterial3d(mat.clone()),
                        tf,
                        ShardVisual,
                    ))
                    .id();
                s.entity = Some(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gb_phys::{Pose, Settings, Sidecar};

    #[test]
    fn dropping_something_on_the_skylight_shatters_it() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/export");
        if !root.join("rooftop.json").exists() {
            return;
        }
        let mut world = World::new(Settings::load(&root).unwrap()).unwrap();
        let src = Sidecar::load(&root, "rooftop").unwrap();
        let inst = world.spawn("rooftop", &src, Pose::IDENTITY).unwrap();
        let mut f = Fractures::from_scene(&world, inst, &src);
        assert!(!f.glass.is_empty(), "no Fracture panes");
        let pane = world
            .collider_bounds(f.glass[0].collider.expect("pane collider"))
            .0;
        let b = world.add_box(
            Iso::new(pane + Vec3::Y * 3.0, Quat::IDENTITY),
            Vec3::splat(0.2),
            50.0,
            0,
            Vec3::ZERO,
        );
        let mut enters = 0;
        for _ in 0..100 {
            world.step();
            enters += world
                .contacts
                .iter()
                .filter(|c| {
                    c.kind == ContactKind::Enter
                        && world.actors[c.this].body == Some(f.glass[0].body)
                })
                .count();
            f.step(&mut world, 0.02);
        }
        println!(
            "pane at {pane:?}, box ended at {:?}, glass enters {enters}, health {}",
            world.pose(b).position,
            f.glass[0].health
        );
        let broken = f.glass.iter().filter(|g| g.broken).count();
        println!(
            "glass: {} panes, {} broken, {} shards",
            f.glass.len(),
            broken,
            f.shards.len()
        );
        assert!(broken >= 1 && !f.shards.is_empty());
    }
}
