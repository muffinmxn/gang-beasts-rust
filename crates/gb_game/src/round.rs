//! Melee round flow: GameMode_Gang.IsRoundOver / SendRoundOverMessage / HandleScore and
//! GameManagerNew.EndRoundOrGame (0x5D8320) timings, plus their UI (DegradingTextMessage banner,
//! Coinboard stars, load screen).
//!
//! Local melee: every player is their own gang. A round is over when fewer than two gangs still
//! have a living (not Dead) beast, or, with a single local gang, when it has none.
//! EndRoundOrGame: wait 0.5 s -> round-over message + score -> (not game over) 3 s winner zoom or
//! (game over) 8 s celebration -> 7.5 s end of round -> 5 s load screen -> reload the stage.
use bevy::prelude::*;
use gb_logic::actor::state;

/// GameModeSetupConfiguration wins (the lobby default shown in the menu: "Wins 3").
pub const WINS_TO_WIN: u32 = 3;
const SETTLE: f32 = 0.5;
const WINNER_ZOOM: f32 = 3.0;
const CELEBRATION: f32 = 8.0;
const END_OF_ROUND: f32 = 7.5;
const LOAD_SCREEN: f32 = 5.0;
/// NetServerMessage from SendRoundOverMessage: 3 s, offset (0, -190).
const MESSAGE_LIFE: f32 = 3.0;
const MESSAGE_OFFSET_Y: f32 = -190.0;
/// Rounds never end in the first moments while beasts drop in.
const MIN_ROUND_TIME: f32 = 2.0;

