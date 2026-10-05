//! The lion's body language as a pure function of time: time in, [`Pose`]
//! out. Nothing here touches a window or a clock, so all of it is tested
//! without GPUI.
//!
//! The lion is restrained and a little menacing: it never bounces, never
//! smiles, never moves for the sake of moving. Most of the time it is at
//! rest. Now and then it does one small thing, then rests again:
//!
//! | Gesture | What it does | Timing |
//! | --- | --- | --- |
//! | [`Gesture::Blink`] | the slits close to a hairline and reopen, quick shut, slower open | 70 ms, 30 ms, 120 ms |
//! | [`Gesture::DoubleBlink`] | two blinks, 90 ms apart | 0.7 s |
//! | [`Gesture::Narrow`] | the glare: the brow drops, the lower lid rises, held | 220 ms in, 650 ms held, 420 ms out |
//! | [`Gesture::Glance`] | both eye cuts shift to one side and the head leans 1.2 degrees with them | 180 ms, 700 ms held, 360 ms |
//! | [`Gesture::Breathe`] | the plate swells by 1.2 % and settles | 1.3 s in, 1.7 s out |
//! | [`Gesture::Twitch`] | the nose chevron lifts a little | 90 ms, 80 ms held, 260 ms |
//!
//! A [`Mood`] biases what is played: `Idle` blinks every four to nine
//! seconds and sometimes glares, glances, breathes or twitches; `Working`
//! keeps the eyes narrowed and scans left and right in turn; `Waiting`
//! looks straight ahead, blinks slowly and narrows now and then as a prompt;
//! `Error` makes one sharp narrow and holds it; `Asleep` is nearly closed
//! and does not move at all.
//!
//! On launch the mark can play an intro: two slits open in the dark, then
//! the plate resolves around them ([`INTRO_LEN`]).
//!
//! # Cost
//!
//! [`Timeline::frame`] answers with a [`Wake`]: [`Wake::NextFrame`] only
//! while a part is moving, [`Wake::At`] with the start of the next gesture
//! (or the end of a hold) while nothing moves, [`Wake::Never`] when nothing
//! will move again. A caller that follows it asks for frames only for the
//! tweens and sets one timer between gestures.
//!
//! The schedule is deterministic for a seed: time is cut into one-minute
//! epochs, each scheduled on its own from the seed, so any instant is found
//! in constant time however long the window has been open.

use std::time::Duration;

/// How long the intro takes, in seconds: two slits open, the plate resolves.
pub const INTRO_LEN: f32 = 0.78;

/// How long the eyes take to settle into the baseline of a new [`Mood`].
pub const MOOD_EASE: f32 = 0.32;

/// The length of one scheduling epoch, in seconds.
const EPOCH: f64 = 60.;

/// Where every moving part of the lion is at one instant. [`Pose::REST`] is
/// the mark exactly as the brand draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// How open the eye slits are: 1 open, 0 closed to a hairline.
    pub eye_open: f32,
    /// The glare, 0 to 1: the brow plane drops and the lower lid rises.
    pub narrow: f32,
    /// How far both eye cuts have shifted to the right, in grid units.
    pub glance: f32,
    /// How far the whole head leans clockwise, in degrees.
    pub lean: f32,
    /// Scale of the whole head about its centre; 1 at rest.
    pub scale: f32,
    /// The nose chevron's lift, 0 to 1.
    pub nose: f32,
    /// How much of the plate has arrived, 0 to 1. Only the intro moves it.
    pub plate: f32,
    /// How brightly the eyes shine as solid slits, 0 to 1. Only the intro
    /// moves it.
    pub glow: f32,
}

impl Pose {
    /// The lion as drawn: eyes open, nothing shifted, the plate whole.
    pub const REST: Pose = Pose {
        eye_open: 1.,
        narrow: 0.,
        glance: 0.,
        lean: 0.,
        scale: 1.,
        nose: 0.,
        plate: 1.,
        glow: 0.,
    };

