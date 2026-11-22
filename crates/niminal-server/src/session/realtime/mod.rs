//! Prepared event windows into an exclusively owned DSP renderer. Both queues
//! are bounded: one delivers constructed voices and mixers, the other returns
//! their storage for destruction on the planning thread.

use super::*;
use crate::controls::{self, Applied, Change, ControlMessage, State as ControlState};
use crate::sounding::Sounding;
use niminal_engine::Voice;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicU32, AtomicUsize};

#[derive(Clone, Copy, Debug)]
pub struct RealtimeConfig {
    pub lookahead_frames: u64,
    pub window_frames: usize,
    pub queued_windows: usize,
    pub max_voices: usize,
}

/// How the first sample of a prepared window relates to the live mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanMode {
    Advance,
    /// Plan again from now with the mixer swapped for a new one, because definitions changed.
    Refresh,
    /// Plan again from now, keeping the mixer, because something waiting was taken back.
    Replan,
    Reset,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            lookahead_frames: 8192,
            window_frames: 2048,
            queued_windows: 32,
            max_voices: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderMetrics {
    pub late_events: usize,
    pub starved_frames: usize,
    pub capacity_drops: usize,
    pub control_drops: usize,
    pub retire_pressure: usize,
    pub deadline_misses: usize,
    pub planner_panics: usize,
    pub max_render_micros: u64,
    /// Mixers built for the renderer: one for each change of definitions, and for each replan.
    pub mixer_builds: usize,
}

pub(super) struct State {
    clock: AtomicU64,
    revision: AtomicU64,
    /// Changes along with `revision` when the mixer too must be swapped.
    swap_epoch: AtomicU64,
    reset_epoch: AtomicU64,
    pub(super) voices: AtomicUsize,
    peaks: [AtomicU32; MAX_CHANNELS],
    silenced: AtomicUsize,
    late_events: AtomicUsize,
    starved_frames: AtomicUsize,
    capacity_drops: AtomicUsize,
    control_drops: AtomicUsize,
    retire_pressure: AtomicUsize,
    deadline_misses: AtomicUsize,
    planner_panics: AtomicUsize,
    max_render_micros: AtomicU64,
    pub(super) latency: usize,
    mixer_builds: Arc<AtomicUsize>,
    /// The thread that plans ahead, woken when something changes that it must act on at once.
    planner_thread: std::sync::Mutex<Option<std::thread::Thread>>,
}

impl State {
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics {
            late_events: self.late_events.load(Ordering::Relaxed),
            starved_frames: self.starved_frames.load(Ordering::Relaxed),
            capacity_drops: self.capacity_drops.load(Ordering::Relaxed),
            control_drops: self.control_drops.load(Ordering::Relaxed),
            retire_pressure: self.retire_pressure.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            planner_panics: self.planner_panics.load(Ordering::Relaxed),
            max_render_micros: self.max_render_micros.load(Ordering::Relaxed),
            mixer_builds: self.mixer_builds.load(Ordering::Relaxed),
        }
    }
}

struct Window {
    end: u64,
    revision: u64,
    commands: Vec<Command>,
    notices: Vec<String>,
    stopped: Vec<String>,
    dropped: usize,
}

impl Window {
    /// Say something once per window, however many notes it applies to.
    fn notice(&mut self, text: &str) {
        if !self.notices.iter().any(|n| n == text) {
            self.notices.push(text.into());
        }
    }
}

/// The most changes envelope lanes may make to parameters in one prepared window.
const MAX_WINDOW_CONTROLS: usize = 2048;

struct Command {
    at: u64,
    kind: Kind,
}

impl Command {
    /// Commands at one sample go in this order: a reset or swap of the graph before the notes it applies to.
    fn order(&self) -> (u64, u8) {
        (
            self.at,
            match self.kind {
                Kind::Reset { .. } | Kind::ClientPanic { .. } => 0,
                Kind::Swap { .. } | Kind::Control { .. } | Kind::Buffer { .. } => 1,
                Kind::Note { .. } => 2,
                Kind::Release { .. }
                | Kind::SetParam { .. }
                | Kind::Map { .. }
                | Kind::BindVoice { .. }
                | Kind::BindChain { .. }
                | Kind::ChainMap { .. } => 3,
            },
        )
    }
}