/// Lobby "Mode" row (`GAMEMODE_NAME_*`): Melee = `GameMode_Survival` ("melee"), every beast its own
/// gang; Gang = `GameMode_Gang` ("gang"), balanced teams. Both end a round when fewer than two
/// gangs have a living beast; only the gang assignment differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Melee,
    Gang,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Melee, Mode::Gang];

    pub fn id(self) -> &'static str {
        match self {
            Mode::Melee => "melee",
            Mode::Gang => "gang",
        }
    }

    pub fn from_id(id: &str) -> Mode {
        Mode::ALL.into_iter().find(|m| m.id() == id).unwrap_or_default()
    }

    /// `GAMEMODE_NAME_*` display strings.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Melee => "Melee",
            Mode::Gang => "Gang",
        }
    }

    /// Gang id of player `k`. Gang mode splits the fighters into two balanced teams
    /// (`SetBalancedTeams`); melee is one gang per fighter.
    pub fn team_of(self, k: usize) -> usize {
        match self {
            Mode::Melee => k,
            Mode::Gang => k % 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Playing,
    Settle,
    WinnerZoom,
    EndOfRound,
    Loading,
}

pub struct Message {
    pub text: String,
    pub color: Color,
    pub age: f32,
}

pub struct Round {
    pub phase: Phase,
    pub timer: f32,
    pub round_time: f32,
    pub wins: Vec<u32>,
    pub wins_to_win: u32,
    pub game_over: bool,
    pub message: Option<Message>,
    /// (Unity pose) of every stage body at load, for the reload between rounds.
    pub stage_poses: Vec<(usize, gb_phys::Iso)>,
    /// Player colour (swatch) names: COLOUR_* -> English.
    pub colour_names: Vec<String>,
    pub reset_requested: bool,
    pub mode: Mode,
}

impl Round {
    pub fn new(colour_names: Vec<String>) -> Self {
        Self::with_wins(colour_names, WINS_TO_WIN)
    }

    pub fn with_wins(colour_names: Vec<String>, wins_to_win: u32) -> Self {
        Self {
            phase: Phase::Playing,
            timer: 0.0,
            round_time: 0.0,
            wins: vec![],
            wins_to_win: wins_to_win.clamp(1, 10),
            game_over: false,
            message: None,
            stage_poses: vec![],
            colour_names,
            reset_requested: false,
            mode: Mode::from_id(&std::env::var("GB_MODE").unwrap_or_default()),
        }
    }

    /// One fixed step. `alive[k]`: player k's beast isn't Dead. `colors[k]`: its colour.
    pub fn step(&mut self, dt: f32, alive: &[bool], colors: &[Color]) {
        if self.wins.len() < alive.len() {
            self.wins.resize(alive.len(), 0);
        }
        if let Some(m) = &mut self.message {
            m.age += dt;
            if m.age > MESSAGE_LIFE {
                self.message = None;
            }
        }
        self.timer += dt;
        match self.phase {
            Phase::Playing => {
                self.round_time += dt;
                let mut gangs: Vec<usize> = (0..alive.len())
                    .filter(|&k| alive[k])
                    .map(|k| self.mode.team_of(k))
                    .collect();
                gangs.sort_unstable();
                gangs.dedup();
                let total_gangs = {
                    let mut all: Vec<usize> = (0..alive.len()).map(|k| self.mode.team_of(k)).collect();
                    all.sort_unstable();
                    all.dedup();
                    all.len()
                };
                // IsRoundOver: fewer than two gangs left (a lone local gang: none left).
                let over = if total_gangs >= 2 {
                    gangs.len() < 2
                } else {
                    gangs.is_empty() && !alive.is_empty()
                };
                if over && self.round_time > MIN_ROUND_TIME {
                    self.phase = Phase::Settle;
                    self.timer = 0.0;
                }
            }
            Phase::Settle if self.timer >= SETTLE => {
                let mode = self.mode;
                let mut winners: Vec<usize> = (0..alive.len()).filter(|&k| alive[k]).collect();
                // HandleScore: each winning gang +1 (every member shows it); IsGameOver: a gang
                // reached the win count.
                let winning: Vec<usize> = winners.iter().map(|&k| mode.team_of(k)).collect();
                for k in 0..alive.len() {
                    if winning.contains(&mode.team_of(k)) {
                        self.wins[k] += 1;
                    }
                }
                // One winning gang is reported by its first fighter (its colour).
                if !winners.is_empty() && winners.iter().all(|&k| mode.team_of(k) == mode.team_of(winners[0])) {
                    winners = vec![(0..alive.len()).find(|&k| mode.team_of(k) == mode.team_of(winners[0])).unwrap_or(winners[0])];
                }
                self.game_over = winners.iter().any(|&k| self.wins[k] >= self.wins_to_win);
                // SendRoundOverMessage: one winner -> its colour; otherwise a draw.
                self.message = Some(if winners.len() == 1 {
                    let k = winners[0];
                    let name = self
                        .colour_names
                        .get(k % self.colour_names.len().max(1))
                        .cloned()
                        .unwrap_or_default();
                    let text = if self.game_over {
                        format!("{name} Wins")
                    } else {
                        format!("Spectating {name}")
                    };
                    Message {
                        text,
                        color: colors.get(k).copied().unwrap_or(Color::WHITE),
                        age: 0.0,
                    }
                } else {
                    Message {
                        text: "Draw".into(),
                        color: Color::WHITE,
                        age: 0.0,
                    }
                });
                self.phase = Phase::WinnerZoom;
                self.timer = 0.0;
            }
            Phase::WinnerZoom
                if self.timer
                    >= if self.game_over {
                        CELEBRATION
                    } else {
                        WINNER_ZOOM
                    } =>
            {
                self.phase = Phase::EndOfRound;
                self.timer = 0.0;
            }
            Phase::EndOfRound if self.timer >= END_OF_ROUND => {
                self.phase = Phase::Loading;
                self.timer = 0.0;
            }
            Phase::Loading if self.timer >= LOAD_SCREEN => {
                self.reset_requested = true;
                if self.game_over {
                    self.wins.iter_mut().for_each(|w| *w = 0);
                    self.game_over = false;
                }
                self.phase = Phase::Playing;
                self.timer = 0.0;
                self.round_time = 0.0;
            }
            _ => {}
        }
    }
}

pub fn alive(actor_state: u32) -> bool {
    actor_state != state::DEAD
}

// ------------------------------------------------------------------------------------------ UI

#[derive(Component)]
pub struct RoundMessage;
#[derive(Component)]
pub struct RoundMessageShadow;
#[derive(Component)]
pub struct Coinboard;
#[derive(Component)]
pub struct LoadScreen;

fn ui_font(assets: &AssetServer, file: &str) -> Handle<Font> {
    assets.load(format!("ui/fonts/{file}"))
}

pub fn spawn_ui(mut commands: Commands, assets: Res<AssetServer>) {
    let font = ui_font(&assets, "NotoSans-Black.ttf");
    // DegradingTextMessage: centred 900x60 text, white Outline component -> 4 dark offsets.
    for (dx, dy) in [(-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
        commands.spawn((
            Text::new(""),
            TextFont {
                font: font.clone(),
                font_size: 38.0,
                ..default()
            },
            TextColor(Color::BLACK),
            TextLayout::new_with_justify(JustifyText::Center),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(50.0),
                margin: UiRect {
                    top: Val::Px(-MESSAGE_OFFSET_Y - 30.0 + dy),
                    left: Val::Px(dx),
                    ..default()
                },
                ..default()
            },
            Visibility::Hidden,
            RoundMessageShadow,
        ));
    }
    commands.spawn((
        Text::new(""),
        TextFont {
            font,
            font_size: 38.0,
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::new_with_justify(JustifyText::Center),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            top: Val::Percent(50.0),
            margin: UiRect {
                top: Val::Px(-MESSAGE_OFFSET_Y - 30.0),
                ..default()
            },
            ..default()
        },
        Visibility::Hidden,
        RoundMessage,
    ));
    // Coinboard: one row per player, balloon head + a star balloon per win.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: Val::Px(10.0),
            ..default()
        },
        Visibility::Hidden,
        Coinboard,
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::BLACK),
        Visibility::Hidden,
        LoadScreen,
    ));
}