    /// The pose between `from` and `to`; exact at both ends.
    pub fn lerp(from: Pose, to: Pose, amount: f32) -> Pose {
        if amount <= 0. {
            return from;
        }
        if amount >= 1. {
            return to;
        }
        let mix = |a: f32, b: f32| a + (b - a) * amount;
        Pose {
            eye_open: mix(from.eye_open, to.eye_open),
            narrow: mix(from.narrow, to.narrow),
            glance: mix(from.glance, to.glance),
            lean: mix(from.lean, to.lean),
            scale: mix(from.scale, to.scale),
            nose: mix(from.nose, to.nose),
            plate: mix(from.plate, to.plate),
            glow: mix(from.glow, to.glow),
        }
    }

    /// This pose with `other` laid over it: lids multiply, the glare and the
    /// nose take the stronger, shifts add.
    pub fn over(self, other: Pose) -> Pose {
        Pose {
            eye_open: self.eye_open * other.eye_open,
            narrow: self.narrow.max(other.narrow),
            glance: self.glance + other.glance,
            lean: self.lean + other.lean,
            scale: self.scale * other.scale,
            nose: self.nose.max(other.nose),
            plate: self.plate * other.plate,
            glow: self.glow.max(other.glow),
        }
    }

    /// What is visible at 20 pixels and below: the lids and the glare. A
    /// glance, a lean, a breath or a twitch would only be noise there.
    pub fn for_small(self) -> Pose {
        Pose {
            glance: 0.,
            lean: 0.,
            scale: 1.,
            nose: 0.,
            ..self
        }
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self::REST
    }
}

/// How the lion is feeling, which biases what it does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mood {
    /// Nothing is going on: blinks, now and then a glare, a glance, a breath
    /// or a twitch of the nose.
    #[default]
    Idle,
    /// Work is going on: the eyes stay narrowed and scan left and right in
    /// turn.
    Working,
    /// Something wants the user: looks straight ahead, blinks slowly and
    /// narrows now and then as a prompt.
    Waiting,
    /// Something failed: one sharp narrow, held.
    Error,
    /// Nobody is looking: the eyes are nearly closed and nothing moves.
    Asleep,
}

impl Mood {
    /// The pose the lion holds in this mood between gestures.
    pub fn baseline(self) -> Pose {
        match self {
            Mood::Working => Pose {
                narrow: 0.6,
                ..Pose::REST
            },
            Mood::Asleep => Pose {
                eye_open: 0.12,
                ..Pose::REST
            },
            Mood::Idle | Mood::Waiting | Mood::Error => Pose::REST,
        }
    }

    /// Whether the lion does anything in this mood besides holding its
    /// baseline.
    pub fn animates(self) -> bool {
        self != Mood::Asleep
    }
}

/// Which way a glance goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// Towards the left edge.
    Left,
    /// Towards the right edge.
    Right,
}

impl Side {
    fn sign(self) -> f32 {
        match self {
            Side::Left => -1.,
            Side::Right => 1.,
        }
    }

    fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// How a tween eases. All three are monotone and exact at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    /// Starts slow and speeds up: a lid falling.
    In,
    /// Starts fast and settles: a lid rising.
    Out,
    /// Slow at both ends.
    InOut,
}

impl Ease {
    /// The eased amount of `progress`, clamped to 0 to 1.
    pub fn apply(self, progress: f32) -> f32 {
        let t = progress.clamp(0., 1.);
        match self {
            Ease::In => t * t * t,
            Ease::Out => {
                let r = 1. - t;
                1. - r * r * r
            }
            Ease::InOut => t * t * (3. - 2. * t),
        }
    }
}

/// One stretch of a gesture: go to `to` over `dur` seconds. A stretch that
/// ends where it began is a hold.
struct Seg {
    dur: f32,
    to: f32,
    ease: Ease,
}

const fn seg(dur: f32, to: f32, ease: Ease) -> Seg {
    Seg { dur, to, ease }
}

