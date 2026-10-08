//! The Den as a simulation: who is where, doing what, at any time.
//!
//! A [`Den`] is fed with the list of cubs ([`Den::update`]) and with what
//! happens ([`Den::happen`]), each with the time it is told at. In between
//! it is asked for a [`Frame`] ([`Den::frame`]), which is a pure function of
//! what it was told and of the time asked for: nothing here reads a clock,
//! so every moment of the den can be tested.
//!
//! Time is counted in ticks of [`TICK`] (100 ms, ten a second: the pace of
//! pixel art, not of the display). A lion walks a tile in [`STEP_TICKS`] and
//! every loop of every pose steps on a whole number of ticks.
//!
//! # Where everybody is
//!
//! Every lion has a home: a work seat of its own, given when it joins (the
//! first free one; a little one the free small seat nearest its parent) and
//! kept while that seat exists. With more lions than seats the rest have a
//! tile of bare floor each, apart from the others. A state asks for a place
//! ([`wanted`]) or for the home; a place holds as many as it has spots
//! ([`ENTRANCE_LINE`] at the entrance), and a lion that finds it full stays
//! home and shows there what it does. So no two lions rest on one tile, and
//! nobody is sent where there is no room.
//!
//! # Cost
//!
//! A frame says when the picture changes next ([`Wake`]): the next tick
//! while a cub walks, an egg hatches or the narrator types; the next beat of
//! the slowest pose on screen when everybody is settled (about one redraw a
//! second for a den of sleeping cubs); never, when nothing is left that
//! moves: an empty den, or a den of cubs that fainted. With reduced motion
//! cubs are put straight on their spot in a still pose and the only changes
//! are those of the text box.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::Hsla;

use crate::feed::{self, Feed, Item, Kind, Past};
use crate::layout::DenLayout;
use crate::model::{Cub, CubState, Event, Happening, Status, ToolKind};
use crate::narrator::{self, Line, Narrator, Said, LINGER, TICK};
use crate::pose::{Act, Bubble, Facing, Look, TILE};
use crate::prefabs::default_layout;
use crate::world::{Ground, Place, Spot, Tile, World};

/// How many ticks a cub takes to walk one tile.
pub const STEP_TICKS: u64 = 4;
/// How long an egg wobbles before it cracks open.
pub const EGG_TICKS: u64 = 28;
/// How long the little one takes to climb out of the shell.
pub const HATCH_TICKS: u64 = 8;
/// How long the tick or the cross of a command stays over a cub.
pub const FLASH_TICKS: u64 = 16;
/// How long a cub must have been thinking before the narrator wonders
/// whether it is napping.
pub const PONDER: Duration = Duration::from_secs(40);
/// How many cubs must be at work for the pride to be busy.
pub const BUSY: usize = 3;

/// When the picture changes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// At this time: set one timer.
    At(Duration),
    /// Nothing will move until the den is told something new.
    Never,
}

impl Wake {
    /// The earlier of two.
    pub fn sooner(self, other: Wake) -> Wake {
        match (self, other) {
            (Wake::At(a), Wake::At(b)) => Wake::At(a.min(b)),
            (Wake::At(a), Wake::Never) | (Wake::Never, Wake::At(a)) => Wake::At(a),
            (Wake::Never, Wake::Never) => Wake::Never,
        }
    }
}

/// A cub as it is drawn in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// The cub's id.
    pub id: u64,
    /// Its name.
    pub name: String,
    /// The left edge of its tile, in pixels of the art.
    pub x: i32,
    /// The top edge of its tile.
    pub y: i32,
    /// The tile it is on, or walking onto.
    pub tile: Tile,
    /// How it is drawn.
    pub look: Look,
    /// The colour of its mane.
    pub tint: Hsla,
    /// What makes it an individual.
    pub seed: u64,
    /// Its level.
    pub level: u32,
    /// It is the selected cub.
    pub selected: bool,
    /// The pointer is on it.
    pub hovered: bool,
}

impl Actor {
    /// The box the lion is drawn in, for the pointer: left, top, width,
    /// height, in pixels of the art.
    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        let (left, top, w, h) = crate::pose::bounds(&self.look);
        (self.x + left, self.y + top, w, h)
    }
}

/// The den at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The tick of the moment.
    pub tick: u64,
    /// The cubs, the farthest first: the order they are drawn in.
    pub actors: Vec<Actor>,
    /// What the den says of itself as a whole when nothing was told: the
    /// line of an empty feed.
    pub said: Said,
    /// The entry of the feed the narrator is typing, and how many of its
    /// characters show.
    pub typing: Option<(u64, usize)>,
    /// When to ask for the next frame.
    pub wake: Wake,
}

/// A line of the roster.
#[derive(Clone, Debug, PartialEq)]
pub struct RosterEntry {
    /// The cub's id.
    pub id: u64,
    /// Its name, as the session is called.
    pub name: String,
    /// Its level.
    pub level: u32,
    /// Its state in a few letters.
    pub tag: &'static str,
    /// Its state.
    pub state: CubState,
    /// The status colour of its state, if it has one.
    pub status: Option<Status>,
    /// The colour of its mane.
    pub tint: Hsla,
    /// A little one: listed under the cub that sent it out.
    pub little: bool,
    /// It is the selected cub.
    pub selected: bool,
}