#[allow(clippy::too_many_arguments)]
pub fn update_ui(
    mut commands: Commands,
    sim: NonSend<crate::play::Sim>,
    assets: Res<AssetServer>,
    mut texts: Query<
        (&mut Text, &mut TextColor, &mut Visibility),
        (With<RoundMessage>, Without<RoundMessageShadow>),
    >,
    mut shadows: Query<
        (&mut Text, &mut TextColor, &mut Visibility),
        (With<RoundMessageShadow>, Without<RoundMessage>),
    >,
    mut board: Query<
        (Entity, &mut Visibility, Option<&Children>),
        (
            With<Coinboard>,
            Without<RoundMessage>,
            Without<RoundMessageShadow>,
            Without<LoadScreen>,
        ),
    >,
    mut load: Query<
        (&mut Visibility, &mut BackgroundColor),
        (
            With<LoadScreen>,
            Without<RoundMessage>,
            Without<RoundMessageShadow>,
            Without<Coinboard>,
        ),
    >,
    mut shown_wins: Local<Vec<u32>>,
) {
    let round = &sim.round;
    // Banner (fades during its last half second, as DegradingTextMessage degrades).
    let (text, color, alpha) = match &round.message {
        Some(m) => (
            m.text.clone(),
            m.color,
            (MESSAGE_LIFE - m.age).clamp(0.0, 0.5) * 2.0,
        ),
        None => (String::new(), Color::WHITE, 0.0),
    };
    for (mut t, mut c, mut v) in &mut texts {
        t.0 = text.clone();
        c.0 = color.with_alpha(alpha);
        *v = if alpha > 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for (mut t, mut c, mut v) in &mut shadows {
        t.0 = text.clone();
        c.0 = Color::BLACK.with_alpha(alpha);
        *v = if alpha > 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    // Coinboard during the end of round.
    let show_board = round.phase == Phase::EndOfRound;
    for (entity, mut v, children) in &mut board {
        *v = if show_board {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *shown_wins != round.wins || children.is_none() {
            *shown_wins = round.wins.clone();
            commands.entity(entity).despawn_related::<Children>();
            let base = assets.load::<Image>("ui/media/BeastBalloonBaseSprite-226f1f73.png");
            let eyes = assets.load::<Image>("ui/media/BeastBalloonEyesSprite-b0bebfd0.png");
            let star = assets.load::<Image>("ui/media/StarBalloonSprite-44b9f68d.png");
            commands.entity(entity).with_children(|rows| {
                for (k, &w) in round.wins.iter().enumerate() {
                    let color = sim.player_color(k);
                    rows.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(10.0),
                        height: Val::Px(100.0),
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            ImageNode::new(base.clone()).with_color(color),
                            Node {
                                width: Val::Px(100.0),
                                height: Val::Px(100.0),
                                ..default()
                            },
                        ))
                        .with_children(|head| {
                            head.spawn((
                                ImageNode::new(eyes.clone()),
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                            ));
                        });
                        for _ in 0..w {
                            row.spawn((
                                ImageNode::new(star.clone())
                                    .with_color(Color::srgb(1.0, 0.87, 0.16)),
                                Node {
                                    width: Val::Px(100.0),
                                    height: Val::Px(100.0),
                                    ..default()
                                },
                            ));
                        }
                    });
                }
            });
        }
    }
    // Load screen: fade to black and hold, fade back in on the new round.
    let fade = match round.phase {
        Phase::Loading => (round.timer / 0.5).min(1.0),
        Phase::Playing if round.round_time < 0.5 => 1.0 - round.round_time / 0.5,
        _ => 0.0,
    };
    for (mut v, mut bg) in &mut load {
        *v = if fade > 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        bg.0 = Color::BLACK.with_alpha(fade);
    }
}