const BLINK: [Seg; 3] = [
    seg(0.07, 1., Ease::In),
    seg(0.03, 1., Ease::In),
    seg(0.12, 0., Ease::Out),
];
const DOUBLE_BLINK: [Seg; 7] = [
    seg(0.07, 1., Ease::In),
    seg(0.03, 1., Ease::In),
    seg(0.12, 0., Ease::Out),
    seg(0.09, 0., Ease::In),
    seg(0.07, 1., Ease::In),
    seg(0.03, 1., Ease::In),
    seg(0.12, 0., Ease::Out),
];
const SLOW_BLINK: [Seg; 3] = [
    seg(0.11, 1., Ease::In),
    seg(0.06, 1., Ease::In),
    seg(0.20, 0., Ease::Out),
];
const NARROW: [Seg; 3] = [
    seg(0.22, 1., Ease::InOut),
    seg(0.65, 1., Ease::InOut),
    seg(0.42, 0., Ease::InOut),
];
const BECKON: [Seg; 3] = [
    seg(0.26, 1., Ease::InOut),
    seg(0.90, 1., Ease::InOut),
    seg(0.50, 0., Ease::InOut),
];
const GLARE: [Seg; 3] = [
    seg(0.11, 1., Ease::Out),
    seg(1.60, 1., Ease::Out),
    seg(0.60, 0., Ease::InOut),
];
const GLANCE: [Seg; 3] = [
    seg(0.18, 1., Ease::Out),
    seg(0.70, 1., Ease::Out),
    seg(0.36, 0., Ease::InOut),
];
const SCAN: [Seg; 3] = [
    seg(0.35, 1., Ease::InOut),
    seg(1.40, 1., Ease::InOut),
    seg(0.45, 0., Ease::InOut),
];
const BREATHE: [Seg; 2] = [seg(1.3, 1., Ease::InOut), seg(1.7, 0., Ease::InOut)];
const TWITCH: [Seg; 3] = [
    seg(0.09, 1., Ease::Out),
    seg(0.08, 1., Ease::Out),
    seg(0.26, 0., Ease::InOut),
];

/// How far the eye cuts shift in a [`Gesture::Glance`], in grid units.
pub const GLANCE_SHIFT: f32 = 1.7;
/// How far the head leans in a [`Gesture::Glance`], in degrees.
pub const GLANCE_LEAN: f32 = 1.2;
/// How far the eye cuts shift in a [`Gesture::Scan`], in grid units.
pub const SCAN_SHIFT: f32 = 1.5;
/// How far the head leans in a [`Gesture::Scan`], in degrees.
pub const SCAN_LEAN: f32 = 0.8;
/// How much the plate swells in a [`Gesture::Breathe`]: 1.2 %.
pub const BREATH: f32 = 0.012;
/// How tight a [`Gesture::Beckon`] gets, of a full narrow.
pub const BECKON_NARROW: f32 = 0.85;

/// What the lion's gesture is doing at an instant.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// A part is moving.
    Moving,
    /// Nothing moves until this many seconds into the gesture.
    Holding(f32),
    /// The gesture is over.
    Over,
}

/// One small thing the lion does and then rests from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gesture {
    /// The slits close to a hairline and reopen.
    Blink,
    /// Two blinks in a row.
    DoubleBlink,
    /// A slower blink, for a lion that is waiting.
    SlowBlink,
    /// The glare: the eyes tighten, hold, relax.
    Narrow,
    /// A glare held a little longer, as a prompt for attention.
    Beckon,
    /// A sharp glare held, for an error.
    Glare,
    /// Both eye cuts shift to one side and the head leans with them.
    Glance(Side),
    /// A slow glance of a working lion, held longer.
    Scan(Side),
    /// The plate swells a little and settles.
    Breathe,
    /// The nose chevron lifts a little.
    Twitch,
}

impl Gesture {
    /// Every gesture, with both sides of the two that have one.
    pub fn catalogue() -> Vec<Gesture> {
        vec![
            Gesture::Blink,
            Gesture::DoubleBlink,
            Gesture::SlowBlink,
            Gesture::Narrow,
            Gesture::Beckon,
            Gesture::Glare,
            Gesture::Glance(Side::Left),
            Gesture::Glance(Side::Right),
            Gesture::Scan(Side::Left),
            Gesture::Scan(Side::Right),
            Gesture::Breathe,
            Gesture::Twitch,
        ]
    }