// A command lives in a window made ahead of time, so its size costs nothing at play time; boxing the note would
// mean freeing it on the audio thread.
#[allow(clippy::large_enum_variant)]
enum Kind {
    ChainMap {
        node: String,
        param: String,
        mapping: Option<Box<niminal_engine::ParamMapping>>,
    },
    BindVoice {
        key: u32,
        resource: u64,
        banks: Vec<niminal_engine::BufferBank>,
    },
    BindChain {
        node: String,
        resource: u64,
        banks: Vec<niminal_engine::BufferBank>,
    },
    ClientPanic {
        client: u64,
        change: GraphChange,
        transfers: Vec<Transfer>,
    },
    Map {
        key: u32,
        param: String,
        mapping: Option<Box<niminal_engine::ParamMapping>>,
    },
    Buffer {
        buffer: Arc<niminal_engine::Buffer>,
        offset: usize,
        values: Arc<[f32]>,
    },
    Note {
        node: Option<String>,
        program: Arc<Program>,
        off: u64,
        key: u32,
        voice: Option<Voice>,
        midi: Option<niminal_lang::MidiNote>,
    },
    /// Let go of the notes a key is holding open.
    Release {
        key: u32,
    },
    /// Move a parameter, by its index, of the notes a key is holding, over `frames`.
    SetParam {
        curve: niminal_engine::GlideCurve,
        key: u32,
        name: String,
        value: f32,
        frames: u64,
    },
    /// A control set to a value, gliding over some frames.
    Control {
        key: String,
        value: f32,
        frames: u64,
    },
    Swap {
        change: GraphChange,
        /// Filled in when the swap is planned, without allocating on the audio thread.
        transfers: Vec<Transfer>,
    },
    Reset {
        change: GraphChange,
        epoch: u64,
    },
}

/// A mixer and the program it plays, with the controls that belong to it: everything a graph change
/// puts in place at once.
struct BufferRestore {
    node: String,
    key: u64,
    banks: Vec<niminal_engine::BufferBank>,
}
struct ChainRestore {
    node: String,
    param: String,
    mapping: Option<Box<niminal_engine::ParamMapping>>,
}
struct GraphChange {
    mappings: Vec<ChainRestore>,
    buffers: Vec<BufferRestore>,
    stop_players: Vec<String>,
    /// Set when a client is waiting for a receipt for this change.
    applied: Option<Arc<AtomicU64>>,
    mixer: Box<Mixer>,
    program: Arc<Program>,
    controls: Vec<ControlState>,
}

/// The command that moves a parameter of a held note, if `program` has the instrument and the parameter.
fn set_command(program: &Program, set: &super::VoiceSet, at: u64, frames: u64) -> Option<Command> {
    let _ = program;
    Some(Command {
        at,
        kind: Kind::SetParam {
            curve: set.curve,
            key: set.key,
            name: set.name.clone(),
            value: set.value,
            frames,
        },
    })
}

fn chain_map_command(program: &Program, map: &super::ChainMap, at: u64) -> Command {
    Command {
        at,
        kind: Kind::ChainMap {
            node: map.node(),
            param: map.control_key.clone(),
            mapping: super::parameters::prepare_mapping(
                program,
                map.source.as_deref(),
                map.channel,
                map.scale,
                map.offset,
                None,
            ),
        },
    }
}
fn bind_command(bind: &super::BufferBind, at: u64) -> Command {
    let banks = bind.banks();
    let kind = match &bind.target {
        super::BufferTarget::Voice(key) => Kind::BindVoice {
            key: *key,
            resource: bind.resource,
            banks,
        },
        super::BufferTarget::Chain(id) => Kind::BindChain {
            node: crate::routing::fx_name(bind.client, id),
            resource: bind.resource,
            banks,
        },
    };
    Command { at, kind }
}

fn map_command(program: &Program, set: &super::VoiceMap, at: u64) -> Command {
    Command {
        at,
        kind: Kind::Map {
            key: set.key,
            param: set.param.clone(),
            mapping: super::parameters::prepare_mapping(
                program,
                set.source.as_deref(),
                set.channel,
                set.scale,
                set.offset,
                set.range,
            ),
        },
    }
}

/// A `midi_out` note on or off, if anyone is listening. Allocates nothing.
/// On a full queue, callers reject a new note-on or retain a note-off to retry.
fn send_midi(
    tx: &mut Option<Producer<crate::MidiEvent>>,
    wake: &Option<std::thread::Thread>,
    at: u64,
    on: bool,
    note: &niminal_lang::MidiNote,
) -> bool {
    if let Some(tx) = tx {
        let sent = tx
            .push(crate::MidiEvent {
                at,
                on,
                port: note.port.clone(),
                channel: note.channel,
                note: note.note,
                velocity: note.velocity,
            })
            .is_ok();
        if sent && let Some(thread) = wake {
            thread.unpark();
        }
        sent
    } else {
        true
    }
}

