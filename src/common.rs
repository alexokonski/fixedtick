use bevy::{
    log::LogPlugin,
    math::bounding::{Aabb2d, BoundingCircle, BoundingVolume, IntersectsVolume},
    prelude::*,
    sprite::MaterialMesh2dBundle,
};
use bevy::ecs::system::EntityCommands;
use serde::Serialize;
use serde::Deserialize;

// These constants are defined in `Transform` units.
// Using the default 2D camera they correspond 1:1 with screen pixels.
pub const PADDLE_SIZE: Vec2 = Vec2::new(120.0, 20.0);
pub const GAP_BETWEEN_PADDLE_AND_FLOOR: f32 = 60.0;
pub const PADDLE_SPEED: f32 = 500.0;
// How close can the paddle get to the wall
pub const PADDLE_PADDING: f32 = 10.0;

// We set the z-value of the ball to 1 (WHEN SPAWNING, NOT HERE) so it renders on top in the case of overlapping sprites.
pub const BALL_STARTING_POSITION: Vec2 = Vec2::new(0.0, -50.0);
pub const BALL_DIAMETER: f32 = 30.;
pub const BALL_SPEED: f32 = 400.0;
pub const INITIAL_BALL_DIRECTION: Vec2 = Vec2::new(0.5, -0.5);

pub const WALL_THICKNESS: f32 = 10.0;
// x coordinates
pub const LEFT_WALL: f32 = -450.;
pub const RIGHT_WALL: f32 = 450.;
// y coordinates
pub const BOTTOM_WALL: f32 = -300.;
pub const PADDLE_Y: f32 = BOTTOM_WALL + GAP_BETWEEN_PADDLE_AND_FLOOR;
pub const TOP_WALL: f32 = 300.;

pub const BRICK_SIZE: Vec2 = Vec2::new(100., 30.);
// These values are exact
pub const GAP_BETWEEN_PADDLE_AND_BRICKS: f32 = 270.0;
pub const GAP_BETWEEN_BRICKS: f32 = 5.0;
// These values are lower bounds, as the number of bricks is computed
pub const GAP_BETWEEN_BRICKS_AND_CEILING: f32 = 20.0;
pub const GAP_BETWEEN_BRICKS_AND_SIDES: f32 = 20.0;

pub const SCOREBOARD_FONT_SIZE: f32 = 40.0;
pub const SCOREBOARD_TEXT_PADDING: Val = Val::Px(5.0);

pub const BACKGROUND_COLOR: Color = Color::srgb(0.9, 0.9, 0.9);
pub const PADDLE_COLOR: Color = Color::srgb(0.3, 0.3, 0.7);
pub const BALL_COLOR: Color = Color::srgb(1.0, 0.5, 0.5);
pub const BRICK_COLOR: Color = Color::srgb(0.5, 0.5, 1.0);
pub const WALL_COLOR: Color = Color::srgb(0.8, 0.8, 0.8);
pub const TEXT_COLOR: Color = Color::srgb(0.5, 0.5, 1.0);
pub const SCORE_COLOR: Color = Color::srgb(1.0, 0.5, 0.5);

#[derive(Component)]
pub struct Paddle;

#[derive(Component)]
pub struct Ball;

#[derive(Component, Deref, DerefMut)]
pub struct Velocity(pub Vec2);

#[derive(Component)]
pub struct Collider;

#[derive(Event, Default)]
pub struct CollisionEvent;

#[derive(Component)]
pub struct Brick;

// This bundle is a collection of the components that define a "wall" in our game
#[derive(Bundle)]
pub struct WallBundle {
    // You can nest bundles inside of other bundles like this
    // Allowing you to compose their functionality
    sprite_bundle: SpriteBundle,
    collider: Collider,
}

/// Which side of the arena is this wall located on?
pub enum WallLocation {
    Left,
    Right,
    Bottom,
    Top,
}

impl WallLocation {
    /// Location of the *center* of the wall, used in `transform.translation()`
    fn position(&self) -> Vec2 {
        match self {
            WallLocation::Left => Vec2::new(LEFT_WALL, 0.),
            WallLocation::Right => Vec2::new(RIGHT_WALL, 0.),
            WallLocation::Bottom => Vec2::new(0., BOTTOM_WALL),
            WallLocation::Top => Vec2::new(0., TOP_WALL),
        }
    }