    fn segs(self) -> &'static [Seg] {
        match self {
            Gesture::Blink => &BLINK,
            Gesture::DoubleBlink => &DOUBLE_BLINK,
            Gesture::SlowBlink => &SLOW_BLINK,
            Gesture::Narrow => &NARROW,
            Gesture::Beckon => &BECKON,
            Gesture::Glare => &GLARE,
            Gesture::Glance(_) => &GLANCE,
            Gesture::Scan(_) => &SCAN,
            Gesture::Breathe => &BREATHE,
            Gesture::Twitch => &TWITCH,
        }
    }

    /// Whether two gestures are of one kind, whatever their side or length:
    /// all the blinks, or both sides of a glance.
    fn same_family(self, other: Gesture) -> bool {
        use Gesture::*;
        matches!(
            (self, other),
            (
                Blink | DoubleBlink | SlowBlink,
                Blink | DoubleBlink | SlowBlink
            ) | (Narrow, Narrow)
                | (Beckon, Beckon)
                | (Glare, Glare)
                | (Glance(_), Glance(_))
                | (Scan(_), Scan(_))
                | (Breathe, Breathe)
                | (Twitch, Twitch)
        )
    }

    /// How long it lasts, in seconds.
    pub fn duration(self) -> f32 {
        self.segs().iter().map(|seg| seg.dur).sum()
    }

    /// Where it is `at` seconds in: the amount of its one curve, 0 to 1,
    /// and whether anything moves.
    fn sample(self, at: f32) -> (f32, Phase) {
        let mut from = 0.;
        let mut start = 0.;
        for seg in self.segs() {
            let end = start + seg.dur;
            if at < end {
                if at < start {
                    return (0., Phase::Over);
                }
                let value = if from == seg.to {
                    from
                } else {
                    from + (seg.to - from) * seg.ease.apply((at - start) / seg.dur)
                };
                let phase = if from == seg.to {
                    Phase::Holding(end)
                } else {
                    Phase::Moving
                };
                return (value, phase);
            }
            from = seg.to;
            start = end;
        }
        (0., Phase::Over)
    }

    /// The pose of the gesture `at` seconds in, laid over [`Pose::REST`].
    pub fn pose(self, at: f32) -> Pose {
        let (v, _) = self.sample(at);
        self.pose_of(v)
    }

    fn pose_of(self, v: f32) -> Pose {
        let rest = Pose::REST;
        match self {
            Gesture::Blink | Gesture::DoubleBlink | Gesture::SlowBlink => Pose {
                eye_open: 1. - v,
                ..rest
            },
            Gesture::Narrow | Gesture::Glare => Pose { narrow: v, ..rest },
            Gesture::Beckon => Pose {
                narrow: BECKON_NARROW * v,
                ..rest
            },
            Gesture::Glance(side) => Pose {
                glance: side.sign() * GLANCE_SHIFT * v,
                lean: side.sign() * GLANCE_LEAN * v,
                ..rest
            },
            Gesture::Scan(side) => Pose {
                glance: side.sign() * SCAN_SHIFT * v,
                lean: side.sign() * SCAN_LEAN * v,
                ..rest
            },
            Gesture::Breathe => Pose {
                scale: 1. + BREATH * v,
                ..rest
            },
            Gesture::Twitch => Pose { nose: v, ..rest },
        }
    }
}

/// Which families of gesture the lion may play. A mark of 20 pixels or less
/// shows only the lids and the glare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureSet {
    /// Blinks, single, double and slow.
    pub blink: bool,
    /// Glares: narrow, beckon, glare.
    pub narrow: bool,
    /// Glances and scans.
    pub glance: bool,
    /// Breaths.
    pub breathe: bool,
    /// Nose twitches.
    pub twitch: bool,
}

impl GestureSet {
    /// Everything.
    pub const ALL: GestureSet = GestureSet {
        blink: true,
        narrow: true,
        glance: true,
        breathe: true,
        twitch: true,
    };

    /// What shows at 20 pixels and below.
    pub const SMALL: GestureSet = GestureSet {
        blink: true,
        narrow: true,
        glance: false,
        breathe: false,
        twitch: false,
    };