/// The plain truth about one cub: what the joke was about.
#[derive(Clone, Debug, PartialEq)]
pub struct TruthCard {
    /// The cub's id.
    pub id: u64,
    /// Its name, as the session is called.
    pub name: String,
    /// Its level: how many tools it has used.
    pub level: u32,
    /// Its state.
    pub state: CubState,
    /// Its state in plain words.
    pub label: &'static str,
    /// The exact detail the host gave: the file, the command.
    pub detail: Option<String>,
    /// The name of the cub that sent it out, for a little one.
    pub parent: Option<String>,
    /// Nothing but working or quiet is known of this session.
    pub mystery: bool,
    /// The status colour of its state, if it has one.
    pub status: Option<Status>,
}

/// How many lions wait in line at the entrance: the rest wait at their
/// own desk, with the same `!`.
pub const ENTRANCE_LINE: usize = 3;

/// What a lion's own place in the room is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HomeKind {
    /// A work seat: one of the spots of this place.
    Seat(Place),
    /// A tile of bare floor: there were more lions than seats.
    Floor,
}

#[derive(Clone, Debug)]
struct Body {
    cub: Cub,
    place: Place,
    spot: Spot,
    /// Its own place in the room, given when it joins and kept: where it
    /// works, where it sleeps, and where it stays when the place its state
    /// asks for is full.
    home: Option<(Spot, HomeKind)>,
    /// The tiles it walks over, from where it was to its spot.
    path: Vec<Tile>,
    /// The tick it sets off at.
    depart: u64,
    /// The tick it appeared at.
    born: u64,
    /// It began as an egg.
    egg: bool,
    /// When it came to its present state.
    since: Duration,
    /// On its way out.
    leaving: bool,
    flash: Option<(Bubble, u64)>,
}

impl Body {
    fn arrival(&self) -> u64 {
        self.depart + (self.path.len() as u64).saturating_sub(1) * STEP_TICKS
    }

    fn settled(&self, tick: u64) -> bool {
        tick >= self.arrival()
    }

    fn in_egg(&self, tick: u64) -> bool {
        self.egg && tick < self.depart
    }

    /// Where it is: the pixel of its tile's corner, the tile it is on or
    /// stepping onto, and the way it walks if it does.
    fn position(&self, tick: u64) -> (i32, i32, Tile, Option<Facing>) {
        let first = self.path.first().copied().unwrap_or(self.spot.tile);
        if tick < self.depart || self.path.len() < 2 {
            return (first.x * TILE, first.y * TILE, first, None);
        }
        let elapsed = tick - self.depart;
        let index = (elapsed / STEP_TICKS) as usize;
        if index + 1 >= self.path.len() {
            let last = self.path[self.path.len() - 1];
            return (last.x * TILE, last.y * TILE, last, None);
        }
        let (from, to) = (self.path[index], self.path[index + 1]);
        let part = (elapsed % STEP_TICKS) as i32 * TILE / STEP_TICKS as i32;
        (
            from.x * TILE + (to.x - from.x) * part,
            from.y * TILE + (to.y - from.y) * part,
            // It is on a tile until it has left it.
            if part == 0 { from } else { to },
            Some(from.facing_to(to)),
        )
    }

    /// Sends it to a tile from wherever it is at this tick.
    fn route(&mut self, world: &World, to: Tile, tick: u64) {
        if tick < self.depart || self.path.len() < 2 {
            let from = self.path.first().copied().unwrap_or(to);
            self.path = world.path(from, to);
            self.depart = self.depart.max(tick);
            return;
        }
        let elapsed = tick - self.depart;
        let index = (elapsed / STEP_TICKS) as usize;
        if index + 1 >= self.path.len() {
            let from = self.path[self.path.len() - 1];
            self.path = world.path(from, to);
            self.depart = tick;
        } else if elapsed % STEP_TICKS == 0 {
            self.path = world.path(self.path[index], to);
            self.depart = tick;
        } else {
            // In the middle of a step: finish it, then turn.
            let mut path = vec![self.path[index]];
            path.extend(world.path(self.path[index + 1], to));
            self.path = path;
            self.depart += index as u64 * STEP_TICKS;
        }
    }

    /// Puts it on a tile, with no walk.
    fn put(&mut self, tile: Tile, tick: u64) {
        self.path = vec![tile];
        self.depart = self.depart.min(tick);
        self.egg = false;
    }
}

/// The place a state asks for, away from the lion's own desk; `None` is
/// the desk itself. A little one does all its work at its small desk. A
/// place holds as many as it has spots: a lion that finds it full stays at
/// its desk and shows there what it does.
pub fn wanted(state: CubState, little: bool) -> Option<Place> {
    match state {
        CubState::WaitingForUser | CubState::NeedsPermission => Some(Place::Entrance),
        // The lounge is a treat for the idle, while it has a free seat.
        CubState::Idle => Some(Place::Sun),
        _ if little => None,
        CubState::Reading | CubState::Searching => Some(Place::Shelf),
        CubState::Running => Some(Place::Rack),
        CubState::Web => Some(Place::Lookout),
        CubState::Planning => Some(Place::Board),
        CubState::Delegating => Some(Place::Watch),
        CubState::Editing
        | CubState::UsingTool
        | CubState::Thinking
        | CubState::Mystery
        | CubState::Asleep
        | CubState::Fainted
        | CubState::Gone => None,
    }
}

/// Where a lion in a state goes when every place has room: the place of
/// [`wanted`], or its desk.
pub fn place_of(state: CubState, little: bool, _at_desk: bool) -> Place {
    wanted(state, little).unwrap_or(if little { Place::Bench } else { Place::Desks })
}

