//! Loading screen for in-process stage switches: the game's black load screen with the spinning
//! Boneloaf (menu `Lobby Loading/Spinner`, AutoMoveAndRotate) and "Loading" in the menu font.
//! Shown before the blocking load, kept up until the new stage's scene has spawned and a few
//! frames have rendered (so lightmaps / mips / costumes don't pop in visibly).
use bevy::prelude::*;
use bevy::scene::{SceneInstance, SceneSpawner};

#[derive(Resource, Default)]
pub struct Loading {
    visible: bool,
    /// Waiting for the new stage's scenes to finish spawning.
    waiting: bool,
    /// Frames rendered since the scenes became ready.
    settled: u32,
}

impl Loading {
    pub fn show(&mut self) {
        self.visible = true;
        self.waiting = false;
        self.settled = 0;
    }
    pub fn hide(&mut self) {
        self.visible = false;
        self.waiting = false;
    }
    pub fn until_scene_ready(&mut self) {
        self.visible = true;
        self.waiting = true;
        self.settled = 0;
    }
}

#[derive(Component)]
struct LoadingRoot;

#[derive(Component)]
struct Spinner;

pub fn plugin(app: &mut App) {
    app.init_resource::<Loading>()
        .add_systems(Startup, spawn)
        .add_systems(Update, (settle, show, spin).chain());
}

fn spawn(mut commands: Commands, assets: Res<AssetServer>) {
    let font: Handle<Font> = assets.load("ui/fonts/snasm_bd.otf");
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(1000),
            Visibility::Hidden,
            LoadingRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(48.0),
                    bottom: Val::Px(40.0),
                    width: Val::Px(72.0),
                    height: Val::Px(72.0),
                    ..default()
                },
                ImageNode::new(assets.load("ui/media/boneloaf 2-3519e919.png")),
                Spinner,
            ));
            root.spawn((
                Text::new("Loading"),
                TextFont { font, font_size: 36.0, ..default() },
                TextColor(Color::srgb(1.0, 0.752, 0.0)),
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(136.0),
                    bottom: Val::Px(56.0),
                    ..default()
                },
            ));
        });
}

/// Hide once every scene root has spawned and 20 frames have rendered on top of it.
fn settle(
    mut loading: ResMut<Loading>,
    roots: Query<Option<&SceneInstance>, With<SceneRoot>>,
    spawner: Res<SceneSpawner>,
) {
    if !loading.waiting {
        return;
    }
    let ready = roots
        .iter()
        .all(|instance| instance.is_some_and(|id| spawner.instance_is_ready(**id)));
    if !ready {
        loading.settled = 0;
        return;
    }
    loading.settled += 1;
    if loading.settled > 20 {
        loading.hide();
    }
}

fn show(loading: Res<Loading>, mut root: Query<&mut Visibility, With<LoadingRoot>>) {
    if !loading.is_changed() {
        return;
    }
    for mut v in &mut root {
        *v = if loading.visible { Visibility::Visible } else { Visibility::Hidden };
    }
}

fn spin(time: Res<Time>, loading: Res<Loading>, mut spinners: Query<&mut Transform, With<Spinner>>) {
    if !loading.visible {
        return;
    }
    for mut t in &mut spinners {
        t.rotate_z(-time.delta_secs() * 4.0);
    }
}
