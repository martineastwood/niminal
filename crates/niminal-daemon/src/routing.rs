//! Client-owned instances and connections, independent of the DSP definitions they use.
use niminal_engine::{ChainInput, GraphBuilder, Route, Src, TrackDef, execution_order};
use niminal_lang::{
    CompileOptions, ControlDef, Layout, Program, TrackInfo, Unit, compile_dsp_with,
};
use niminal_score::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Endpoint {
    Node {
        id: String,
    },
    Bus {
        id: String,
        #[serde(default)]
        owner: Option<u64>,
    },
    Master,
}

/// Edits in one routing operation are applied and validated together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutingEdit {
    /// A reusable processor signature and body, e.g. `(wet: num = 0.5) { out = it * wet }`.
    Processor {
        name: String,
        source: String,
    },
    /// Use a compiled language track as a template, removing its implicit instance.
    Import {
        name: String,
        track: String,
    },
    Bus {
        id: String,
        channels: usize,
        #[serde(default)]
        shared: bool,
        #[serde(default)]
        source: Option<String>,
    },
    Node {
        id: String,
        #[serde(default)]
        processor: Option<String>,
        #[serde(default)]
        instrument: Option<String>,
        #[serde(default)]
        args: BTreeMap<String, Value>,
    },
    Connect {
        id: String,
        from: Endpoint,
        to: Endpoint,
        #[serde(default = "unity")]
        gain: f64,
        #[serde(default)]
        glide: f64,
        #[serde(default)]
        delayed: bool,
    },
    Set {
        id: String,
        args: BTreeMap<String, Value>,
        #[serde(default)]
        glide: f64,
    },
    Disconnect {
        id: String,
    },
    /// Disconnect incoming audio and refuse new notes; output remains connected for tails.
    Drain {
        id: String,
    },
    FreeNode {
        id: String,
    },
    FreeBus {
        id: String,
    },
    FreeProcessor {
        name: String,
    },
    /// Reconcile a client's graph after a project edit without recreating retained instances.
    Retain {
        nodes: Vec<String>,
        processors: Vec<String>,
        buses: Vec<String>,
        connections: Vec<String>,
    },
    Clear,
}
fn unity() -> f64 {
    1.0
}

#[derive(Clone)]
struct Processor {
    track: TrackInfo,
    controls: Vec<ControlDef>,
    buses: Vec<String>,
    imported: Option<String>,
    instrument: Option<String>,
}
#[derive(Clone)]
struct Bus {
    channels: usize,
    shared: bool,
    source: Option<String>,
}
#[derive(Clone)]
struct Node {
    processor: Option<(String, Processor)>,
    instrument: Option<String>,
    args: BTreeMap<String, Value>,
    glide: f64,
    draining: bool,
}
#[derive(Clone)]
struct Connection {
    from: Endpoint,
    to: Endpoint,
    gain: f64,
    glide: f64,
    delayed: bool,
}
#[derive(Clone, Default)]
struct Patch {
    processors: BTreeMap<String, Processor>,
    buses: BTreeMap<String, Bus>,
    nodes: BTreeMap<String, Node>,
    connections: BTreeMap<String, Connection>,
}
#[derive(Clone, Default)]
pub(crate) struct Routing {
    clients: BTreeMap<u64, Patch>,
}

pub(crate) fn node_name(client: u64, id: &str) -> String {
    format!("__routing_node_{client}_{id}")
}
fn valid(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        Err("routing IDs use 1–128 ASCII letters, digits or underscores".into())
    } else {
        Ok(())
    }
}
fn smoothing(seconds: f64) -> Result<(), String> {
    if seconds.is_finite() && (0.0..=60.0).contains(&seconds) {
        Ok(())
    } else {
        Err("glide must be finite and between 0 and 60 seconds".into())
    }
}

