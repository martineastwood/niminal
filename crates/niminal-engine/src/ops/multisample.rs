use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::sample::{PORTS, Sampler};
use crate::opcode::{Opcode, Port, ProcessCtx, block_size};

/// The most regions one note plays at once; the rest of a crowded layer stack is dropped.
const MAX_LAYERS: usize = 16;

/// The port that carries the note's velocity.
const VEL_PORT: usize = 12;

/// How long a region that another one chokes takes to fade, in seconds.
const CUT_TIME: f32 = 0.006;

/// How long a release lasts at least, so cutting a note never clicks.
const MIN_RELEASE: f32 = 0.003;

/// Counts the notes that choose among round-robin regions, so each takes the next one in turn.
///
/// Every channel of a note asks with the same note id and must hear the same answer, so the
/// counter remembers which note it last advanced for.
#[derive(Debug, Default)]
pub struct RoundRobin(AtomicU64);

impl RoundRobin {
    /// How many notes have counted so far.
    fn count(&self) -> u32 {
        self.0.load(Ordering::Relaxed) as u32
    }

    /// The number of notes before this one, counting each note once however often it asks.
    fn before(&self, note: u32) -> u32 {
        let mut now = self.0.load(Ordering::Relaxed);
        loop {
            if (now >> 32) as u32 == note && now != 0 {
                return (now as u32).wrapping_sub(1);
            }
            let next = (u64::from(note) << 32) | u64::from((now as u32).wrapping_add(1));
            match self
                .0
                .compare_exchange_weak(now, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return now as u32,
                Err(seen) => now = seen,
            }
        }
    }
}

/// An amplitude envelope in seconds: delay, attack, hold, decay to `sustain`, then release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmpEnv {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    /// 0 to 1.
    pub sustain: f32,
    pub release: f32,
}