/// What a lion in a state does once it is at its place. A little one, which
/// has only its small desk, types there whatever tool it uses.
pub fn act_of(state: CubState, little: bool) -> Act {
    match state {
        CubState::Thinking => Act::Think,
        CubState::Mystery => Act::Wonder,
        CubState::WaitingForUser => Act::Wait,
        CubState::NeedsPermission => Act::Stare,
        CubState::Idle => Act::Lounge,
        CubState::Asleep => Act::Sleep,
        CubState::Fainted => Act::Faint,
        CubState::Gone => Act::Stand,
        _ if little => Act::Type,
        CubState::Editing | CubState::UsingTool => Act::Type,
        CubState::Reading => Act::Browse,
        CubState::Searching => Act::Rummage,
        CubState::Running => Act::Operate,
        CubState::Web => Act::Peer,
        CubState::Planning => Act::Plan,
        CubState::Delegating => Act::Guard,
    }
}

fn bubble_of(act: Act) -> Option<Bubble> {
    match act {
        Act::Wait => Some(Bubble::Bang),
        Act::Stare => Some(Bubble::Urgent),
        Act::Think => Some(Bubble::Thought),
        Act::Wonder => Some(Bubble::Question),
        Act::Sleep => Some(Bubble::Zzz),
        Act::Faint => Some(Bubble::Cross),
        _ => None,
    }
}

/// The tick a time falls in.
pub fn tick_of(now: Duration) -> u64 {
    (now.as_millis() / TICK.as_millis()) as u64
}

/// The time a tick starts at.
pub fn time_of(tick: u64) -> Duration {
    Duration::from_millis(tick * TICK.as_millis() as u64)
}

/// The den and everybody in it.
#[derive(Clone, Debug)]
pub struct Den {
    layout: DenLayout,
    world: World,
    bodies: Vec<Body>,
    narrator: Narrator,
    feed: Feed,
    /// What time it is, as the host last said: seconds since the Unix epoch.
    wall: Option<i64>,
    reduced: bool,
    /// The box says the plain state of the den, not the narrator's lines.
    plain: bool,
    selected: Option<u64>,
    hovered: Option<u64>,
    /// How many times each kind of line was said of each cub: what turns
    /// the templates.
    said: HashMap<(u64, u8), u64>,
    /// Since when the den has been empty, or busy.
    mood: Option<(Mood, Duration)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mood {
    Empty,
    Busy,
}

impl Default for Den {
    fn default() -> Self {
        Self::new()
    }
}

impl Den {
    /// An empty den in the default room.
    pub fn new() -> Self {
        Self {
            layout: default_layout(),
            world: World::build(&default_layout()),
            bodies: Vec::new(),
            narrator: Narrator::new(),
            feed: Feed::new(),
            wall: None,
            reduced: false,
            plain: false,
            selected: None,
            hovered: None,
            said: HashMap::new(),
            mood: Some((Mood::Empty, Duration::ZERO)),
        }
    }

    /// The map.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Whether the den holds still.
    pub fn reduced_motion(&self) -> bool {
        self.reduced
    }

    /// With reduced motion a cub is put on its spot in a still pose, an egg
    /// is skipped and a line of the narrator appears whole. What is shown is
    /// the same.
    pub fn set_reduced_motion(&mut self, reduced: bool, now: Duration) {
        if self.reduced == reduced {
            return;
        }
        self.reduced = reduced;
        self.narrator.set_reduced_motion(reduced);
        if reduced {
            let tick = tick_of(now);
            for body in &mut self.bodies {
                let tile = body.spot.tile;
                body.put(tile, tick);
            }
            self.bodies.retain(|body| !body.leaving);
        }
    }

    /// The layout of the room.
    pub fn layout(&self) -> &DenLayout {
        &self.layout
    }

    /// Gives the den another room, or the same one with its furniture
    /// moved. Every lion takes a spot of the new room: with `walk` it walks
    /// there from where it stands (somebody is moving the furniture under
    /// it), without it is simply there.
    pub fn set_layout(&mut self, layout: &DenLayout, walk: bool, now: Duration) {
        if *layout == self.layout {
            return;
        }
        self.layout = layout.clone();
        self.world = World::build(layout);
        let tick = tick_of(now);
        let walk = walk && !self.reduced;
        if !walk {
            self.bodies.retain(|body| !body.leaving);
        }
        self.rehome(true);
        self.replace_all(walk, tick);
    }