    fn allows(&self, gesture: Gesture) -> bool {
        match gesture {
            Gesture::Blink | Gesture::DoubleBlink | Gesture::SlowBlink => self.blink,
            Gesture::Narrow | Gesture::Beckon | Gesture::Glare => self.narrow,
            Gesture::Glance(_) | Gesture::Scan(_) => self.glance,
            Gesture::Breathe => self.breathe,
            Gesture::Twitch => self.twitch,
        }
    }
}

impl Default for GestureSet {
    fn default() -> Self {
        Self::ALL
    }
}

/// When the pose changes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// A part is moving now: draw the next frame.
    NextFrame,
    /// Nothing moves until this time on the timeline: set one timer.
    At(Duration),
    /// Nothing will move again.
    Never,
}

impl Wake {
    fn sooner(self, other: Wake) -> Wake {
        match (self, other) {
            (Wake::NextFrame, _) | (_, Wake::NextFrame) => Wake::NextFrame,
            (Wake::At(a), Wake::At(b)) => Wake::At(a.min(b)),
            (Wake::At(a), Wake::Never) | (Wake::Never, Wake::At(a)) => Wake::At(a),
            (Wake::Never, Wake::Never) => Wake::Never,
        }
    }

    fn at(seconds: f64) -> Wake {
        Wake::At(Duration::from_secs_f64(seconds.max(0.)))
    }
}

/// A pose and when to compute the next one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// What to draw.
    pub pose: Pose,
    /// When what to draw changes.
    pub wake: Wake,
}

impl Frame {
    /// The static mark, with nothing scheduled.
    pub const STILL: Frame = Frame {
        pose: Pose::REST,
        wake: Wake::Never,
    };
}

/// What the lion does over time, from the moment it appears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    seed: u64,
    mood: Mood,
    /// When the current mood began, in seconds on the timeline.
    since: f64,
    /// The baseline the lion was holding when the mood changed.
    from: Option<Pose>,
    intro: bool,
    gestures: GestureSet,
    still: bool,
}

impl Timeline {
    /// A lion that is at rest from the first instant and then idles, its
    /// gestures scheduled from `seed`.
    pub fn idle(seed: u64) -> Self {
        Self {
            seed,
            mood: Mood::Idle,
            since: 0.,
            from: None,
            intro: false,
            gestures: GestureSet::ALL,
            still: false,
        }
    }

    /// A lion that plays the intro first, then idles.
    pub fn intro(seed: u64) -> Self {
        Self {
            intro: true,
            ..Self::idle(seed)
        }
    }

    /// A lion that never moves: the rest pose, nothing scheduled.
    pub fn still() -> Self {
        Self {
            still: true,
            ..Self::idle(0)
        }
    }

    /// The same timeline in another mood from its start.
    pub fn with_mood(mut self, mood: Mood) -> Self {
        self.mood = mood;
        self
    }

    /// The same timeline allowed only some families of gesture.
    pub fn with_gestures(mut self, gestures: GestureSet) -> Self {
        self.gestures = gestures;
        self
    }

    /// The mood it is in.
    pub fn mood(&self) -> Mood {
        self.mood
    }

    /// The same lion, from `at` on in another mood: the eyes ease from where
    /// they are to the new baseline and the gestures of the new mood start
    /// from then.
    pub fn mood_changed(&self, mood: Mood, at: Duration) -> Self {
        if mood == self.mood {
            return *self;
        }
        Self {
            mood,
            since: at.as_secs_f64(),
            from: Some(self.baseline(at.as_secs_f64())),
            ..*self
        }
    }

    /// The baseline held at `t` seconds, easing from the previous mood's.
    fn baseline(&self, t: f64) -> Pose {
        let target = self.mood.baseline();
        match self.from {
            Some(from) => {
                let progress = ((t - self.since) / f64::from(MOOD_EASE)) as f32;
                Pose::lerp(from, target, Ease::InOut.apply(progress))
            }
            None => target,
        }
    }