impl Default for AmpEnv {
    fn default() -> Self {
        AmpEnv {
            delay: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Done,
}

/// An [`AmpEnv`] running, with its times in frames.
#[derive(Debug, Clone, Copy)]
struct EnvRun {
    stage: Stage,
    /// Frames spent in the stage.
    at: f32,
    level: f32,
    /// The level the release started from.
    from: f32,
    delay: f32,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl EnvRun {
    fn new(env: AmpEnv, sample_rate: f32) -> Self {
        let frames = |seconds: f32| seconds.max(0.0) * sample_rate;
        EnvRun {
            stage: Stage::Delay,
            at: 0.0,
            level: 0.0,
            from: 0.0,
            delay: frames(env.delay),
            attack: frames(env.attack),
            hold: frames(env.hold),
            decay: frames(env.decay),
            sustain: env.sustain.clamp(0.0, 1.0),
            release: frames(env.release.max(MIN_RELEASE)),
        }
    }

    fn done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Fade out quickly, whatever the stage and the note's release.
    fn cut(&mut self, sample_rate: f32) {
        let frames = CUT_TIME * sample_rate;
        match self.stage {
            Stage::Done => {}
            Stage::Release => self.release = self.release.min(self.at + frames),
            _ => {
                self.stage = Stage::Release;
                self.from = self.level;
                self.at = 0.0;
                self.release = frames;
            }
        }
    }

    /// The level for the next frame.
    fn next(&mut self, held: bool) -> f32 {
        if !held && !matches!(self.stage, Stage::Release | Stage::Done) {
            self.stage = Stage::Release;
            self.at = 0.0;
            self.from = self.level;
        }
        loop {
            match self.stage {
                Stage::Delay if self.at >= self.delay => self.enter(Stage::Attack),
                Stage::Attack if self.at >= self.attack => {
                    self.level = 1.0;
                    self.enter(Stage::Hold);
                }
                Stage::Hold if self.at >= self.hold => self.enter(Stage::Decay),
                Stage::Decay if self.at >= self.decay => {
                    self.level = self.sustain;
                    self.enter(Stage::Sustain);
                }
                Stage::Release if self.at >= self.release => {
                    self.level = 0.0;
                    self.enter(Stage::Done);
                }
                _ => break,
            }
        }
        match self.stage {
            Stage::Delay | Stage::Done => self.level = 0.0,
            Stage::Attack => self.level = self.at / self.attack,
            Stage::Hold => self.level = 1.0,
            Stage::Decay => self.level = 1.0 + (self.sustain - 1.0) * self.at / self.decay,
            Stage::Sustain => self.level = self.sustain,
            Stage::Release => self.level = self.from * (1.0 - self.at / self.release),
        }
        self.at += 1.0;
        self.level
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.at = 0.0;
    }
}

/// One region of a multisample: when it plays, how it is tuned and shaped, and the player
/// for one channel of its audio.
#[derive(Clone)]
pub struct Region {
    /// The keys it answers, as MIDI note numbers, both ends included.
    pub keys: (u8, u8),
    /// The velocities it answers, 1 to 127, both ends included.
    pub vels: (u8, u8),
    /// It plays when a random number from 0 up to (not including) 1 falls in this range.
    pub rand: (f32, f32),
    /// Round robin: the count of regions taking turns, this one's turn (from 0), and the shared counter.
    pub seq: Option<(u32, u32, Arc<RoundRobin>)>,
    /// A change of pitch in semitones.
    pub tune: f32,
    /// How much the pitch follows the key: 1 is a semitone a key, 0 is the same pitch on every key.
    pub keytrack: f32,
    /// A level factor.
    pub gain: f32,
    pub env: AmpEnv,
    /// The note's end doesn't stop it.
    pub one_shot: bool,
    /// Its choke group, as an index into the multisample's groups: playing it cuts the regions
    /// whose `off_by` is this group.
    pub group: Option<usize>,
    /// The choke group whose notes cut this region.
    pub off_by: Option<usize>,
    pub sampler: Sampler,
}

/// A region playing: its player, its envelope and the level it applies.
#[derive(Clone)]
struct Playing {
    sampler: Sampler,
    env: EnvRun,
    tune: f32,
    keytrack: f32,
    root_hz: f32,
    gain: f32,
    one_shot: bool,
    off_by: Option<usize>,
    /// How many notes had counted in the `off_by` group when this one started.
    seen: u32,
}

/// An SFZ-style multisample for one channel: at note start it chooses every region that answers
/// the note's key and velocity (its `freq` and `vel`) and plays them together, each with its own
/// tuning, level and envelope. Every channel of a stereo sample makes the same choice.
#[derive(Clone)]
pub struct MultiSampler {
    regions: Arc<[Region]>,
    /// One counter per choke group, shared by every voice of the instrument.
    groups: Arc<[RoundRobin]>,
    playing: Vec<Playing>,
    started: bool,
    sample_rate: f32,
}

impl MultiSampler {
    pub fn new(regions: Vec<Region>, groups: Arc<[RoundRobin]>) -> Self {
        let layers = regions.len().min(MAX_LAYERS);
        MultiSampler {
            regions: regions.into(),
            groups,
            playing: Vec::with_capacity(layers),
            started: false,
            sample_rate: 48_000.0,
        }
    }

    fn start(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]]) {
        self.started = true;
        let key = (69.0 + 12.0 * (f64::from(inputs[0][0].max(1.0)) / 440.0).log2())
            .round()
            .clamp(0.0, 127.0) as u8;
        let vel = (inputs[VEL_PORT][0] * 127.0).round().clamp(1.0, 127.0) as u8;
        let note = mix(ctx.note_token ^ u64::from(ctx.index).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let chance = (note >> 40) as f32 / (1_u64 << 24) as f32;
        let note_id = (note >> 8) as u32 | 1;
        for region in self.regions.iter() {
            if self.playing.len() == MAX_LAYERS {
                break;
            }
            if !(region.keys.0..=region.keys.1).contains(&key)
                || !(region.vels.0..=region.vels.1).contains(&vel)
                || !(region.rand.0 <= chance && chance < region.rand.1)
            {
                continue;
            }
            if let Some((count, turn, counter)) = &region.seq
                && counter.before(note_id) % count != *turn
            {
                continue;
            }
            let mut sampler = region.sampler.clone();
            sampler.prepare(self.sample_rate);
            self.playing.push(Playing {
                root_hz: sampler.root_hz(),
                sampler,
                env: EnvRun::new(region.env, self.sample_rate),
                tune: region.tune,
                keytrack: region.keytrack,
                gain: region.gain,
                one_shot: region.one_shot,
                off_by: region.off_by,
                seen: 0,
            });
            // the note counts once in each group of the regions it plays, however many channels ask
            if let Some(group) = region.group {
                self.groups[group].before(note_id);
            }
        }
        // only notes after this one cut what it starts
        for voice in &mut self.playing {
            voice.seen = voice.off_by.map_or(0, |group| self.groups[group].count());
        }
    }
}

/// SplitMix64's finisher: spreads a note's identity over all of its bits.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl Opcode for MultiSampler {
    fn name(&self) -> &str {
        "sample"
    }
    fn ports(&self) -> &'static [Port] {
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(MultiSampler {
            playing: Vec::with_capacity(self.regions.len().min(MAX_LAYERS)),
            started: false,
            ..self.clone()
        })
    }
    fn reset(&mut self) {
        self.playing.clear();
        self.started = false;
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if !self.started {
            self.start(ctx, inputs);
        }
        out.fill(0.0);
        let n = out.len();
        debug_assert!(n <= block_size());
        let mut freq = [0.0_f32; crate::opcode::MAX_BLOCK];
        let mut pitch = [0.0_f32; crate::opcode::MAX_BLOCK];
        let mut heard = [0.0_f32; crate::opcode::MAX_BLOCK];
        for voice in &mut self.playing {
            if let Some(group) = voice.off_by
                && self.groups[group].count() != voice.seen
            {
                voice.seen = self.groups[group].count();
                voice.env.cut(self.sample_rate);
            }
            if voice.env.done() || !voice.sampler.is_active() {
                voice.env.stage = Stage::Done;
                continue;
            }
            let mut local: [&[f32]; PORTS.len()] = [&[]; PORTS.len()];
            local.copy_from_slice(&inputs[..PORTS.len()]);
            // tuning and key tracking become a change of the pitch the player is asked for
            if voice.tune != 0.0 || voice.keytrack != 1.0 {
                for i in 0..n {
                    pitch[i] = inputs[1][i] + voice.tune;
                    freq[i] = if voice.keytrack == 1.0 {
                        inputs[0][i]
                    } else {
                        voice.root_hz * (inputs[0][i] / voice.root_hz).powf(voice.keytrack)
                    };
                }
                local[0] = &freq[..n];
                local[1] = &pitch[..n];
            }
            voice.sampler.process(ctx, &local, &mut heard[..n]);
            let held = ctx.gate || voice.one_shot;
            for (y, x) in out.iter_mut().zip(&heard[..n]) {
                *y += x * voice.gain * voice.env.next(held);
            }
        }
        // players that ran dry or whose release is over drop out of the chord
        self.playing
            .retain(|v| !v.env.done() && v.sampler.is_active());
    }
    fn is_active(&self) -> bool {
        !self.started || !self.playing.is_empty()
    }
    fn is_envelope(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Table;
    use crate::ops::LoopMode;

    const RATE: f32 = 48_000.0;

    fn constant(value: f32, frames: usize) -> Arc<Table> {
        Arc::new(Table::from_f32(vec![value; frames], Some(RATE)))
    }

    fn no_groups() -> Arc<[RoundRobin]> {
        Arc::from([])
    }

    fn region(keys: (u8, u8), vels: (u8, u8), value: f32) -> Region {
        Region {
            keys,
            vels,
            rand: (0.0, 1.0),
            seq: None,
            tune: 0.0,
            keytrack: 1.0,
            gain: 1.0,
            env: AmpEnv::default(),
            one_shot: false,
            group: None,
            off_by: None,
            sampler: Sampler::new(constant(value, 4800), 261.625_56, None),
        }
    }

    /// Play one note of `hz` at `gain`, held for `held` blocks then released, for `blocks` blocks.
    fn play(
        multi: &mut MultiSampler,
        hz: f32,
        gain: f32,
        held: usize,
        blocks: usize,
    ) -> Vec<Vec<f32>> {
        multi.prepare(RATE);
        let (freq, gain, zero, one) = ([hz; 64], [gain; 64], [0.0; 64], [1.0; 64]);
        (0..blocks)
            .map(|b| {
                let mut ctx = ProcessCtx::new(RATE, b < held);
                ctx.note_token = 7;
                let mut out = vec![0.0; 64];
                multi.process(
                    &ctx,
                    &[
                        &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &zero, &one, &zero,
                        &zero, &gain,
                    ],
                    &mut out,
                );
                out
            })
            .collect()
    }

    fn level(blocks: &[Vec<f32>], block: usize) -> f32 {
        blocks[block].iter().fold(0.0, |m: f32, v| m.max(v.abs()))
    }

    #[test]
    fn a_note_plays_the_regions_for_its_key_and_velocity() {
        let regions = || {
            vec![
                region((0, 59), (1, 127), 0.1),
                region((60, 127), (1, 63), 0.2),
                region((60, 127), (64, 127), 0.4),
            ]
        };
        // c4 is key 60; a velocity of 1 is 127 and 0.2 is 25
        let loud = play(
            &mut MultiSampler::new(regions(), no_groups()),
            261.625_56,
            1.0,
            10,
            2,
        );
        assert!((level(&loud, 1) - 0.4).abs() < 1e-4);
        let soft = play(
            &mut MultiSampler::new(regions(), no_groups()),
            261.625_56,
            0.2,
            10,
            2,
        );
        assert!((level(&soft, 1) - 0.2).abs() < 1e-4);
        let low = play(
            &mut MultiSampler::new(regions(), no_groups()),
            130.81,
            1.0,
            10,
            2,
        );
        assert!((level(&low, 1) - 0.1).abs() < 1e-4);
        let none = play(
            &mut MultiSampler::new(vec![region((72, 80), (1, 127), 0.5)], no_groups()),
            261.6,
            1.0,
            10,
            2,
        );
        assert_eq!(level(&none, 1), 0.0);
    }

    #[test]
    fn regions_that_overlap_play_together() {
        let mut multi = MultiSampler::new(
            vec![
                region((0, 127), (1, 127), 0.25),
                region((0, 127), (1, 127), 0.5),
            ],
            no_groups(),
        );
        let blocks = play(&mut multi, 261.625_56, 1.0, 10, 2);
        assert!((level(&blocks, 1) - 0.75).abs() < 1e-4);
    }

    #[test]
    fn tuning_follows_the_region_and_its_key_tracking() {
        // a ramp shows the speed: an octave up reads twice as fast
        let ramp: Arc<Table> = Arc::new(Table::from_f32(
            (0..4800).map(|i| i as f32 / 4800.0).collect::<Vec<_>>(),
            Some(RATE),
        ));
        let sampler = || Sampler::new(ramp.clone(), 261.625_56, None);
        let mut up = region((0, 127), (1, 127), 0.0);
        up.sampler = sampler();
        up.tune = 12.0;
        let a = play(
            &mut MultiSampler::new(vec![up], no_groups()),
            261.625_56,
            1.0,
            10,
            1,
        );
        let mut flat = region((0, 127), (1, 127), 0.0);
        flat.sampler = sampler();
        flat.keytrack = 0.0;
        // an octave above the root, but not tracking the key: plays as recorded
        let b = play(
            &mut MultiSampler::new(vec![flat], no_groups()),
            523.25,
            1.0,
            10,
            1,
        );
        let step = |blocks: &[Vec<f32>]| (blocks[0][63] - blocks[0][0]) * 4800.0 / 63.0;
        assert!((step(&a) - 2.0).abs() < 0.05, "{}", step(&a));
        assert!((step(&b) - 1.0).abs() < 0.05, "{}", step(&b));
    }

    #[test]
    fn a_released_note_fades_over_its_release_and_then_ends() {
        let mut long = region((0, 127), (1, 127), 1.0);
        long.sampler = Sampler::new(constant(1.0, 96_000), 261.625_56, None);
        long.env.release = 0.02;
        let mut multi = MultiSampler::new(vec![long], no_groups());
        let blocks = play(&mut multi, 261.625_56, 1.0, 5, 30);
        assert!((level(&blocks, 4) - 1.0).abs() < 1e-4);
        // 20ms is 960 frames, 15 blocks
        assert!(blocks[5][63] < blocks[5][0] && level(&blocks, 10) < level(&blocks, 6));
        assert_eq!(level(&blocks, 25), 0.0);
        assert!(!multi.is_active());
    }

    #[test]
    fn a_one_shot_ignores_the_end_of_the_note() {
        let mut hit = region((0, 127), (1, 127), 1.0);
        hit.one_shot = true;
        let mut multi = MultiSampler::new(vec![hit], no_groups());
        let blocks = play(&mut multi, 261.625_56, 1.0, 0, 10);
        assert!((level(&blocks, 9) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_loop_holds_until_the_note_ends() {
        let mut held = region((0, 127), (1, 127), 0.5);
        held.sampler = Sampler::new(constant(0.5, 480), 261.625_56, None).with_loop_frames(
            LoopMode::Forward,
            100,
            400,
        );
        let mut multi = MultiSampler::new(vec![held], no_groups());
        let blocks = play(&mut multi, 261.625_56, 1.0, 50, 60);
        assert!((level(&blocks, 49) - 0.5).abs() < 1e-4);
        assert_eq!(level(&blocks, 59), 0.0);
    }

    #[test]
    fn a_note_in_a_group_cuts_the_regions_that_name_it() {
        // an open hat (group 0) is cut by the closed hat (group 1) and its own group
        let groups: Arc<[RoundRobin]> = Arc::from([RoundRobin::default(), RoundRobin::default()]);
        let mut open = region((60, 60), (1, 127), 0.5);
        open.sampler = Sampler::new(constant(0.5, 96_000), 261.625_56, None);
        open.group = Some(0);
        open.off_by = Some(1);
        let mut closed = region((62, 62), (1, 127), 0.25);
        closed.sampler = Sampler::new(constant(0.25, 4800), 293.664_77, None);
        closed.group = Some(1);
        let template = MultiSampler::new(vec![open, closed], groups);
        let (zero, one) = ([0.0; 64], [1.0; 64]);
        let block = |voice: &mut MultiSampler, hz: f32, token: u64| {
            let freq = [hz; 64];
            let mut ctx = ProcessCtx::new(RATE, true);
            ctx.note_token = token;
            let mut out = vec![0.0; 64];
            voice.process(
                &ctx,
                &[
                    &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &zero, &one, &zero,
                    &zero, &one,
                ],
                &mut out,
            );
            out[63]
        };
        let mut hat = template.clone();
        hat.prepare(RATE);
        assert!((block(&mut hat, 261.625_56, 1) - 0.5).abs() < 1e-4);
        // the closed hat is a different key and note: it cuts the open hat within a few ms
        let mut closed = template.clone();
        closed.prepare(RATE);
        block(&mut closed, 293.664_77, 2);
        let tail: Vec<f32> = (0..8).map(|_| block(&mut hat, 261.625_56, 1)).collect();
        assert_eq!(tail[7], 0.0);
        assert!(!hat.is_active());
        // the closed hat itself is untouched by its own note
        assert!((block(&mut closed, 293.664_77, 2) - 0.25).abs() < 1e-4);
    }

    #[test]
    fn round_robin_regions_take_turns_by_key() {
        let counter = Arc::new(RoundRobin::default());
        let regions = || {
            [0u32, 1, 2]
                .map(|turn| {
                    let mut r = region((60, 60), (1, 127), 0.1 * (turn + 1) as f32);
                    r.seq = Some((3, turn, counter.clone()));
                    r
                })
                .to_vec()
        };
        let template = MultiSampler::new(regions(), no_groups());
        let heard: Vec<f32> = (0..6)
            .map(|n| {
                let (freq, zero, one) = ([261.625_56; 64], [0.0; 64], [1.0; 64]);
                let mut ctx = ProcessCtx::new(RATE, true);
                ctx.note_token = 100 + n;
                // two channels of one note ask with the same note id and must agree
                let channels: Vec<f32> = (0..2)
                    .map(|_| {
                        let mut voice = template.clone();
                        voice.prepare(RATE);
                        let mut out = vec![0.0; 64];
                        voice.process(
                            &ctx,
                            &[
                                &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &zero, &one,
                                &zero, &zero, &one,
                            ],
                            &mut out,
                        );
                        out[63]
                    })
                    .collect();
                assert_eq!(channels[0], channels[1]);
                channels[0]
            })
            .collect();
        let steps: Vec<i32> = heard.iter().map(|v| (v * 10.0).round() as i32).collect();
        assert_eq!(steps, [1, 2, 3, 1, 2, 3]);
    }
}