    /// Gives a home to whoever has none: a lion the first free work seat,
    /// a little one the free small seat nearest its parent's (or else a
    /// lion's), and, when the seats are all taken, a tile of bare floor
    /// apart from the others. With `moved` (the furniture changed) a home
    /// that is no longer a seat is given up first. A lion that stood on the
    /// floor takes a seat when one is free.
    fn rehome(&mut self, moved: bool) {
        let world = &self.world;
        let seat_of = |place: Place, tile: Tile| {
            world
                .spots(place)
                .iter()
                .find(|spot| spot.tile == tile)
                .copied()
        };
        let mut homes: HashSet<Tile> = HashSet::new();
        // Seats that are still seats are kept, with the way they face now.
        for body in &mut self.bodies {
            if body.leaving || body.cub.state == CubState::Gone {
                body.home = None;
                continue;
            }
            body.home = match body.home {
                Some((spot, HomeKind::Seat(place))) => seat_of(place, spot.tile)
                    .filter(|spot| homes.insert(spot.tile))
                    .map(|spot| (spot, HomeKind::Seat(place))),
                Some((spot, HomeKind::Floor)) if !moved => Some((spot, HomeKind::Floor)),
                _ => None,
            };
        }
        // Then the seats that are free, in the order of the roster.
        let parents: Vec<(u64, Tile)> = self
            .bodies
            .iter()
            .filter_map(|body| Some((body.cub.id, body.home?.0.tile)))
            .collect();
        let mut parents = parents;
        for index in 0..self.bodies.len() {
            let body = &self.bodies[index];
            if body.leaving || body.cub.state == CubState::Gone {
                continue;
            }
            if matches!(body.home, Some((_, HomeKind::Seat(_)))) {
                continue;
            }
            let near = body
                .cub
                .parent
                .and_then(|parent| parents.iter().find(|(id, _)| *id == parent))
                .map(|(_, tile)| *tile);
            let nearest = |place: Place, homes: &HashSet<Tile>| {
                world
                    .spots(place)
                    .iter()
                    .enumerate()
                    .filter(|(_, spot)| !homes.contains(&spot.tile))
                    .min_by_key(|(order, spot)| {
                        (near.map_or(0, |near| spot.tile.distance(near)), *order)
                    })
                    .map(|(_, spot)| (*spot, HomeKind::Seat(place)))
            };
            let seat = if body.cub.parent.is_some() {
                nearest(Place::Bench, &homes).or_else(|| nearest(Place::Desks, &homes))
            } else {
                nearest(Place::Desks, &homes)
            };
            let home = match (seat, body.home) {
                (Some(seat), old) => {
                    if let Some((old, _)) = old {
                        homes.remove(&old.tile);
                    }
                    seat
                }
                (None, Some(floor)) => floor,
                (None, None) => {
                    let near = near.unwrap_or_else(|| world.anchor(Place::Desks));
                    (world.standing(near, &homes), HomeKind::Floor)
                }
            };
            homes.insert(home.0.tile);
            parents.push((self.bodies[index].cub.id, home.0.tile));
            self.bodies[index].home = Some(home);
        }
        // The floor homes of before are checked last: one that a seat or the
        // furniture now covers is given again.
        let mut floor: HashSet<Tile> = self
            .bodies
            .iter()
            .filter_map(|body| match body.home {
                Some((spot, HomeKind::Seat(_))) => Some(spot.tile),
                _ => None,
            })
            .collect();
        for index in 0..self.bodies.len() {
            let Some((spot, HomeKind::Floor)) = self.bodies[index].home else {
                continue;
            };
            let good = self.world.walkable(spot.tile)
                && spot.tile != self.world.door
                && floor.insert(spot.tile);
            if !good {
                let near = self.world.anchor(Place::Desks);
                let again = self.world.standing(near, &floor);
                floor.insert(again.tile);
                self.bodies[index].home = Some((again, HomeKind::Floor));
            }
        }
    }

    /// The tiles of bare floor that are somebody's home: in a room with
    /// little floor one may be the spot of a place, and then it is taken.
    fn floor_homes(&self) -> HashSet<Tile> {
        self.bodies
            .iter()
            .filter(|body| !body.leaving)
            .filter_map(|body| match body.home {
                Some((spot, HomeKind::Floor)) => Some(spot.tile),
                _ => None,
            })
            .collect()
    }

    /// The home of a body as a place and a spot.
    fn home_of(&self, body: &Body) -> (Place, Spot) {
        let little = body.cub.parent.is_some();
        match body.home {
            Some((spot, HomeKind::Seat(place))) => (place, spot),
            Some((spot, HomeKind::Floor)) => {
                (if little { Place::Bench } else { Place::Desks }, spot)
            }
            None => (
                Place::Desks,
                Spot {
                    tile: self.world.anchor(Place::Desks),
                    facing: Facing::Down,
                },
            ),
        }
    }

    /// The free spot of the place a body's state asks for, if it has one.
    fn away_spot(&self, body: &Body, taken: &HashSet<Tile>) -> Option<(Place, Spot)> {
        let place = wanted(body.cub.state, body.cub.parent.is_some())?;
        let spots = self.world.spots(place);
        let most = if place == Place::Entrance {
            ENTRANCE_LINE.min(spots.len())
        } else {
            spots.len()
        };
        spots[..most]
            .iter()
            .find(|spot| !taken.contains(&spot.tile))
            .map(|spot| (place, *spot))
    }

    /// Sends everybody where its state asks, as far as there is room: who
    /// is where it should be stays, who must move takes a free spot of its
    /// place or else its own desk. `known` are the cubs that were here
    /// before this update.
    fn place(&mut self, known: &HashSet<u64>, walk: bool, tick: u64) {
        let mut taken: HashSet<Tile> = self.floor_homes();
        let mut moving = Vec::new();
        for index in 0..self.bodies.len() {
            let fresh = !known.contains(&self.bodies[index].cub.id);
            let (home_place, home) = self.home_of(&self.bodies[index]);
            let body = &mut self.bodies[index];
            match body.cub.state {
                CubState::Gone => {
                    if !body.leaving {
                        body.leaving = true;
                        body.place = Place::Entrance;
                        let outside = self.world.outside();
                        body.spot = Spot {
                            tile: outside,
                            facing: Facing::Down,
                        };
                        if walk {
                            body.route(&self.world, outside, tick);
                        } else {
                            body.put(outside, tick);
                        }
                    }
                }
                CubState::Fainted if !fresh => {
                    // It drops where it is.
                    let (_, _, tile, _) = body.position(tick);
                    if body.spot.tile != tile || !body.settled(tick) {
                        body.spot.tile = tile;
                        body.route(&self.world, tile, tick);
                    }
                    taken.insert(tile);
                }
                state => {
                    let little = body.cub.parent.is_some();
                    let at_home = body.spot.tile == home.tile && body.place == home_place;
                    let stays = !fresh
                        && match wanted(state, little) {
                            // Where it should be already, or at its desk
                            // because that place was full.
                            Some(place) => body.place == place,
                            // Asleep, it stays on the sofa it was idle on.
                            None => {
                                at_home || (state == CubState::Asleep && body.place == Place::Sun)
                            }
                        };
                    if stays {
                        taken.insert(body.spot.tile);
                    } else {
                        moving.push(index);
                    }
                }
            }
        }
        for index in moving {
            let (place, spot) = self
                .away_spot(&self.bodies[index], &taken)
                .unwrap_or_else(|| self.home_of(&self.bodies[index]));
            taken.insert(spot.tile);
            let body = &mut self.bodies[index];
            // A lion at its desk that could now go where its state asks
            // goes; one that is there stays.
            if body.place == place && body.spot.tile == spot.tile && body.settled(tick) {
                body.spot = spot;
                continue;
            }
            body.place = place;
            body.spot = spot;
            if walk {
                body.route(&self.world, spot.tile, tick);
            } else {
                body.put(spot.tile, tick);
            }
        }
    }