    /// The gestures of one epoch, as `(start in seconds since the mood
    /// began, gesture)`, in order. None crosses the end of the epoch.
    fn epoch(&self, epoch: u64) -> Vec<(f64, Gesture)> {
        let Some(plan) = plan(self.mood) else {
            return Vec::new();
        };
        let start = epoch as f64 * EPOCH;
        let end = start + EPOCH;
        let mut rng = Rng::new(
            self.seed
                ^ (self.mood as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ epoch.wrapping_mul(0xd1b5_4a32_d192_ed03),
        );
        let mut t = start;
        let mut out = Vec::new();
        let mut last_blink = start - plan.blink_max + rng.range(0.5, 3.);
        let last_beckon = start - rng.range(0., 4.);
        let mut side = Side::Left;

        if epoch == 0 {
            if self.intro {
                t += (f64::from(INTRO_LEN) - self.since).max(0.);
            }
            if self.mood == Mood::Error && self.gestures.allows(Gesture::Glare) {
                out.push((0., Gesture::Glare));
                t = f64::from(Gesture::Glare.duration());
                last_blink = 0.;
            }
        }

        // What is due again within a time: the blink, and where it prompts,
        // the beckon. Each is placed so that its deadline is kept.
        let mut dues = vec![Due {
            kind: plan.blink,
            max: plan.blink_max,
            last: last_blink,
        }];
        if let Some(max) = plan.beckon_max {
            dues.push(Due {
                kind: Kind::Beckon,
                max,
                last: last_beckon,
            });
        }

        let make = |kind: Kind, rng: &mut Rng, side: &mut Side| match kind {
            Kind::Blink if rng.next_f64() < 0.12 => Gesture::DoubleBlink,
            Kind::Blink => Gesture::Blink,
            Kind::SlowBlink => Gesture::SlowBlink,
            Kind::Narrow => Gesture::Narrow,
            Kind::Beckon => Gesture::Beckon,
            Kind::Glance => {
                let pick = if rng.next_f64() < 0.5 {
                    Side::Left
                } else {
                    Side::Right
                };
                Gesture::Glance(pick)
            }
            Kind::Scan => {
                let now = *side;
                *side = side.other();
                Gesture::Scan(now)
            }
            Kind::Breathe => Gesture::Breathe,
            Kind::Twitch => Gesture::Twitch,
        };

        loop {
            let begin = t + rng.range(plan.gap.0, plan.gap.1);
            let mut kind = plan.pick(&mut rng, &self.gestures);
            // Anything that would leave a deadline out of reach gives way to
            // the one that is due soonest.
            let after = begin + f64::from(kind.gesture().duration());
            let reach = after + plan.gap.1;
            if let Some(due) = dues
                .iter()
                .filter(|due| {
                    // Out of reach of its deadline, or out of room before the
                    // end of the epoch, where it is owed in the last seconds.
                    let room = after + plan.gap.0 + f64::from(due.kind.gesture().duration());
                    due.kind != kind
                        && (reach > due.last + due.max
                            || (due.last < end - due.tail(plan.gap.1) && room > end))
                })
                .min_by(|a, b| (a.last + a.max).total_cmp(&(b.last + b.max)))
            {
                kind = due.kind;
            }
            let gesture = make(kind, &mut rng, &mut side);
            let duration = f64::from(gesture.duration());
            if begin + duration > end {
                break;
            }
            t = begin + duration;
            if !self.gestures.allows(gesture) {
                // A family that is switched off: the lion rests instead.
                continue;
            }
            for due in &mut dues {
                if due.kind.gesture().same_family(gesture) {
                    due.last = begin;
                }
            }
            out.push((begin, gesture));
        }

        // The end of the epoch is not a reason for a long wait: what is due
        // is placed in its last seconds.
        for due in &dues {
            let tail = end - due.tail(plan.gap.1);
            if due.last >= tail {
                continue;
            }
            let gesture = due.kind.gesture();
            let begin = (t + plan.gap.0).max(tail);
            let duration = f64::from(gesture.duration());
            if begin + duration <= end && self.gestures.allows(gesture) {
                out.push((begin, gesture));
                t = begin + duration;
            }
        }

        // A working lion scans left then right: an unmatched last scan goes,
        // so that the next epoch starts on the left again.
        let scans = out
            .iter()
            .filter(|(_, g)| matches!(g, Gesture::Scan(_)))
            .count();
        if scans % 2 == 1 {
            if let Some(last) = out.iter().rposition(|(_, g)| matches!(g, Gesture::Scan(_))) {
                out.remove(last);
            }
        }
        out
    }

    /// Every gesture the lion plays from its start to `until`, with the time
    /// each begins, in order.
    pub fn gestures(&self, until: Duration) -> Vec<(Duration, Gesture)> {
        let until = until.as_secs_f64();
        let mut all = Vec::new();
        if !self.mood.animates() || self.still {
            return all;
        }
        let local_end = until - self.since;
        let mut epoch = 0;
        while (epoch as f64) * EPOCH <= local_end {
            for (start, gesture) in self.epoch(epoch) {
                let at = self.since + start;
                if at <= until {
                    all.push((Duration::from_secs_f64(at), gesture));
                }
            }
            epoch += 1;
        }
        all
    }

    /// The gesture running at `local` seconds into the mood, and the start
    /// of the one after it.
    fn locate(&self, local: f64) -> (Option<(f64, Gesture)>, Option<f64>) {
        let first = (local / EPOCH).floor().max(0.) as u64;
        for epoch in first..first + 4 {
            for (start, gesture) in self.epoch(epoch) {
                if local < start {
                    return (None, Some(start));
                }
                if local < start + f64::from(gesture.duration()) {
                    // The next one may be in this epoch or the next.
                    let after = self
                        .epoch(epoch)
                        .into_iter()
                        .map(|(s, _)| s)
                        .find(|s| *s > start)
                        .or_else(|| self.epoch(epoch + 1).first().map(|(s, _)| *s));
                    return (Some((start, gesture)), after);
                }
            }
        }
        (None, None)
    }

    /// The pose at `at`, and when it changes next.
    ///
    /// `poke` is the time on this timeline at which the pointer last arrived
    /// on the mark: it plays a [`Gesture::Narrow`] over whatever else is
    /// going on.
    pub fn frame(&self, at: Duration, poke: Option<Duration>) -> Frame {
        if self.still {
            return Frame::STILL;
        }
        let t = at.as_secs_f64();
        let local = (t - self.since).max(0.);
        let mut wake = Wake::Never;

        let mut pose = self.baseline(t);
        if self.from.is_some() && local < f64::from(MOOD_EASE) {
            wake = Wake::NextFrame;
        }

        if self.mood.animates() {
            let (running, next) = self.locate(local);
            if let Some((start, gesture)) = running {
                let (v, phase) = gesture.sample((local - start) as f32);
                pose = pose.over(gesture.pose_of(v));
                wake = wake.sooner(match phase {
                    Phase::Moving => Wake::NextFrame,
                    Phase::Holding(until) => Wake::at(self.since + start + f64::from(until)),
                    Phase::Over => Wake::Never,
                });
            }
            if let Some(next) = next {
                wake = wake.sooner(Wake::at(self.since + next));
            }
        }

        if let Some(poke) = poke.map(|p| p.as_secs_f64()).filter(|p| t >= *p) {
            let (v, phase) = Gesture::Narrow.sample((t - poke) as f32);
            if phase != Phase::Over {
                pose = pose.over(Gesture::Narrow.pose_of(v));
                wake = wake.sooner(match phase {
                    Phase::Moving => Wake::NextFrame,
                    Phase::Holding(until) => Wake::at(poke + f64::from(until)),
                    Phase::Over => Wake::Never,
                });
            }
        }

        if self.intro && t < f64::from(INTRO_LEN) {
            let (open, plate, glow) = intro_at(t as f32);
            pose.eye_open *= open;
            pose.plate = plate;
            pose.glow = glow;
            wake = Wake::NextFrame;
        }

        Frame { pose, wake }
    }

    /// The pose at `at`.
    pub fn pose(&self, at: Duration) -> Pose {
        self.frame(at, None).pose
    }
}

/// The intro at `t` seconds: how open the eyes are, how much of the plate
/// has arrived and how brightly the slits glow. Two slits open in the dark,
/// the plate resolves around them, the glow gives way to the cuts.
fn intro_at(t: f32) -> (f32, f32, f32) {
    let open = Ease::Out.apply(t / 0.22);
    let plate = Ease::Out.apply((t - 0.26) / 0.52);
    let glow = 1. - Ease::InOut.apply((t - 0.30) / 0.32);
    (open, plate, glow)
}

/// What the plan of a mood may pick.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Blink,
    SlowBlink,
    Narrow,
    Beckon,
    Glance,
    Scan,
    Breathe,
    Twitch,
}