    /// (x, y) dimensions of the wall, used in `transform.scale()`
    fn size(&self) -> Vec2 {
        let arena_height = TOP_WALL - BOTTOM_WALL;
        let arena_width = RIGHT_WALL - LEFT_WALL;
        // Make sure we haven't messed up our constants
        assert!(arena_height > 0.0);
        assert!(arena_width > 0.0);

        match self {
            WallLocation::Left | WallLocation::Right => {
                Vec2::new(WALL_THICKNESS, arena_height + WALL_THICKNESS)
            }
            WallLocation::Bottom | WallLocation::Top => {
                Vec2::new(arena_width + WALL_THICKNESS, WALL_THICKNESS)
            }
        }
    }
}
impl WallBundle {
    // This "builder method" allows us to reuse logic across our wall entities,
    // making our code easier to read and less prone to bugs when we change the logic
    pub fn new(location: WallLocation) -> WallBundle {
        WallBundle {
            sprite_bundle: SpriteBundle {
                transform: Transform {
                    // We need to convert our Vec2 into a Vec3, by giving it a z-coordinate
                    // This is used to determine the order of our sprites
                    translation: location.position().extend(0.0),
                    // The z-scale of 2D objects must always be 1.0,
                    // or their ordering will be affected in surprising ways.
                    // See https://github.com/bevyengine/bevy/issues/4149
                    scale: location.size().extend(1.0),
                    ..default()
                },
                sprite: Sprite {
                    color: WALL_COLOR,
                    ..default()
                },
                ..default()
            },
            collider: Collider,
        }
    }
}

// This resource tracks the game's score
#[derive(Resource)]
pub struct Score(pub u32, pub NetId);

#[derive(Component)]
pub struct ScoreboardUi;

#[repr(u8)]
pub enum NetKey {
    Left,
    Right,
}

#[derive(Deserialize, Serialize)]
pub struct PlayerInput {
    pub key_mask: u8,
    pub simulating_frame: u32
}

#[derive(Deserialize, Serialize)]
pub enum ClientToServerPacket {
    Input(PlayerInput)
}

#[derive(Deserialize, Serialize)]
pub struct NetPaddleData {
    pub pos: Vec2
}

#[derive(Deserialize, Serialize)]
pub struct NetBrickData {
    pub pos: Vec2
}

#[derive(Deserialize, Serialize)]
pub struct NetBallData {
    pub pos: Vec2
}

#[derive(Deserialize, Serialize)]
pub struct NetScoreData {
    pub score: u32
}

#[derive(Deserialize, Serialize)]
pub enum NetEntityType {
    Paddle(NetPaddleData),
    Brick(NetBrickData),
    Ball(NetBallData),
    Score(NetScoreData)
}

#[derive(Component, Deserialize, Serialize, Clone, Copy, Hash, PartialEq, Eq)]
pub struct NetId(pub u16);

#[derive(Deserialize, Serialize)]
pub struct NetEntity {
    pub entity_type: NetEntityType,
    pub net_id: NetId,
}

#[derive(Deserialize, Serialize, Default)]
pub struct WorldStateData {
    pub frame: u32,
    pub entities: Vec<NetEntity>,
    //pub entities_removed: Vec<NetId> don't do this, losing removes on lost (or eaten) packets is bad
}