/// COLOUR_* names for the exported player swatches, in swatch order.
pub fn colour_names(root: &std::path::Path) -> Vec<String> {
    let strings: serde_json::Value = std::fs::read(root.join("ui/strings_en.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let colors: serde_json::Value = std::fs::read(root.join("player-colors.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    colors["colors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let code = c["loc_code"].as_str().unwrap_or("");
                    strings[code].as_str().unwrap_or(code).to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_beast_standing_wins_the_round_and_the_stage_reloads() {
        let mut r = Round::new(vec!["Red".into(), "Blue".into()]);
        let colors = [Color::srgb(1.0, 0.0, 0.0), Color::srgb(0.0, 0.0, 1.0)];
        for _ in 0..200 {
            r.step(0.02, &[true, true], &colors);
        }
        assert_eq!(r.phase, Phase::Playing);
        r.step(0.02, &[true, false], &colors);
        assert_eq!(r.phase, Phase::Settle);
        for _ in 0..30 {
            r.step(0.02, &[true, false], &colors);
        }
        assert_eq!(r.wins, vec![1, 0]);
        assert_eq!(r.message.as_ref().unwrap().text, "Spectating Red");
        for _ in 0..((WINNER_ZOOM + END_OF_ROUND + LOAD_SCREEN) / 0.02) as usize + 5 {
            r.step(0.02, &[true, false], &colors);
        }
        assert!(r.reset_requested);
        assert_eq!(r.phase, Phase::Playing);
    }

    #[test]
    fn gang_mode_ends_when_one_team_is_left_and_scores_every_member() {
        let mut r = Round::new(vec!["Red".into(), "Blue".into()]);
        r.mode = Mode::Gang;
        let colors = [Color::WHITE; 4];
        for _ in 0..200 {
            r.step(0.02, &[true, true, true, true], &colors);
        }
        // Players 0 and 2 are one gang, 1 and 3 the other: one fighter per gang left is not over.
        r.step(0.02, &[true, true, false, false], &colors);
        assert_eq!(r.phase, Phase::Playing);
        r.step(0.02, &[true, false, true, false], &colors);
        assert_eq!(r.phase, Phase::Settle);
        for _ in 0..30 {
            r.step(0.02, &[true, false, true, false], &colors);
        }
        assert_eq!(r.wins, vec![1, 0, 1, 0]);
    }
}