impl Kind {
    fn gesture(self) -> Gesture {
        match self {
            Kind::Blink => Gesture::Blink,
            Kind::SlowBlink => Gesture::SlowBlink,
            Kind::Narrow => Gesture::Narrow,
            Kind::Beckon => Gesture::Beckon,
            Kind::Glance => Gesture::Glance(Side::Left),
            Kind::Scan => Gesture::Scan(Side::Left),
            Kind::Breathe => Gesture::Breathe,
            Kind::Twitch => Gesture::Twitch,
        }
    }
}

/// A gesture the lion owes within a time of the last one like it.
struct Due {
    kind: Kind,
    max: f64,
    last: f64,
}

impl Due {
    /// How long before the end of an epoch the last one of these must have
    /// been, so that the wait across the boundary stays within bounds.
    fn tail(&self, longest_gap: f64) -> f64 {
        self.max - longest_gap - 2.
    }
}

/// How a mood schedules its gestures.
struct Plan {
    /// The rest between two gestures, in seconds: least and most.
    gap: (f64, f64),
    /// The longest the lion goes without a blink.
    blink_max: f64,
    /// The blink it makes when one is due.
    blink: Kind,
    /// The longest it goes without a prompt, where it prompts.
    beckon_max: Option<f64>,
    /// What it picks from, with weights.
    table: &'static [(Kind, u32)],
}