#[derive(Deserialize, Serialize)]
pub enum ServerToClientPacket {
    WorldState(WorldStateData),
    //ScoreUpdate(u32)
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum Collision {
    Left,
    Right,
    Top,
    Bottom,
}


// Returns `Some` if `ball` collides with `bounding_box`.
// The returned `Collision` is the side of `bounding_box` that `ball` hit.
fn ball_collision(ball: BoundingCircle, bounding_box: Aabb2d) -> Option<Collision> {
    if !ball.intersects(&bounding_box) {
        return None;
    }

    let closest = bounding_box.closest_point(ball.center());
    let offset = ball.center() - closest;
    let side = if offset.x.abs() > offset.y.abs() {
        if offset.x < 0. {
            Collision::Left
        } else {
            Collision::Right
        }
    } else if offset.y > 0. {
        Collision::Top
    } else {
        Collision::Bottom
    };

    Some(side)
}

#[derive(Resource, Default)]
pub struct FixedTickWorldResource {
    pub frame_counter: u32,
    //pub net_ids_removed_this_frame: Vec<NetId>
}

// This should probably be done via events but quick and dirty for now
pub fn despawn_net_id_entity(commands: &mut Commands, entity: Entity, net_id: NetId, world_resource: &mut FixedTickWorldResource) {
    //world_resource.net_ids_removed_this_frame.push(net_id);
    commands.entity(entity).despawn();
}

pub fn maybe_despawn_net_id_entity(commands: &mut Commands, entity: Entity, net_id: Option<&NetId>, world_resource: &mut FixedTickWorldResource) {
    /*if net_id.is_some() {
        world_resource.net_ids_removed_this_frame.push(*net_id.unwrap());
    }*/
    commands.entity(entity).despawn();
}

pub fn check_for_collisions(
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut ball_query: Query<(&mut Velocity, &Transform), With<Ball>>,
    collider_query: Query<(Entity, &Transform, Option<&NetId>, Option<&Brick>), With<Collider>>,
    mut collision_events: EventWriter<CollisionEvent>,
    mut world_resource: ResMut<FixedTickWorldResource>
) {
    for (mut ball_velocity, ball_transform) in ball_query.iter_mut() {
        for (collider_entity, collider_transform, collider_net_id, maybe_brick) in &collider_query {
            let collision = ball_collision(
                BoundingCircle::new(ball_transform.translation.truncate(), BALL_DIAMETER / 2.),
                Aabb2d::new(
                    collider_transform.translation.truncate(),
                    collider_transform.scale.truncate() / 2.,
                ),
            );

            if let Some(collision) = collision {
                // Sends a collision event so that other systems can react to the collision
                collision_events.send_default();

                // Bricks should be despawned and increment the scoreboard on collision
                if maybe_brick.is_some() {
                    maybe_despawn_net_id_entity(&mut commands, collider_entity, collider_net_id, &mut world_resource);
                    score.0 += 1;
                }

                // Reflect the ball's velocity when it collides
                let mut reflect_x = false;
                let mut reflect_y = false;

                // Reflect only if the velocity is in the opposite direction of the collision
                // This prevents the ball from getting stuck inside the bar
                match collision {
                    Collision::Left => reflect_x = ball_velocity.x > 0.0,
                    Collision::Right => reflect_x = ball_velocity.x < 0.0,
                    Collision::Top => reflect_y = ball_velocity.y < 0.0,
                    Collision::Bottom => reflect_y = ball_velocity.y > 0.0,
                }

                // Reflect velocity on the x-axis if we hit something on the x-axis
                if reflect_x {
                    ball_velocity.x = -ball_velocity.x;
                }

                // Reflect velocity on the y-axis if we hit something on the y-axis
                if reflect_y {
                    ball_velocity.y = -ball_velocity.y;
                }
            }
        }
    }
}

pub fn update_scoreboard(score: Res<Score>, mut query: Query<&mut Text, With<ScoreboardUi>>) {
    let mut text = query.single_mut();
    text.sections[1].value = score.0.to_string();
}

#[derive(Bundle)]
pub struct PaddleBundle {
    sprite_bundle: SpriteBundle,
    paddle: Paddle,
    collider: Collider,
    net_id: NetId,
}

impl PaddleBundle {
    pub fn new(translation: Vec2, net_id: NetId) -> Self {
        PaddleBundle {
            sprite_bundle: SpriteBundle {
                transform: Transform {
                    translation: Vec3::from((translation, 0.0)),
                    scale: PADDLE_SIZE.extend(1.0),
                    ..default()
                },
                sprite: Sprite {
                    color: PADDLE_COLOR,
                    ..default()
                },
                ..default()
            },
            paddle: Paddle,
            collider: Collider,
            net_id
        }
    }
}

/*
pub fn spawn_paddle(
    mut commands: Commands,
    translation: Vec2,
    net_id: NetId
) -> EntityCommands {
    commands.spawn((
        SpriteBundle {
            transform: Transform {
                translation: Vec3::from((translation, 0.0)),
                scale: PADDLE_SIZE.extend(1.0),
                ..default()
            },
            sprite: Sprite {
                color: PADDLE_COLOR,
                ..default()
            },
            ..default()
        },
        Paddle,
        Collider,
        net_id
    ))
}*/

#[derive(Bundle)]
pub struct BallBundle {
    mesh_bundle: MaterialMesh2dBundle<ColorMaterial>,
    ball: Ball,
    velocity: Velocity,
    net_id: NetId
}

impl BallBundle {
    pub fn new(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<ColorMaterial>,
        translation: Vec2,
        net_id: NetId) -> Self {
       BallBundle {
           mesh_bundle: MaterialMesh2dBundle {
               mesh: meshes.add(Circle::default()).into(),
               material: materials.add(BALL_COLOR),
               transform: Transform::from_translation(Vec3::from((translation, 1.0)))
                   .with_scale(Vec2::splat(BALL_DIAMETER).extend(1.)),
               ..default()
           },
           ball: Ball,
           velocity: Velocity(INITIAL_BALL_DIRECTION.normalize() * BALL_SPEED),
           net_id
       }
    }
}

/*pub fn spawn_ball(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    translation: Vec2,
    net_id: NetId
) -> EntityCommands {
    commands.spawn((
        MaterialMesh2dBundle {
            mesh: meshes.add(Circle::default()).into(),
            material: materials.add(BALL_COLOR),
            transform: Transform::from_translation(Vec3::from((translation, 1.0)))
                .with_scale(Vec2::splat(BALL_DIAMETER).extend(1.)),
            ..default()
        },
        Ball,
        Velocity(INITIAL_BALL_DIRECTION.normalize() * BALL_SPEED),
        net_id
    ))
}*/

#[derive(Bundle)]
pub struct BrickBundle {
    sprite_bundle: SpriteBundle,
    brick: Brick,
    collider: Collider,
    net_id: NetId
}

impl BrickBundle {
    pub fn new(brick_position: Vec2, net_id: NetId) -> Self {
        BrickBundle {
            sprite_bundle: SpriteBundle {
                sprite: Sprite {
                    color: BRICK_COLOR,
                    ..default()
                },
                transform: Transform {
                    translation: brick_position.extend(0.0),
                    scale: Vec3::new(BRICK_SIZE.x, BRICK_SIZE.y, 1.0),
                    ..default()
                },
                ..default()
            },
            brick: Brick,
            collider: Collider,
            net_id
        }
    }
}

/*pub fn spawn_brick(commands: &mut Commands, brick_position: Vec2, net_id: NetId) -> EntityCommands {
    // brick
    commands.spawn((
        SpriteBundle {
            sprite: Sprite {
                color: BRICK_COLOR,
                ..default()
            },
            transform: Transform {
                translation: brick_position.extend(0.0),
                scale: Vec3::new(BRICK_SIZE.x, BRICK_SIZE.y, 1.0),
                ..default()
            },
            ..default()
        },
        Brick,
        Collider,
        net_id
    ))
}*/

#[derive(Bundle)]
pub struct ScoreboardUiBundle {
    scoreboard_ui: ScoreboardUi,
    text_bundle: TextBundle,
}

impl ScoreboardUiBundle {
    pub fn new() -> Self {
        let text_bundle = TextBundle::from_sections([
            TextSection::new(
                "Score: ",
                TextStyle {
                    font_size: SCOREBOARD_FONT_SIZE,
                    color: TEXT_COLOR,
                    ..default()
                },
            ),
            TextSection::from_style(TextStyle {
                font_size: SCOREBOARD_FONT_SIZE,
                color: SCORE_COLOR,
                ..default()
            }),
        ])
        .with_style(Style {
            position_type: PositionType::Absolute,
            top: SCOREBOARD_TEXT_PADDING,
            left: SCOREBOARD_TEXT_PADDING,
            ..default()
        });

        ScoreboardUiBundle {
            scoreboard_ui: ScoreboardUi,
            text_bundle
        }
    }
}

/*pub fn spawn_scoreboard(commands: &mut Commands) {
    commands.spawn((
        ScoreboardUi,
        TextBundle::from_sections([
            TextSection::new(
                "Score: ",
                TextStyle {
                    font_size: SCOREBOARD_FONT_SIZE,
                    color: TEXT_COLOR,
                    ..default()
                },
            ),
            TextSection::from_style(TextStyle {
                font_size: SCOREBOARD_FONT_SIZE,
                color: SCORE_COLOR,
                ..default()
            }),
        ])
            .with_style(Style {
                position_type: PositionType::Absolute,
                top: SCOREBOARD_TEXT_PADDING,
                left: SCOREBOARD_TEXT_PADDING,
                ..default()
            }),
    ));
}*/