#[allow(clippy::large_enum_variant)] // boxing would allocate on the audio thread
enum Retired {
    Window(Box<Window>),
    Voice(Voice),
    Injected(Box<Injected>),
}

/// A command prepared for a stretch that has been prepared already, to be merged into it by sample.
/// It belongs to one revision, as a window does, and is dropped with it.
struct Injected {
    revision: u64,
    command: Command,
}

/// Something added since the planner last looked.
pub(super) enum Fresh {
    Note {
        event: Event,
        start: u64,
        dur: u64,
        key: Option<u32>,
        activity: Option<Arc<niminal_engine::Activity>>,
    },
    Release {
        key: u32,
        at: u64,
    },
    Set(super::VoiceSet),
    Node(super::NodeSet),
    Buffer(super::BufferSet),
    Map(super::VoiceMap),
    Bind(super::BufferBind),
    ChainMap(super::ChainMap),
}

impl Fresh {
    fn at(&self) -> u64 {
        match self {
            Fresh::Note { start, .. } => *start,
            Fresh::Release { at, .. } => *at,
            Fresh::Set(set) => set.at,
            Fresh::Node(set) => set.at,
            Fresh::Buffer(set) => set.at,
            Fresh::Map(set) => set.at,
            Fresh::Bind(set) => set.at,
            Fresh::ChainMap(map) => map.at,
        }
    }
}

pub(super) struct Planner {
    ready: Producer<Box<Window>>,
    inject_tx: Producer<Box<Injected>>,
    control_tx: Producer<ControlMessage>,
    control_applied: Consumer<Applied>,
    retired: Consumer<Retired>,
    pub(super) state: Arc<State>,
    config: RealtimeConfig,
    prepared_until: u64,
    revision: u64,
    swap_epoch: u64,
    reported: RenderMetrics,
    refresh: bool,
    replan: bool,
    deferred: Vec<Command>,
    stopped: Vec<String>,
}

impl Planner {
    pub(super) fn lookahead(&self) -> u64 {
        self.config.lookahead_frames
    }

    pub(super) fn push_control(&mut self, change: ControlMessage) -> Result<(), ()> {
        self.control_tx.push(change).map_err(|_| ())
    }
}

/// The render thread owns all running DSP state. Construction, pattern queries,
/// graph compilation, and resource destruction never take place in `render`.
pub struct RealtimeRenderer {
    master_meter: Arc<niminal_engine::Meter>,
    ready: Consumer<Box<Window>>,
    inject_rx: Consumer<Box<Injected>>,
    /// Commands for stretches prepared already, in the order they run.
    #[allow(clippy::vec_box)] // moved to the retirement queue whole, without copying
    injected: Vec<Box<Injected>>,
    control_rx: Consumer<ControlMessage>,
    control_applied: Producer<Applied>,
    control_pending: Vec<Change>,
    controls: Vec<ControlState>,
    retired: Producer<Retired>,
    state: Arc<State>,
    panic_epoch: Arc<AtomicU64>,
    seen_panic: u64,
    mixer: Box<Mixer>,
    program: Arc<Program>,
    limiter: Option<Master>,
    channels: usize,
    sample_rate: f32,
    clock: u64,
    current: Option<Box<Window>>,
    cursor: usize,
    sounding: Sounding,
    finished: Vec<Voice>,
    config: RealtimeConfig,
    /// Audio from a device, for nodes that read `input(..)`.
    audio_in: Option<crate::AudioInput>,
    /// What the renderer mixes into, and what the audio input delivered, kept so rendering never allocates.
    block: Vec<Vec<f32>>,
    heard: Vec<Vec<f32>>,
    /// Where notes for `midi_out` instruments go, and those still sounding: when they end and the key that holds them.
    midi_tx: Option<Producer<crate::MidiEvent>>,
    midi_wake: Option<std::thread::Thread>,
}


/// When a note starts, how long it lasts, and the key holding it open if there is one.
struct Hold {
    activity: Option<Arc<niminal_engine::Activity>>,
    at: u64,
    dur: u64,
    key: Option<u32>,
}

/// A note the engine was handed: when and how long, what, and the key holding it open.
struct Explicit {
    activity: Option<Arc<niminal_engine::Activity>>,
    start: u64,
    dur: u64,
    event: Event,
    key: Option<u32>,
}


mod plan;
mod render;
#[cfg(test)]
mod tests;