    /// Puts everybody in a room that changed: who is at a spot that is
    /// still a spot of its place keeps it, the rest go where their state
    /// asks or to their desk.
    fn replace_all(&mut self, walk: bool, tick: u64) {
        let mut taken: HashSet<Tile> = self.floor_homes();
        for index in 0..self.bodies.len() {
            if self.bodies[index].leaving {
                let outside = self.world.outside();
                let body = &mut self.bodies[index];
                body.spot.tile = outside;
                body.route(&self.world, outside, tick);
                continue;
            }
            let body = &self.bodies[index];
            let (home_place, home) = self.home_of(body);
            let little = body.cub.parent.is_some();
            // Out cold, it stays where it fell if that is still floor.
            let fallen = body.cub.state == CubState::Fainted
                && self.world.ground(body.spot.tile) == Ground::Floor
                && !taken.contains(&body.spot.tile);
            let kept = wanted(body.cub.state, little)
                .filter(|place| *place == body.place)
                .and_then(|place| {
                    self.world
                        .spots(place)
                        .iter()
                        .find(|spot| spot.tile == body.spot.tile && !taken.contains(&spot.tile))
                        .map(|spot| (place, *spot))
                });
            let (place, spot) = if fallen {
                (body.place, body.spot)
            } else {
                kept.or_else(|| self.away_spot(body, &taken))
                    .unwrap_or((home_place, home))
            };
            taken.insert(spot.tile);
            let body = &mut self.bodies[index];
            body.place = place;
            body.spot = spot;
            if walk && !body.in_egg(tick) {
                body.route(&self.world, spot.tile, tick);
            } else {
                body.put(spot.tile, tick);
            }
        }
    }

    /// Tells the den who is in it now. Cubs it has not seen walk in by the
    /// entrance (a little one hatches in the nest), cubs whose state changed
    /// walk to their new place, and cubs that are no longer listed, or are
    /// [`CubState::Gone`], walk out.
    pub fn update(&mut self, cubs: &[Cub], now: Duration) {
        let tick = tick_of(now);
        let reduced = self.reduced;
        self.bodies
            .retain(|body| !(body.leaving && body.settled(tick)));

        // Who is here keeps its order; who is new comes after.
        for body in &mut self.bodies {
            match cubs.iter().find(|cub| cub.id == body.cub.id) {
                Some(cub) => {
                    if cub.state != body.cub.state {
                        body.since = now;
                    }
                    body.cub = cub.clone();
                }
                None => body.cub.state = CubState::Gone,
            }
        }
        let known: HashSet<u64> = self.bodies.iter().map(|body| body.cub.id).collect();
        let mut arrivals = 0;
        let mut eggs: HashSet<Tile> = self
            .bodies
            .iter()
            .filter(|body| body.in_egg(tick))
            .filter_map(|body| body.path.first().copied())
            .collect();
        for cub in cubs {
            if known.contains(&cub.id) || cub.state == CubState::Gone {
                continue;
            }
            let hatches = cub.parent.is_some() && !reduced;
            let (start, depart) = if hatches {
                let nest = self.world.claim(Place::Nest, &eggs).tile;
                eggs.insert(nest);
                (nest, tick + EGG_TICKS + HATCH_TICKS)
            } else {
                arrivals += 1;
                // One after the other through the door, not in a heap.
                (self.world.outside(), tick + (arrivals - 1) * 3)
            };
            self.bodies.push(Body {
                cub: cub.clone(),
                // No place yet: it is given one below, like a cub that moves.
                place: Place::Nest,
                spot: Spot {
                    tile: start,
                    facing: Facing::Down,
                },
                home: None,
                path: vec![start],
                depart,
                born: tick,
                egg: hatches,
                since: now,
                leaving: false,
                flash: None,
            });
        }

        self.rehome(false);
        self.place(&known, !reduced, tick);
        if reduced {
            self.bodies.retain(|body| !body.leaving);
        }

        if self.selected.is_some_and(|id| !self.order().contains(&id)) {
            self.selected = None;
        }
        let mood = self.mood_now();
        if mood != self.mood.map(|(mood, _)| mood) {
            self.mood = mood.map(|mood| (mood, now));
        }
    }

    fn mood_now(&self) -> Option<Mood> {
        let here = self.bodies.iter().filter(|body| !body.leaving);
        if here.clone().count() == 0 {
            Some(Mood::Empty)
        } else if here.filter(|body| body.cub.state.is_working()).count() >= BUSY {
            Some(Mood::Busy)
        } else {
            None
        }
    }