impl Plan {
    fn pick(&self, rng: &mut Rng, allowed: &GestureSet) -> Kind {
        let total: u32 = self
            .table
            .iter()
            .filter(|(kind, _)| allowed.allows(kind.gesture()))
            .map(|(_, weight)| weight)
            .sum();
        if total == 0 {
            return self.blink;
        }
        let mut roll = (rng.next_f64() * f64::from(total)) as u32;
        for (kind, weight) in self.table {
            if !allowed.allows(kind.gesture()) {
                continue;
            }
            if roll < *weight {
                return *kind;
            }
            roll -= weight;
        }
        self.blink
    }
}

fn plan(mood: Mood) -> Option<&'static Plan> {
    const IDLE: Plan = Plan {
        gap: (1.6, 4.2),
        blink_max: 11.,
        blink: Kind::Blink,
        beckon_max: None,
        table: &[
            (Kind::Blink, 52),
            (Kind::Glance, 22),
            (Kind::Narrow, 10),
            (Kind::Breathe, 6),
            (Kind::Twitch, 3),
        ],
    };
    const WORKING: Plan = Plan {
        gap: (0.8, 2.4),
        blink_max: 12.,
        blink: Kind::SlowBlink,
        beckon_max: None,
        table: &[(Kind::Scan, 70), (Kind::SlowBlink, 30)],
    };
    const WAITING: Plan = Plan {
        gap: (2.0, 4.0),
        blink_max: 14.,
        blink: Kind::SlowBlink,
        beckon_max: Some(10.5),
        table: &[(Kind::SlowBlink, 45), (Kind::Beckon, 55)],
    };
    const ERROR: Plan = Plan {
        gap: (3., 6.),
        blink_max: 16.,
        blink: Kind::SlowBlink,
        beckon_max: None,
        table: &[(Kind::SlowBlink, 100)],
    };
    match mood {
        Mood::Idle => Some(&IDLE),
        Mood::Working => Some(&WORKING),
        Mood::Waiting => Some(&WAITING),
        Mood::Error => Some(&ERROR),
        Mood::Asleep => None,
    }
}

/// SplitMix64: a small deterministic generator, so a seed always gives the
/// same lion.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_f64()
    }
}