impl Routing {
    pub fn has_node(&self, client: u64, id: &str) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|p| p.nodes.contains_key(id))
    }
    pub fn is_empty(&self, client: u64) -> bool {
        self.clients.get(&client).is_none_or(|p| {
            p.nodes.is_empty()
                && p.buses.is_empty()
                && p.processors.is_empty()
                && p.connections.is_empty()
        })
    }

    pub fn apply(
        &mut self,
        client: u64,
        edits: &[RoutingEdit],
        definitions: &Program,
        options: &CompileOptions,
    ) -> Result<(), String> {
        let patch = self.clients.entry(client).or_default();
        for edit in edits {
            match edit {
                RoutingEdit::Processor { name, source } => {
                    valid(name)?;
                    let internal = format!("__routing_template_{client}_{name}");
                    let declaration = format!("track {internal}{source}");
                    let statements = niminal_lang::analyze(&declaration).map_err(|d| d.message)?;
                    if statements.len() != 1
                        || statements[0].key() != Some(format!("track:{internal}").as_str())
                    {
                        return Err("processor source must be one signature and body".into());
                    }
                    let text = format!("{}\n{declaration}", definitions.source);
                    let mut options = options.clone();
                    options.buffers = definitions.buffers.iter().cloned().collect();
                    let compiled = compile_dsp_with(&text, &options).map_err(|ds| {
                        ds.into_iter()
                            .map(|d| d.message)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })?;
                    let template = compiled
                        .tracks
                        .iter()
                        .find(|t| t.name == internal)
                        .expect("the validated processor declaration compiled");
                    if template.instrument.is_some() || template.notes.is_some() {
                        return Err("a processor processes audio; select instruments when creating nodes or sending notes".into());
                    }
                    let controls = compiled
                        .controls
                        .iter()
                        .filter(|c| c.name.starts_with(&format!("{internal}::")))
                        .cloned()
                        .collect();
                    patch.processors.insert(
                        name.clone(),
                        Processor {
                            track: template.clone(),
                            controls,
                            buses: compiled.buses,
                            imported: None,
                            instrument: None,
                        },
                    );
                }
                RoutingEdit::Import { name, track } => {
                    valid(name)?;
                    let template = definitions
                        .tracks
                        .iter()
                        .skip(1)
                        .find(|t| t.name == *track)
                        .ok_or_else(|| format!("no track template `{track}`"))?;
                    let controls = definitions
                        .controls
                        .iter()
                        .filter(|c| c.name.starts_with(&format!("{track}::")))
                        .cloned()
                        .collect();
                    patch.processors.insert(
                        name.clone(),
                        Processor {
                            track: template.clone(),
                            controls,
                            buses: definitions.buses.clone(),
                            imported: Some(track.clone()),
                            instrument: template
                                .instrument
                                .map(|i| definitions.instruments[i].name.clone()),
                        },
                    );
                }
                RoutingEdit::Bus {
                    id,
                    channels,
                    shared,
                    source,
                } => {
                    valid(id)?;
                    if !(1..=niminal_engine::MAX_CHANNELS).contains(channels) {
                        return Err("bus channels must be between 1 and 16".into());
                    }
                    if let Some(source) = source {
                        let index = definitions
                            .buses
                            .iter()
                            .position(|n| n == source)
                            .ok_or_else(|| format!("no source bus `{source}`"))?;
                        if definitions.bus_layouts[index].channels() != *channels {
                            return Err("imported bus channel count differs".into());
                        }
                    }
                    if let Some(old) = patch.buses.get(id)
                        && (old.channels != *channels || old.source != *source)
                    {
                        return Err("free a bus before changing its layout or identity".into());
                    }
                    patch.buses.insert(
                        id.clone(),
                        Bus {
                            channels: *channels,
                            shared: *shared,
                            source: source.clone(),
                        },
                    );
                }
                RoutingEdit::Node {
                    id,
                    processor,
                    instrument,
                    args,
                } => {
                    valid(id)?;
                    let template = processor
                        .as_ref()
                        .map(|name| {
                            patch
                                .processors
                                .get(name)
                                .cloned()
                                .ok_or_else(|| format!("no processor `{name}`"))
                        })
                        .transpose()?;
                    patch.nodes.insert(
                        id.clone(),
                        Node {
                            processor: processor.clone().zip(template),
                            instrument: instrument.clone(),
                            args: args.clone(),
                            glide: 0.0,
                            draining: false,
                        },
                    );
                }
                RoutingEdit::Connect {
                    id,
                    from,
                    to,
                    gain,
                    glide,
                    delayed,
                } => {
                    valid(id)?;
                    smoothing(*glide)?;
                    if !gain.is_finite() || gain.abs() > f64::from(f32::MAX) {
                        return Err("connection gain must be a finite f32 value".into());
                    }
                    patch.connections.insert(
                        id.clone(),
                        Connection {
                            from: from.clone(),
                            to: to.clone(),
                            gain: *gain,
                            glide: *glide,
                            delayed: *delayed,
                        },
                    );
                }
                RoutingEdit::Set { id, args, glide } => {
                    smoothing(*glide)?;
                    let node = patch
                        .nodes
                        .get_mut(id)
                        .ok_or_else(|| format!("no node `{id}`"))?;
                    node.args.extend(args.clone());
                    node.glide = *glide;
                }
                RoutingEdit::Disconnect { id } => {
                    if patch.connections.remove(id).is_none() {
                        return Err(format!("no connection `{id}`"));
                    }
                }
                RoutingEdit::Drain { id } => {
                    patch
                        .nodes
                        .get_mut(id)
                        .ok_or_else(|| format!("no node `{id}`"))?
                        .draining = true;
                    patch
                        .connections
                        .retain(|_, c| !matches!(&c.to, Endpoint::Node { id: to } if to == id));
                }
                RoutingEdit::FreeNode { id } => {
                    if patch.nodes.remove(id).is_none() {
                        return Err(format!("no node `{id}`"));
                    }
                    patch.connections.retain(|_, c| {
                        !matches!(&c.from, Endpoint::Node { id: n } if n == id)
                            && !matches!(&c.to, Endpoint::Node { id: n } if n == id)
                    });
                }
                RoutingEdit::FreeBus { id } => {
                    if patch.buses.remove(id).is_none() {
                        return Err(format!("no bus `{id}`"));
                    }
                }
                RoutingEdit::FreeProcessor { name } => {
                    if patch.processors.remove(name).is_none() {
                        return Err(format!("no processor `{name}`"));
                    }
                }
                RoutingEdit::Retain {
                    nodes,
                    processors,
                    buses,
                    connections,
                } => {
                    patch.nodes.retain(|id, _| nodes.contains(id));
                    patch.processors.retain(|id, _| processors.contains(id));
                    patch.buses.retain(|id, _| buses.contains(id));
                    patch.connections.retain(|id, _| connections.contains(id));
                }
                RoutingEdit::Clear => *patch = Patch::default(),
            }
        }
        if patch.nodes.len() + patch.connections.len() > 4096
            || patch.buses.len() > 1024
            || patch.processors.len() > 1024
        {
            return Err("routing capacity exceeded".into());
        }
        if edits.iter().any(|e| matches!(e, RoutingEdit::Clear)) {
            for patch in self.clients.values_mut() {
                patch.connections.retain(|_, c| ![&c.from, &c.to].iter().any(|e| matches!(e, Endpoint::Bus { owner: Some(owner), .. } if *owner == client)));
            }
        }
        if self.is_empty(client) {
            self.clients.remove(&client);
        }
        Ok(())
    }

    /// Build routing around already compiled DSP graphs. Connections never compile DSP source.
    pub fn materialize(&self, definitions: &Program) -> Result<Program, String> {
        let mut program = definitions.clone();
        let consumed: BTreeSet<&str> = self
            .clients
            .values()
            .flat_map(|p| {
                p.processors.values().chain(
                    p.nodes
                        .values()
                        .filter_map(|n| n.processor.as_ref().map(|(_, template)| template)),
                )
            })
            .filter_map(|p| p.imported.as_deref())
            .collect();
        program
            .tracks
            .retain(|t| !consumed.contains(t.name.as_str()));
        program.controls.retain(|c| {
            !c.name
                .split_once("::")
                .is_some_and(|(track, _)| consumed.contains(track))
        });
        let channels = program.master.channels();
        let mut buses = BTreeMap::new();
        for (&owner, patch) in &self.clients {
            for (id, bus) in &patch.buses {
                let index = if let Some(source) = &bus.source {
                    program
                        .buses
                        .iter()
                        .position(|n| n == source)
                        .ok_or_else(|| format!("imported bus `{source}` was removed"))?
                } else {
                    add_bus(
                        &mut program,
                        format!("__routing_bus_{owner}_{id}"),
                        bus.channels,
                    )
                };
                buses.insert((owner, id.clone()), (index, bus.shared));
            }
        }
        let mut inputs = BTreeMap::new();
        let mut outputs = BTreeMap::new();
        for (&owner, patch) in &self.clients {
            for (id, node) in &patch.nodes {
                inputs.insert(
                    (owner, id.clone()),
                    add_bus(
                        &mut program,
                        format!("{}__in", node_name(owner, id)),
                        channels,
                    ),
                );
                outputs.insert(
                    (owner, id.clone()),
                    add_bus(
                        &mut program,
                        format!("{}__out", node_name(owner, id)),
                        node.processor
                            .as_ref()
                            .map(|(_, template)| template)
                            .and_then(|p| p.track.def.chain.as_ref())
                            .map_or(channels, |g| g.channels()),
                    ),
                );
            }
        }
        for (&owner, patch) in &self.clients {
            for (id, node) in &patch.nodes {
                let name = node_name(owner, id);
                let mut track = if let Some((_, processor)) = &node.processor {
                    let mut track = processor.track.clone();
                    let mapping: Vec<_> = processor
                        .buses
                        .iter()
                        .map(|b| program.buses.iter().position(|n| n == b))
                        .collect();
                    for input in &mut track.def.inputs {
                        if let ChainInput::Bus { bus, .. } = input {
                            *bus = mapping
                                .get(*bus)
                                .copied()
                                .flatten()
                                .ok_or("processor input bus was removed")?;
                        }
                    }
                    if let Some(graph) = &track.def.chain {
                        track.def.chain = Some(Arc::new(graph.with_bus_mapping(&mapping)));
                    }
                    track.instrument = processor
                        .instrument
                        .as_ref()
                        .map(|name| {
                            program
                                .instruments
                                .iter()
                                .position(|i| i.name == *name)
                                .ok_or_else(|| {
                                    format!("processor's instrument `{name}` was removed")
                                })
                        })
                        .transpose()?;
                    track.def.voice_sends = track
                        .def
                        .voice_sends
                        .iter()
                        .filter_map(|&b| mapping.get(b).copied().flatten())
                        .collect();
                    track.def.controls.clear();
                    for control in &processor.controls {
                        let param = control
                            .name
                            .split_once("::")
                            .expect("compiled track controls are qualified")
                            .1;
                        let mut exposed = control.clone();
                        exposed.name = format!("{name}::{param}");
                        exposed.key = exposed.name.clone();
                        if let Some(value) = node.args.get(param) {
                            if matches!(value, Value::Beats(_) | Value::Bars(_)) {
                                return Err("resolve musical parameter units in the client".into());
                            }
                            exposed.initial = control
                                .from_value(*value, program.tempo)
                                .map_err(|e| e.to_string())?;
                            if !exposed.initial.is_finite() {
                                return Err("processor parameters must be finite f32 values".into());
                            }
                            exposed.smooth_seconds = node.glide;
                        }
                        track
                            .def
                            .controls
                            .push((exposed.key.clone(), control.key.clone()));
                        program.controls.push(exposed);
                    }
                    for arg in node.args.keys() {
                        if !processor
                            .controls
                            .iter()
                            .any(|c| c.name.ends_with(&format!("::{arg}")))
                        {
                            return Err(format!("unknown processor parameter `{arg}`"));
                        }
                    }
                    track
                } else {
                    if !node.args.is_empty() {
                        return Err("a collection node has no processor parameters".into());
                    }
                    TrackInfo::default()
                };
                if let Some(instrument) = &node.instrument {
                    track.instrument = Some(
                        program
                            .instruments
                            .iter()
                            .position(|i| i.name == *instrument)
                            .ok_or_else(|| format!("no instrument `{instrument}`"))?,
                    );
                }
                // Voice collection is independent of the processor instance. Both feed the same input bus.
                let incoming = inputs[&(owner, id.clone())];
                let collector = TrackInfo {
                    name: name.clone(),
                    instrument: track.instrument,
                    defaults: track.defaults.clone(),
                    accepts_notes: !node.draining,
                    def: TrackDef {
                        route: Route::Bus(incoming),
                        voice_sends: track.def.voice_sends.clone(),
                        voice_buffer_reads: track.def.voice_buffer_reads.clone(),
                        voice_buffer_writes: track.def.voice_buffer_writes.clone(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                program.tracks.push(collector);
                track.name = format!("__routing_processor_{owner}_{id}");
                track.instrument = None;
                track.accepts_notes = false;
                if track.def.chain.is_none() {
                    let mut graph = GraphBuilder::new();
                    let outputs = (0..channels).map(|_| graph.input()).collect::<Vec<_>>();
                    track.def.chain = Some(Arc::new(graph.build_channels(&outputs)));
                    track.def.inputs = (0..channels)
                        .map(|channel| ChainInput::It { channel })
                        .collect();
                }
                for input in &mut track.def.inputs {
                    if let ChainInput::It { channel } = *input {
                        *input = ChainInput::Bus {
                            bus: incoming,
                            channel,
                        };
                    }
                }
                track.def.voice_sends.clear();
                track.def.voice_buffer_reads.clear();
                track.def.voice_buffer_writes.clear();
                track.def.route = Route::Bus(outputs[&(owner, id.clone())]);
                program.tracks.push(track);
            }
            for (id, connection) in &patch.connections {
                let (source, from_channels) = endpoint_bus(
                    &connection.from,
                    owner,
                    true,
                    &inputs,
                    &outputs,
                    &buses,
                    &program,
                )?;
                let (target, to_channels) = endpoint_bus(
                    &connection.to,
                    owner,
                    false,
                    &inputs,
                    &outputs,
                    &buses,
                    &program,
                )?;
                if from_channels != to_channels {
                    return Err(
                        "connection channel layouts must match; insert a conversion processor"
                            .into(),
                    );
                }
                let source = source.expect("master was rejected as a source");
                if connection.delayed && target.is_none() {
                    return Err("a delayed connection needs a bus or node destination".into());
                }
                let mut graph = GraphBuilder::new();
                let key = format!("__routing_gain_{owner}_{id}");
                let gain = graph
                    .add(
                        niminal_engine::ops::Control::new(key.clone(), connection.gain as f32),
                        &[],
                    )
                    .expect("control ports");
                let mut ports = Vec::new();
                let mut result = Vec::new();
                for channel in 0..from_channels {
                    let input = graph.input();
                    ports.push(ChainInput::Bus {
                        bus: source,
                        channel,
                    });
                    let value = graph
                        .add(niminal_engine::ops::Mul, &[("a", input), ("b", gain)])
                        .expect("multiply ports");
                    if connection.delayed {
                        graph.send_channel_delayed(
                            target.expect("delayed destination was validated"),
                            channel,
                            value,
                        );
                    } else {
                        result.push(value);
                    }
                }
                if connection.delayed {
                    result = vec![Src::Const(0.0); channels];
                }
                let graph = Arc::new(graph.build_channels(&result));
                program.controls.push(ControlDef {
                    name: key.clone(),
                    key,
                    unit: Unit::Num,
                    initial: connection.gain as f32,
                    smooth_seconds: connection.glide,
                    input: None,
                });
                program.tracks.push(TrackInfo {
                    name: format!("__routing_connection_{owner}_{id}"),
                    def: TrackDef {
                        chain: Some(graph),
                        inputs: ports,
                        route: if connection.delayed {
                            Route::Master
                        } else {
                            target.map_or(Route::Master, Route::Bus)
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }
        }
        let defs = program
            .tracks
            .iter()
            .map(|t| t.def.clone())
            .collect::<Vec<_>>();
        execution_order(&defs).map_err(|cycle| {
            format!(
                "routing feedback requires a delayed connection: {}",
                cycle
                    .iter()
                    .map(|&i| program.tracks[i].name.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )
        })?;
        program.compensate_latency();
        Ok(program)
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!(self.clients.iter().map(|(client, patch)| serde_json::json!({
            "client": client,
            "processors": patch.processors.keys().collect::<Vec<_>>(),
            "buses": patch.buses.iter().map(|(id, b)| serde_json::json!({ "id": id, "channels": b.channels, "shared": b.shared })).collect::<Vec<_>>(),
            "nodes": patch.nodes.iter().map(|(id, n)| serde_json::json!({
                "id": id, "processor": n.processor.as_ref().map(|(name, _)| name),
                "instrument": n.instrument, "args": n.args, "draining": n.draining,
            })).collect::<Vec<_>>(),
            "connections": patch.connections.iter().map(|(id, c)| serde_json::json!({
                "id": id, "from": c.from, "to": c.to, "gain": c.gain, "delayed": c.delayed,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>())
    }
}
fn add_bus(program: &mut Program, name: String, channels: usize) -> usize {
    let index = program.buses.len();
    program.buses.push(name);
    program.bus_layouts.push(Layout::from_count(channels));
    index
}
fn endpoint_bus(
    endpoint: &Endpoint,
    client: u64,
    source: bool,
    inputs: &BTreeMap<(u64, String), usize>,
    outputs: &BTreeMap<(u64, String), usize>,
    buses: &BTreeMap<(u64, String), (usize, bool)>,
    program: &Program,
) -> Result<(Option<usize>, usize), String> {
    let index = match endpoint {
        Endpoint::Master if !source => return Ok((None, program.master.channels())),
        Endpoint::Master => return Err("master cannot be a connection source".into()),
        Endpoint::Node { id } => *(if source { outputs } else { inputs })
            .get(&(client, id.clone()))
            .ok_or_else(|| format!("no node `{id}`"))?,
        Endpoint::Bus { id, owner } => {
            let owner = owner.unwrap_or(client);
            let &(bus, shared) = buses
                .get(&(owner, id.clone()))
                .ok_or_else(|| format!("no bus `{id}`"))?;
            if owner != client && !shared {
                return Err("another client's bus is private".into());
            }
            bus
        }
    };
    Ok((Some(index), program.bus_layouts[index].channels()))
}