    /// Tells the den what happened: the narrator says it, and a command that
    /// ends leaves a tick or a cross over its cub for a moment.
    pub fn happen(&mut self, happening: &Happening, now: Duration) {
        let tick = tick_of(now);
        let flash = match happening.event {
            Event::ToolFinished { ok: false, .. } => Some(Bubble::Cross),
            Event::ToolFinished {
                kind: ToolKind::Run,
                ok: true,
            } => Some(Bubble::Tick),
            _ => None,
        };
        if let (Some(flash), Some(body)) = (
            flash,
            self.bodies
                .iter_mut()
                .find(|body| body.cub.id == happening.cub),
        ) {
            body.flash = Some((flash, tick));
        }
        let kind = match &happening.event {
            Event::Joined => 0,
            Event::ToolStarted { kind, .. } => 1 + *kind as u8,
            Event::ToolFinished { .. } => 10,
            Event::SentOut { .. } => 11,
            Event::CameBack { .. } => 12,
            Event::PermissionPrompt => 13,
            Event::TurnEnded => 14,
            Event::Mysterious => 15,
            Event::FellAsleep => 16,
            Event::Fainted { .. } => 17,
            Event::WentHome => 18,
        };
        let count = self.said.entry((happening.cub, kind)).or_insert(0);
        let line = narrator::narrate(happening, *count);
        *count += 1;
        let Some(line) = line else {
            return;
        };
        let (owner, tint) = self.owner_of(happening.cub);
        self.feed.push(
            Item {
                cub: happening.cub,
                owner,
                name: happening.name.clone(),
                tint,
                at: self.wall,
                kind: Kind::Narration {
                    urgent: line.urgent,
                },
                text: line.text(),
                plain: narrator::plainly(happening).unwrap_or_default(),
            },
            now,
        );
    }

    /// The lion whose feed a cub's entries belong to (itself, or the one
    /// that sent it out) and the colour of its own mane.
    fn owner_of(&self, cub: u64) -> (u64, Option<gpui_kit::Hsla>) {
        match self.bodies.iter().find(|body| body.cub.id == cub) {
            Some(body) => (body.cub.parent.unwrap_or(cub), Some(body.cub.species.tint)),
            None => (cub, None),
        }
    }

    /// Tells the den what time it is, in seconds since the Unix epoch: the
    /// time of what is told from now on, and what "5m" is counted from.
    pub fn set_wall_time(&mut self, now: i64) {
        self.wall = Some(now);
    }

    /// What time the host last said it is.
    pub fn wall_time(&self) -> Option<i64> {
        self.wall
    }

    /// Puts in the feed what an agent just said to the user, as it said it.
    pub fn speak(&mut self, cub: u64, name: &str, text: &str, at: Option<i64>, now: Duration) {
        if text.trim().is_empty() {
            return;
        }
        let (owner, tint) = self.owner_of(cub);
        self.feed.push(
            Item {
                cub,
                owner,
                name: name.to_owned(),
                tint,
                at: at.or(self.wall),
                kind: Kind::Speech,
                text: text.to_owned(),
                plain: String::new(),
            },
            now,
        );
    }

    /// Puts the past of a session in the feed ([`feed::backfill`]): its
    /// last messages, and a line for the tools of each turn.
    pub fn remember(&mut self, cub: u64, name: &str, past: &[Past]) {
        let (owner, tint) = self.owner_of(cub);
        let items = feed::backfill(cub, owner, name, tint, past, feed::PER_SESSION);
        self.feed.insert_past(items);
    }

    /// The feed.
    pub fn feed(&self) -> &Feed {
        &self.feed
    }

    /// The feed, to scroll it or to open a message.
    pub fn feed_mut(&mut self) -> &mut Feed {
        &mut self.feed
    }

    /// Whether the narrator is off: the feed says its lines plainly.
    pub fn plain_status(&self) -> bool {
        self.plain
    }

    /// With `plain`, the text box drops the narrator and says the state of
    /// the den in plain words: how many sessions are live, working, waiting.
    /// A command that ends still leaves its tick or its cross.
    pub fn set_plain_status(&mut self, plain: bool) {
        self.plain = plain;
    }

    /// The state of the den in plain words, for the box of a den whose
    /// narrator is off: one or two rows.
    pub fn status(&self) -> Line {
        let here: Vec<&Body> = self.bodies.iter().filter(|body| !body.leaving).collect();
        let count = |wanted: fn(CubState) -> bool| {
            here.iter().filter(|body| wanted(body.cub.state)).count()
        };
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        if here.is_empty() {
            return Line {
                rows: vec!["No live sessions.".to_owned()],
                urgent: false,
            };
        }
        let working = count(CubState::is_working);
        let waiting = count(|state| state == CubState::WaitingForUser);
        let asking = count(|state| state == CubState::NeedsPermission);
        let failed = count(|state| state == CubState::Fainted);
        let mut rows = vec![format!(
            "{}: {working} working, {waiting} waiting.",
            plural(here.len(), "live session", "live sessions")
        )];
        let mut second = Vec::new();
        if asking > 0 {
            second.push(plural(asking, "needs permission", "need permission"));
        }
        if failed > 0 {
            second.push(plural(failed, "exited with an error", "exited with errors"));
        }
        if !second.is_empty() {
            rows.push(format!("{}.", second.join(", ")));
        }
        Line {
            rows,
            urgent: asking > 0 || failed > 0,
        }
    }

    /// Says a line that is not about a happening.
    pub fn say(&mut self, line: Line, now: Duration) {
        self.narrator.say(line, now);
    }

    /// What the text box says of the den as a whole, when it has nothing
    /// newer to say: the line and since when it holds.
    fn ambient(&self, now: Duration) -> Option<(Line, Duration)> {
        match self.mood {
            Some((Mood::Empty, since)) => Some((narrator::quiet_line(), since)),
            Some((Mood::Busy, since)) => Some((narrator::busy_line(), since)),
            None => self
                .bodies
                .iter()
                .filter(|body| {
                    !body.leaving
                        && matches!(body.cub.state, CubState::Thinking | CubState::Mystery)
                })
                .filter(|body| now >= body.since + PONDER)
                .min_by_key(|body| body.since)
                .map(|body| {
                    (
                        narrator::pondering_line(&body.cub.name),
                        body.since + PONDER,
                    )
                }),
        }
    }

    fn speech(&self, now: Duration) -> Said {
        if self.plain {
            let line = self.status();
            return Said {
                rows: line.rows,
                urgent: line.urgent,
                complete: true,
                more: false,
                next: None,
                since: None,
            };
        }
        let said = self.narrator.said(now);
        if !said.complete && !said.rows.is_empty() || said.more {
            return said;
        }
        // The last line has been read; after a while the box may speak of
        // the den as a whole.
        let free_at = said.since.map_or(Duration::ZERO, |since| since + LINGER);
        let Some((line, since)) = self.ambient(now) else {
            // A cub that thinks will be wondered about later.
            let ponder = self
                .bodies
                .iter()
                .filter(|body| {
                    !body.leaving
                        && matches!(body.cub.state, CubState::Thinking | CubState::Mystery)
                })
                .map(|body| (body.since + PONDER).max(free_at))
                .min();
            return Said {
                next: said.next.or(ponder),
                ..said
            };
        };
        let start = since.max(free_at);
        if now < start {
            return Said {
                next: Some(start),
                ..said
            };
        }
        if said.rows == line.rows {
            return said;
        }
        let total = line.len();
        let ticks = ((now - start).as_millis() / TICK.as_millis()) as usize + 1;
        let typed = if self.reduced {
            total
        } else {
            (ticks * narrator::TYPED_PER_TICK).min(total)
        };
        let complete = typed >= total;
        Said {
            rows: line.typed(typed),
            urgent: false,
            complete,
            more: false,
            next: (!complete).then(|| start + TICK * ticks as u32),
            since: complete.then_some(start + narrator::typing_time(&line)),
        }
    }

    fn actor(&self, body: &Body, tick: u64) -> Option<Actor> {
        if body.leaving && body.settled(tick) {
            return None;
        }
        let cub = &body.cub;
        let little = cub.parent.is_some();
        let (x, y, tile, walking) = body.position(tick);
        let seat = self.world.ground(body.spot.tile) == Ground::Seat;
        // Cubs do not blink together: each has its own offset.
        let phase = crate::palette::scramble(cub.id) % 16;
        let (act, facing, beat) = if body.in_egg(tick) {
            let hatch_at = body.depart.saturating_sub(HATCH_TICKS);
            if tick >= hatch_at {
                (Act::Hatch, Facing::Down, tick - hatch_at)
            } else {
                // The wobble quickens as the time comes.
                let age = tick.saturating_sub(body.born);
                (Act::Egg, Facing::Down, age / Act::Egg.beat_ticks())
            }
        } else if let Some(facing) = walking {
            (Act::Walk, facing, tick - body.depart)
        } else if tick < body.depart {
            (Act::Stand, Facing::Up, 0)
        } else {
            let act = act_of(cub.state, little);
            // At its desk because the place its state asks for is full, it
            // does that work there: typing, with the tool on its card.
            let at_desk = matches!(body.place, Place::Desks | Place::Bench);
            let act = match act {
                Act::Browse | Act::Rummage | Act::Operate | Act::Peer | Act::Plan | Act::Guard
                    if at_desk =>
                {
                    Act::Type
                }
                act => act,
            };
            let facing = match act {
                // On its seat it looks where the seat looks.
                act if act.seated() && seat => body.spot.facing,
                Act::Peer | Act::Plan | Act::Operate | Act::Rummage | Act::Guard => {
                    body.spot.facing
                }
                _ => Facing::Down,
            };
            let beat = match act.beat_ticks() {
                0 => 0,
                ticks => (tick + phase) / ticks,
            };
            (act, facing, beat)
        };
        let flash = body
            .flash
            .filter(|(_, at)| tick < at + FLASH_TICKS)
            .map(|(bubble, _)| bubble);
        let bubble = match (cub.state, act) {
            (_, Act::Egg | Act::Hatch) => None,
            // What is urgent shows even on the way.
            (CubState::NeedsPermission, _) => Some(Bubble::Urgent),
            (CubState::Fainted, _) => Some(Bubble::Cross),
            _ => flash.or(bubble_of(act)),
        };
        let still = self.reduced;
        Some(Actor {
            id: cub.id,
            name: cub.name.clone(),
            x,
            y,
            tile,
            look: Look {
                act,
                facing,
                little,
                seat: seat && walking.is_none() && tick >= body.depart && act.seated(),
                beat: if still { 0 } else { beat },
                bubble,
            },
            tint: cub.species.tint,
            seed: cub.species.seed,
            level: cub.level,
            selected: self.selected == Some(cub.id),
            hovered: self.hovered == Some(cub.id),
        })
    }

    /// When a body's picture changes next, as a tick.
    fn next_change(&self, body: &Body, tick: u64) -> Option<u64> {
        if body.leaving && body.settled(tick) {
            return None;
        }
        if !body.settled(tick) {
            return Some(tick + 1);
        }
        let flash_end = body
            .flash
            .map(|(_, at)| at + FLASH_TICKS)
            .filter(|end| tick < *end);
        let act = act_of(body.cub.state, body.cub.parent.is_some());
        let beat = match act.beat_ticks() {
            0 => None,
            ticks => {
                let phase = crate::palette::scramble(body.cub.id) % 16;
                Some(((tick + phase) / ticks + 1) * ticks - phase)
            }
        };
        match (flash_end, beat) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The den at a time: who is drawn where and how, what the box says, and
    /// when to ask again.
    pub fn frame(&self, now: Duration) -> Frame {
        let tick = tick_of(now);
        let mut actors: Vec<Actor> = self
            .bodies
            .iter()
            .filter_map(|body| self.actor(body, tick))
            .collect();
        actors.sort_by_key(|actor| (actor.y, actor.x, actor.id));
        let said = self.speech(now);
        let mut wake = said.next.map_or(Wake::Never, Wake::At);
        let typing = if self.reduced || self.plain {
            None
        } else {
            self.feed.typing(now)
        };
        if let Some(next) = self.feed.next_typed(now).filter(|_| typing.is_some()) {
            wake = wake.sooner(Wake::At(next));
        }
        if !self.reduced {
            for body in &self.bodies {
                if let Some(next) = self.next_change(body, tick) {
                    wake = wake.sooner(Wake::At(time_of(next)));
                }
            }
        }
        Frame {
            tick,
            actors,
            said,
            typing,
            wake,
        }
    }

    /// The cubs in the order of the roster, which is the order the keyboard
    /// walks them in: each cub in the order it came, followed by its little
    /// ones.
    pub fn order(&self) -> Vec<u64> {
        let here: Vec<&Body> = self.bodies.iter().filter(|body| !body.leaving).collect();
        let is_here = |id: u64| here.iter().any(|body| body.cub.id == id);
        let mut order = Vec::with_capacity(here.len());
        for body in &here {
            // A little one whose parent is gone is listed on its own.
            if body.cub.parent.is_some_and(is_here) {
                continue;
            }
            order.push(body.cub.id);
            for little in &here {
                if little.cub.parent == Some(body.cub.id) {
                    order.push(little.cub.id);
                }
            }
        }
        order
    }

    /// The roster: the pride as a list.
    pub fn roster(&self) -> Vec<RosterEntry> {
        self.order()
            .into_iter()
            .filter_map(|id| self.bodies.iter().find(|body| body.cub.id == id))
            .map(|body| {
                let cub = &body.cub;
                RosterEntry {
                    id: cub.id,
                    name: cub.name.clone(),
                    level: cub.level,
                    tag: cub.state.tag(),
                    state: cub.state,
                    status: cub.state.status(),
                    tint: cub.species.tint,
                    little: cub
                        .parent
                        .is_some_and(|parent| self.bodies.iter().any(|b| b.cub.id == parent)),
                    selected: self.selected == Some(cub.id),
                }
            })
            .collect()
    }

    /// The plain truth about a cub.
    pub fn truth(&self, id: u64) -> Option<TruthCard> {
        let body = self
            .bodies
            .iter()
            .find(|body| body.cub.id == id && !body.leaving)?;
        let cub = &body.cub;
        Some(TruthCard {
            id,
            name: cub.name.clone(),
            level: cub.level,
            state: cub.state,
            label: cub.state.label(),
            detail: cub
                .detail
                .clone()
                .filter(|detail| !detail.trim().is_empty()),
            parent: cub.parent.and_then(|parent| {
                self.bodies
                    .iter()
                    .find(|body| body.cub.id == parent)
                    .map(|body| body.cub.name.clone())
            }),
            mystery: cub.mystery,
            status: cub.state.status(),
        })
    }

    /// The selected cub.
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    /// Selects a cub, or nobody. A cub that is not in the den is nobody.
    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id.filter(|id| self.order().contains(id));
    }

    /// Moves the selection along the roster by `by` places, around the ends.
    /// With nobody selected it takes the first cub going forward and the
    /// last going back. It answers who is selected now.
    pub fn select_by(&mut self, by: i32) -> Option<u64> {
        let order = self.order();
        if order.is_empty() {
            self.selected = None;
            return None;
        }
        let len = order.len() as i32;
        let at = self
            .selected
            .and_then(|id| order.iter().position(|listed| *listed == id));
        let next = match at {
            Some(at) => (at as i32 + by).rem_euclid(len),
            None if by < 0 => len - 1,
            None => 0,
        };
        self.selected = Some(order[next as usize]);
        self.selected
    }

    /// The cub the pointer is on.
    pub fn hovered(&self) -> Option<u64> {
        self.hovered
    }

    /// Tells the den which cub the pointer is on. It answers whether that
    /// changed.
    pub fn hover(&mut self, id: Option<u64>) -> bool {
        let changed = self.hovered != id;
        self.hovered = id;
        changed
    }

    /// The cub at a point of the map, in pixels of the art: the nearest to
    /// the viewer of those drawn there.
    pub fn cub_at(&self, x: i32, y: i32, now: Duration) -> Option<u64> {
        self.frame(now)
            .actors
            .iter()
            .rev()
            .find(|actor| {
                let (left, top, w, h) = actor.bounds();
                x >= left && x < left + w && y >= top && y < top + h
            })
            .map(|actor| actor.id)
    }
}
