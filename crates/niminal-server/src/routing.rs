//! Client-owned instances and connections, independent of the DSP definitions they use.
use niminal_engine::{ChainInput, GraphBuilder, NodeDef, Route, Src, execution_order};
use niminal_lang::{CompileOptions, ControlDef, Layout, NodeInfo, Program, Unit, compile_with};
use niminal_score::{Tempo, Value};
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
    Array {
        id: String,
        param: String,
        values: Vec<Value>,
        #[serde(default)]
        glide: f64,
    },
    BindBuffer {
        id: String,
        param: String,
        buffer: String,
    },
    Player {
        id: String,
        #[serde(flatten)]
        player: crate::Player,
    },
    Seek {
        id: String,
        seconds: f64,
    },
    Map {
        id: String,
        param: String,
        #[serde(default)]
        source: Option<ModSource>,
        #[serde(default = "one")]
        channel: usize,
        #[serde(default = "unity32")]
        scale: f32,
        #[serde(default)]
        offset: f32,
    },
    /// A reusable fx signature and body, e.g. `(wet: num = 0.5) { out = it * wet }`.
    Fx {
        name: String,
        source: String,
        /// Explicit native output layout; input collections still use the master layout.
        #[serde(default)]
        output_channels: Option<usize>,
    },
    /// Use a compiled fx or collection as a template, removing its implicit instance.
    Import {
        name: String,
        node: String,
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
        fx: Option<String>,
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
    /// Replace this instance's DSP, preserving matching parameters unless overridden.
    Replace {
        id: String,
        fx: String,
        #[serde(default)]
        args: BTreeMap<String, Value>,
        #[serde(default)]
        fade: f64,
    },
    /// Reset persistent fx memory (for example oscillator phase), retaining voices and parameter values.
    Reset {
        id: String,
    },
    /// Move an existing connection's gain without changing its topology.
    Gain {
        id: String,
        value: f64,
        #[serde(default)]
        glide: f64,
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
    FreeFx {
        name: String,
    },
    /// Reconcile a client's graph after a project edit without recreating retained instances.
    Retain {
        nodes: Vec<String>,
        fx_decls: Vec<String>,
        buses: Vec<String>,
        connections: Vec<String>,
    },
    Clear,
}
fn one() -> usize {
    1
}
fn unity32() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModSource {
    Control {
        name: String,
    },
    Bus {
        id: String,
        #[serde(default)]
        owner: Option<u64>,
    },
}

fn unity() -> f64 {
    1.0
}

#[derive(Clone)]
struct Fx {
    resources: BTreeMap<String, u64>,
    arrays: BTreeMap<String, Vec<String>>,
    node: NodeInfo,
    controls: Vec<ControlDef>,
    buses: Vec<String>,
    imported: Option<String>,
    instrument: Option<String>,
}
#[derive(Clone)]
struct Bus {
    meter: Arc<niminal_engine::Meter>,
    channels: usize,
    shared: bool,
    source: Option<String>,
}
#[derive(Clone)]
struct Node {
    meter: Arc<niminal_engine::Meter>,
    generation: u64,
    reset_state: bool,
    fade: f64,
    fade_until: u64,
    fx: Option<(String, Fx)>,
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
    fx_decls: BTreeMap<String, Fx>,
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
fn connection_key(client: u64, id: &str) -> String {
    format!("__routing_gain_{client}_{id}")
}
fn gain(value: f64) -> Result<f32, String> {
    if value.is_finite() && value.abs() <= f64::from(f32::MAX) {
        Ok(value as f32)
    } else {
        Err("connection gain must be a finite f32 value".into())
    }
}
fn smoothing(seconds: f64) -> Result<(), String> {
    if seconds.is_finite() && (0.0..=60.0).contains(&seconds) {
        Ok(())
    } else {
        Err("glide must be finite and between 0 and 60 seconds".into())
    }
}
/// A compiled control's name without the declaration it was compiled from.
fn param_name(control: &ControlDef) -> &str {
    control
        .name
        .rsplit_once("::")
        .map_or(control.name.as_str(), |(_, param)| param)
}
/// A value sent by a client, as the finite f32 a control holds.
fn control_value(control: &ControlDef, value: Value, tempo: Tempo) -> Result<f32, String> {
    if matches!(value, Value::Beats(_) | Value::Bars(_)) {
        return Err("resolve musical units in the client".into());
    }
    let value = control
        .from_value(value, tempo)
        .map_err(|e| e.to_string())?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err("fx parameters must be finite".into())
    }
}

impl Routing {
    pub fn player_keys(&self, owner: Option<u64>) -> Vec<String> {
        self.clients
            .iter()
            .filter(|(client, _)| owner.is_none_or(|id| id == **client))
            .flat_map(|(client, patch)| {
                patch
                    .nodes
                    .iter()
                    .filter(|(_, node)| {
                        node.fx
                            .as_ref()
                            .is_some_and(|(name, _)| name == "file_player")
                    })
                    .map(move |(id, _)| format!("{}::playing", node_name(*client, id)))
            })
            .collect()
    }
    pub fn buffer_binding(
        &self,
        client: u64,
        id: &str,
        param: &str,
    ) -> Result<(u64, usize, usize), String> {
        let fx = &self
            .clients
            .get(&client)
            .and_then(|p| p.nodes.get(id))
            .and_then(|n| n.fx.as_ref())
            .ok_or("no fx node")?
            .1;
        let key = *fx.resources.get(param).ok_or("no buffer parameter")?;
        let count = fx
            .node
            .def
            .chain
            .as_ref()
            .map_or(0, |g| g.buffer_bindings(key));
        Ok((
            key,
            count,
            fx.node
                .def
                .chain
                .as_ref()
                .map_or(0, |g| g.buffer_channels(key)),
        ))
    }
    pub fn parameter_unit(&self, client: u64, id: &str, param: &str) -> Option<Unit> {
        self.clients
            .get(&client)?
            .nodes
            .get(id)?
            .fx
            .as_ref()?
            .1
            .controls
            .iter()
            .find(|c| param_name(c) == param)
            .map(|c| c.unit)
    }
    pub fn parameter(&self, client: u64, id: &str, param: &str) -> Result<String, String> {
        let node = self
            .clients
            .get(&client)
            .and_then(|p| p.nodes.get(id))
            .ok_or("no such node")?;
        let fx = node
            .fx
            .as_ref()
            .ok_or("collection node has no parameters")?;
        if fx.0 == "file_player" && param != "rate" {
            return Err("only file-player rate supports modulation".into());
        }
        if fx.1.resources.contains_key(param) {
            return Err("use bind_buffer for resource parameters".into());
        }
        if !fx.1.controls.iter().any(|c| param_name(c) == param) {
            return Err(format!("no parameter `{param}`"));
        }
        Ok(format!("{}::{param}", node_name(client, id)))
    }
    pub fn source(&self, client: u64, source: &ModSource) -> Result<String, String> {
        match source {
            ModSource::Control { name } => Ok(crate::session::definitions::scoped(client, name)),
            ModSource::Bus { id, owner } => {
                let owner = owner.unwrap_or(client);
                let bus = self
                    .clients
                    .get(&owner)
                    .and_then(|p| p.buses.get(id))
                    .filter(|b| owner == client || b.shared)
                    .ok_or("no accessible bus")?;
                Ok(bus
                    .source
                    .clone()
                    .unwrap_or_else(|| format!("__routing_bus_{owner}_{id}")))
            }
        }
    }
    pub fn meter(
        &self,
        client: u64,
        endpoint: &Endpoint,
    ) -> Result<Arc<niminal_engine::Meter>, String> {
        match endpoint {
            Endpoint::Node { id } => self
                .clients
                .get(&client)
                .and_then(|p| p.nodes.get(id))
                .map(|n| n.meter.clone())
                .ok_or_else(|| format!("no node `{id}`")),
            Endpoint::Bus { id, owner } => {
                let owner = owner.unwrap_or(client);
                self.clients
                    .get(&owner)
                    .and_then(|p| p.buses.get(id))
                    .filter(|b| owner == client || b.shared)
                    .map(|b| b.meter.clone())
                    .ok_or_else(|| format!("no accessible bus `{id}`"))
            }
            Endpoint::Master => Err("master meter belongs to the session".into()),
        }
    }

    pub fn has_node(&self, client: u64, id: &str) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|p| p.nodes.contains_key(id))
    }
    pub fn is_empty(&self, client: u64) -> bool {
        self.clients.get(&client).is_none_or(|p| {
            p.nodes.is_empty()
                && p.buses.is_empty()
                && p.fx_decls.is_empty()
                && p.connections.is_empty()
        })
    }

    pub fn apply(
        &mut self,
        client: u64,
        edits: &[RoutingEdit],
        definitions: &Program,
        options: &CompileOptions,
        at: u64,
        sample_rate: f32,
    ) -> Result<(), String> {
        let patch = self.clients.entry(client).or_default();
        for edit in edits {
            match edit {
                RoutingEdit::Player { id, player } => {
                    valid(id)?;
                    let prefix = format!("__routing_template_{client}_{id}");
                    let (graph, controls) = crate::streaming::prepare(player, &prefix)?;
                    let fx = Fx {
                        resources: BTreeMap::new(),
                        arrays: BTreeMap::new(),
                        node: NodeInfo {
                            def: NodeDef {
                                chain: Some(graph),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        controls,
                        buses: Vec::new(),
                        imported: None,
                        instrument: None,
                    };
                    let previous = patch.nodes.get(id);
                    let node = Node {
                        meter: previous.map(|n| n.meter.clone()).unwrap_or_default(),
                        generation: previous.map_or(1, |n| n.generation + 1),
                        reset_state: false,
                        fade: 0.,
                        fade_until: 0,
                        fx: Some(("file_player".into(), fx)),
                        instrument: None,
                        args: BTreeMap::new(),
                        glide: 0.,
                        draining: false,
                    };
                    patch.nodes.insert(id.clone(), node);
                }
                RoutingEdit::Seek { id, seconds } => {
                    if !seconds.is_finite() || *seconds < 0. {
                        return Err("seek position is nonnegative seconds".into());
                    }
                    let node = patch
                        .nodes
                        .get_mut(id)
                        .filter(|n| n.fx.as_ref().is_some_and(|(name, _)| name == "file_player"))
                        .ok_or("seek requires a file player")?;
                    node.generation += 1;
                    node.args.insert("seek".into(), Value::Seconds(*seconds));
                    node.args.insert(
                        "seek_trigger".into(),
                        Value::Num((node.generation % (1 << 24)) as f64),
                    );
                }
                RoutingEdit::Map { .. }
                | RoutingEdit::Array { .. }
                | RoutingEdit::BindBuffer { .. } => {}
                RoutingEdit::Fx { name, source, output_channels } => {
                    valid(name)?;
                    let internal = format!("__routing_template_{client}_{name}");
                    let declaration = format!("fx {internal}{source}");
                    let statements = niminal_lang::analyze(&declaration).map_err(|d| d.message)?;
                    if statements.len() != 1 || statements[0].key != format!("fx:{internal}") {
                        return Err("fx source must be one signature and body".into());
                    }
                    let text = format!("{}\n{declaration}", definitions.source);
                    let mut options = options.clone();
                    if let Some(channels) = output_channels {
                        if !(1..=16).contains(channels) { return Err("fx output_channels must be from 1 to 16".into()); }
                        options.node_outputs.insert(internal.clone(), Layout::from_count(*channels));
                    }
                    options.buffers = definitions.buffers.iter().cloned().collect();
                    options.namespaces = definitions.namespaces.clone();
                    options.namespace_samples = definitions.namespace_samples.clone();
                    if client != 0 {
                        options.namespaces.push((
                            definitions.source.len() + 1,
                            text.len(),
                            format!("__client_{client}"),
                        ));
                    }
                    let compiled = compile_with(&text, &options).map_err(|ds| {
                        ds.into_iter()
                            .map(|d| d.message)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })?;
                    let template = compiled
                        .nodes
                        .iter()
                        .find(|t| t.name == internal)
                        .expect("the validated fx declaration compiled");
                    if template.instrument.is_some() {
                        return Err("an fx processes audio; select instruments when creating nodes or sending notes".into());
                    }
                    let controls = compiled
                        .controls
                        .iter()
                        .filter(|c| c.name.starts_with(&format!("{internal}::")))
                        .cloned()
                        .collect();
                    patch.fx_decls.insert(
                        name.clone(),
                        Fx {
                            resources: compiled
                                .resource_controls
                                .iter()
                                .filter_map(|(n, k)| {
                                    n.strip_prefix(&format!("{internal}::"))
                                        .map(|n| (n.to_string(), *k))
                                })
                                .collect(),
                            arrays: compiled
                                .array_controls
                                .iter()
                                .filter_map(|(n, k)| {
                                    n.strip_prefix(&format!("{internal}::"))
                                        .map(|n| (n.to_string(), k.clone()))
                                })
                                .collect(),
                            node: template.clone(),
                            controls,
                            buses: compiled.buses,
                            imported: None,
                            instrument: None,
                        },
                    );
                }
                RoutingEdit::Import {
                    name,
                    node: render_node,
                } => {
                    valid(name)?;
                    let template = definitions
                        .nodes
                        .iter()
                        .skip(1)
                        .find(|t| t.name == *render_node)
                        .ok_or_else(|| format!("no fx or collection template `{render_node}`"))?;
                    let controls = definitions
                        .controls
                        .iter()
                        .filter(|c| c.name.starts_with(&format!("{render_node}::")))
                        .cloned()
                        .collect();
                    patch.fx_decls.insert(
                        name.clone(),
                        Fx {
                            resources: definitions
                                .resource_controls
                                .iter()
                                .filter_map(|(n, k)| {
                                    n.strip_prefix(&format!("{render_node}::"))
                                        .map(|n| (n.to_string(), *k))
                                })
                                .collect(),
                            arrays: definitions
                                .array_controls
                                .iter()
                                .filter_map(|(n, k)| {
                                    n.strip_prefix(&format!("{render_node}::"))
                                        .map(|n| (n.to_string(), k.clone()))
                                })
                                .collect(),
                            node: template.clone(),
                            controls,
                            buses: definitions.buses.clone(),
                            imported: Some(render_node.clone()),
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
                            meter: patch
                                .buses
                                .get(id)
                                .map(|b| b.meter.clone())
                                .unwrap_or_default(),
                            channels: *channels,
                            shared: *shared,
                            source: source.clone(),
                        },
                    );
                }
                RoutingEdit::Node {
                    id,
                    fx,
                    instrument,
                    args,
                } => {
                    valid(id)?;
                    let template = fx
                        .as_ref()
                        .map(|name| {
                            patch
                                .fx_decls
                                .get(name)
                                .cloned()
                                .ok_or_else(|| format!("no fx `{name}`"))
                        })
                        .transpose()?;
                    patch.nodes.insert(
                        id.clone(),
                        Node {
                            meter: patch
                                .nodes
                                .get(id)
                                .map(|n| n.meter.clone())
                                .unwrap_or_default(),
                            generation: patch.nodes.get(id).map_or(1, |n| n.generation + 1),
                            reset_state: false,
                            fade: 0.0,
                            fade_until: 0,
                            fx: fx.clone().zip(template),
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
                    self::gain(*gain)?;
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
                RoutingEdit::Reset { id } => {
                    let node = patch.nodes.get_mut(id).ok_or_else(|| format!("no node `{id}`"))?;
                    node.generation += 1;
                    node.fade = 0.0;
                    node.reset_state = true;
                }
                RoutingEdit::Replace { id, fx, args, fade } => {
                    smoothing(*fade)?;
                    let mut template = patch
                        .fx_decls
                        .get(fx)
                        .cloned()
                        .ok_or_else(|| format!("no fx `{fx}`"))?;
                    let node = patch
                        .nodes
                        .get_mut(id)
                        .ok_or_else(|| format!("no node `{id}`"))?;
                    if *fade > 0.0 {
                        if at < node.fade_until {
                            return Err("this fx is still crossfading; schedule its next replacement after the fade".into());
                        }
                        let previous = node
                            .fx
                            .as_ref()
                            .ok_or("crossfade requires an existing fx")?;
                        for p in [&previous.1, &template] {
                            if p.node
                                .def
                                .inputs
                                .iter()
                                .any(|i| !matches!(i, ChainInput::It { .. }))
                                || p.node
                                    .def
                                    .chain
                                    .as_ref()
                                    .is_some_and(|g| !g.send_buses().is_empty())
                            {
                                return Err("crossfaded fx declarations must use only incoming audio and explicit output connections".into());
                            }
                        }
                        if previous.1.node.def.chain.as_ref().map(|g| g.channels())
                            != template.node.def.chain.as_ref().map(|g| g.channels())
                        {
                            return Err(
                                "crossfaded fx declarations must have the same output layout"
                                    .into(),
                            );
                        }
                    }
                    if let Some((_, previous)) = &node.fx {
                        // a parameter the new fx shares keeps the value it holds now
                        for next in &mut template.controls {
                            if let Some(old) = previous.controls.iter().find(|old| {
                                param_name(old) == param_name(next) && old.unit == next.unit
                            }) {
                                next.initial = old.initial;
                            }
                        }
                    }
                    // explicit values survive only where the new fx has that parameter in the same unit
                    let previous = node.fx.as_ref().map(|(_, fx)| fx);
                    node.args.retain(|param, _| {
                        let unit = template
                            .controls
                            .iter()
                            .find(|control| param_name(control) == param.as_str())
                            .map(|control| control.unit);
                        previous.zip(unit).is_some_and(|(old, unit)| {
                            old.controls
                                .iter()
                                .any(|c| param_name(c) == param.as_str() && c.unit == unit)
                        })
                    });
                    node.args.extend(args.clone());
                    node.fx = Some((fx.clone(), template));
                    node.generation += 1;
                    node.reset_state = false;
                    node.fade = *fade;
                    node.fade_until =
                        at.saturating_add((*fade * f64::from(sample_rate)).round() as u64);
                }
                RoutingEdit::Gain { id, value, glide } => {
                    smoothing(*glide)?;
                    gain(*value)?;
                    let c = patch
                        .connections
                        .get_mut(id)
                        .ok_or_else(|| format!("no connection `{id}`"))?;
                    c.gain = *value;
                    c.glide = *glide;
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
                RoutingEdit::FreeFx { name } => {
                    if patch.fx_decls.remove(name).is_none() {
                        return Err(format!("no fx `{name}`"));
                    }
                }
                RoutingEdit::Retain {
                    nodes,
                    fx_decls,
                    buses,
                    connections,
                } => {
                    patch.nodes.retain(|id, _| nodes.contains(id));
                    patch.fx_decls.retain(|id, _| fx_decls.contains(id));
                    patch.buses.retain(|id, _| buses.contains(id));
                    patch.connections.retain(|id, _| connections.contains(id));
                }
                RoutingEdit::Clear => *patch = Patch::default(),
            }
        }
        if patch.nodes.len() + patch.connections.len() > 4096
            || patch.buses.len() > 1024
            || patch.fx_decls.len() > 1024
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
                p.fx_decls.values().chain(
                    p.nodes
                        .values()
                        .filter_map(|n| n.fx.as_ref().map(|(_, template)| template)),
                )
            })
            .filter_map(|p| p.imported.as_deref())
            .collect();
        program
            .nodes
            .retain(|t| !consumed.contains(t.name.as_str()));
        program.controls.retain(|c| {
            !c.name
                .rsplit_once("::")
                .is_some_and(|(render_node, _)| consumed.contains(render_node))
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
                if program.bus_layouts[index].channels() != bus.channels {
                    return Err("imported bus channel count differs".into());
                }
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
                        node.fx
                            .as_ref()
                            .map(|(_, template)| template)
                            .and_then(|p| p.node.def.chain.as_ref())
                            .map_or(channels, |g| g.channels()),
                    ),
                );
            }
        }
        for (&owner, patch) in &self.clients {
            for (id, node) in &patch.nodes {
                let name = node_name(owner, id);
                let mut render_node = if let Some((_, fx)) = &node.fx {
                    let mut render_node = fx.node.clone();
                    let mapping: Vec<_> = fx
                        .buses
                        .iter()
                        .map(|b| program.buses.iter().position(|n| n == b))
                        .collect();
                    for input in &mut render_node.def.inputs {
                        if let ChainInput::Bus { bus, .. } = input {
                            *bus = mapping
                                .get(*bus)
                                .copied()
                                .flatten()
                                .ok_or("fx input bus was removed")?;
                        }
                    }
                    if let Some(graph) = &render_node.def.chain {
                        render_node.def.chain = Some(Arc::new(graph.with_bus_mapping(&mapping)));
                    }
                    render_node.instrument = fx
                        .instrument
                        .as_ref()
                        .map(|name| {
                            program
                                .instruments
                                .iter()
                                .position(|i| i.name == *name)
                                .ok_or_else(|| format!("fx's instrument `{name}` was removed"))
                        })
                        .transpose()?;
                    render_node.def.voice_sends = render_node
                        .def
                        .voice_sends
                        .iter()
                        .filter_map(|&b| mapping.get(b).copied().flatten())
                        .collect();
                    render_node.def.controls.clear();
                    for control in &fx.controls {
                        let param = param_name(control);
                        let mut exposed = control.clone();
                        exposed.name = format!("{name}::{param}");
                        exposed.key = exposed.name.clone();
                        if let Some(value) = node.args.get(param) {
                            exposed.initial = control_value(control, *value, program.tempo)?;
                            exposed.smooth_seconds = node.glide;
                        }
                        render_node
                            .def
                            .controls
                            .push((exposed.key.clone(), exposed.key.clone()));
                        program.controls.push(exposed);
                    }
                    if let Some(graph) = &render_node.def.chain {
                        let mut graph = graph.expose_controls(
                            &fx.controls
                                .iter()
                                .map(|c| {
                                    (
                                        c.key.as_str(),
                                        program
                                            .controls
                                            .iter()
                                            .rev()
                                            .find(|p| {
                                                p.name == format!("{name}::{}", param_name(c))
                                            })
                                            .unwrap()
                                            .key
                                            .as_str(),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        );
                        for control in &fx.controls {
                            graph.set_param_range(
                                &format!("{name}::{}", param_name(control)),
                                control.range.map(|(lo, hi)| (lo as f32, hi as f32)),
                            );
                        }
                        render_node.def.chain = Some(Arc::new(graph));
                    }
                    for arg in node.args.keys() {
                        if !fx
                            .controls
                            .iter()
                            .any(|control| param_name(control) == arg.as_str())
                        {
                            return Err(format!("unknown fx parameter `{arg}`"));
                        }
                    }
                    render_node
                } else {
                    if !node.args.is_empty() {
                        return Err("a collection node has no fx parameters".into());
                    }
                    NodeInfo::default()
                };
                if let Some(instrument) = &node.instrument {
                    render_node.instrument = Some(
                        program
                            .instruments
                            .iter()
                            .position(|i| i.name == *instrument)
                            .ok_or_else(|| format!("no instrument `{instrument}`"))?,
                    );
                }
                // Voice collection is independent of the fx instance. Both feed the same input bus.
                let incoming = inputs[&(owner, id.clone())];
                let collector = NodeInfo {
                    name: name.clone(),
                    instrument: render_node.instrument,
                    defaults: render_node.defaults.clone(),
                    accepts_notes: !node.draining,
                    def: NodeDef {
                        route: Route::Bus(incoming),
                        voice_sends: render_node.def.voice_sends.clone(),
                        voice_buffer_reads: render_node.def.voice_buffer_reads.clone(),
                        voice_buffer_writes: render_node.def.voice_buffer_writes.clone(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                program.nodes.push(collector);
                render_node.name = format!("__routing_fx_{owner}_{id}");
                render_node.instrument = None;
                render_node.def.meter = Some(node.meter.clone());
                render_node.def.transition = (node.generation, node.fade);
                render_node.def.reset_state = node.reset_state;
                render_node.accepts_notes = false;
                if render_node.def.chain.is_none() {
                    let mut graph = GraphBuilder::new();
                    let outputs = (0..channels).map(|_| graph.input()).collect::<Vec<_>>();
                    render_node.def.chain = Some(Arc::new(graph.build_channels(&outputs)));
                    render_node.def.inputs = (0..channels)
                        .map(|channel| ChainInput::It { channel })
                        .collect();
                }
                for input in &mut render_node.def.inputs {
                    if let ChainInput::It { channel } = *input {
                        *input = ChainInput::Bus {
                            bus: incoming,
                            channel,
                        };
                    }
                }
                render_node.def.voice_sends.clear();
                render_node.def.voice_buffer_reads.clear();
                render_node.def.voice_buffer_writes.clear();
                render_node.def.route = Route::Bus(outputs[&(owner, id.clone())]);
                program.nodes.push(render_node);
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
                        "connection channel layouts must match; insert a conversion fx".into(),
                    );
                }
                let source = source.expect("master was rejected as a source");
                if connection.delayed && target.is_none() {
                    return Err("a delayed connection needs a bus or node destination".into());
                }
                let mut graph = GraphBuilder::new();
                let key = connection_key(owner, id);
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
                    range: None,
                    name: key.clone(),
                    key,
                    unit: Unit::Num,
                    initial: connection.gain as f32,
                    smooth_seconds: connection.glide,
                });
                program.nodes.push(NodeInfo {
                    name: format!("__routing_connection_{owner}_{id}"),
                    def: NodeDef {
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
            .nodes
            .iter()
            .map(|t| t.def.clone())
            .collect::<Vec<_>>();
        execution_order(&defs).map_err(|cycle| {
            format!(
                "routing feedback requires a delayed connection: {}",
                cycle
                    .iter()
                    .map(|&i| program.nodes[i].name.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )
        })?;
        for (&owner, patch) in &self.clients {
            for (id, bus) in &patch.buses {
                program
                    .bus_meters
                    .push((buses[&(owner, id.clone())].0, bus.meter.clone()));
            }
        }
        program.compensate_latency();
        program.profile = Arc::new(niminal_engine::Profile::new(program.nodes.len()));
        Ok(program)
    }

    pub fn is_control_edit(edits: &[RoutingEdit]) -> bool {
        !edits.is_empty()
            && edits.iter().all(|e| {
                matches!(
                    e,
                    RoutingEdit::Set { .. }
                        | RoutingEdit::Gain { .. }
                        | RoutingEdit::Map { .. }
                        | RoutingEdit::Array { .. }
                        | RoutingEdit::BindBuffer { .. }
                )
            })
    }

    pub fn updates(
        &self,
        client: u64,
        edits: &[RoutingEdit],
        program: &Program,
    ) -> Result<Vec<(String, f32, f64)>, String> {
        let patch = self
            .clients
            .get(&client)
            .ok_or("this connection has no routing graph")?;
        let mut updates = Vec::new();
        for edit in edits {
            if let RoutingEdit::Array {
                id,
                param,
                values,
                glide,
            } = edit
            {
                let node = self
                    .clients
                    .get(&client)
                    .and_then(|p| p.nodes.get(id))
                    .ok_or("no node")?;
                let fx = &node.fx.as_ref().ok_or("no fx")?.1;
                let paths = fx.arrays.get(param).ok_or("no array parameter")?;
                if paths.len() != values.len() {
                    return Err("array shape differs".into());
                }
                updates.extend(self.updates(
                    client,
                    &[RoutingEdit::Set {
                        id: id.clone(),
                        args: paths.iter().cloned().zip(values.iter().copied()).collect(),
                        glide: *glide,
                    }],
                    program,
                )?);
                continue;
            }
            match edit {
                RoutingEdit::Map { .. }
                | RoutingEdit::Array { .. }
                | RoutingEdit::BindBuffer { .. } => {}
                RoutingEdit::Set { id, args, glide } => {
                    smoothing(*glide)?;
                    if !patch.nodes.contains_key(id) {
                        return Err(format!("no node `{id}`"));
                    }
                    for (name, value) in args {
                        let key = format!("{}::{name}", node_name(client, id));
                        let def = program
                            .controls
                            .iter()
                            .find(|c| c.key == key)
                            .ok_or_else(|| format!("unknown fx parameter `{name}`"))?;
                        let value = control_value(def, *value, program.tempo)?;
                        updates.push((key, value, *glide));
                    }
                }
                RoutingEdit::Gain { id, value, glide } => {
                    smoothing(*glide)?;
                    if !patch.connections.contains_key(id) {
                        return Err(format!("no connection `{id}`"));
                    }
                    gain(*value)?;
                    updates.push((connection_key(client, id), *value as f32, *glide));
                }
                _ => unreachable!("only control edits use the control path"),
            }
        }
        Ok(updates)
    }

    pub fn meters(&self, take: bool) -> serde_json::Value {
        let clients = self.clients.iter().map(|(client, patch)| {
            serde_json::json!({
                "client": client,
                "nodes": patch.nodes.iter().map(|(id, node)| serde_json::json!({ "id": id, "peaks": node.meter.read(take) })).collect::<Vec<_>>(),
                "buses": patch.buses.iter().map(|(id, bus)| serde_json::json!({ "id": id, "peaks": bus.meter.read(take) })).collect::<Vec<_>>(),
            })
        });
        serde_json::json!(clients.collect::<Vec<_>>())
    }

    pub fn status(&self, values: &[(String, f32, f32, Option<&str>)]) -> serde_json::Value {
        let control = |key: &str| values.iter().find(|v| v.0 == key).map(|v| (v.1, v.2));
        let clients = self.clients.iter().map(|(client, patch)| {
            let nodes = patch
                .nodes
                .iter()
                .map(|(id, node)| {
                    let params = node.fx.as_ref().map(|(_, fx)| {
                        fx
                            .controls
                            .iter()
                            .map(|c| {
                                let param = param_name(c);
                                let (value, target) =
                                    control(&format!("{}::{param}", node_name(*client, id))).unzip();
                                serde_json::json!({
                                    "name": param, "range":c.range,"unit": c.unit.suffix(), "default": c.initial, "smooth": c.smooth_seconds,
                                    "value": value, "target": target,
                                })
                            })
                            .collect::<Vec<_>>()
                    });
                    serde_json::json!({
                        "id": id, "fx": node.fx.as_ref().map(|(name, _)| name),
                        "player":node.fx.as_ref().and_then(|(_,p)|p.node.def.chain.as_ref()).and_then(|g|g.stream_cache()).map(|cache|serde_json::json!({"frames":cache.frames,"channels":cache.channels,"sample_rate":cache.sample_rate,"position":cache.requested.load(std::sync::atomic::Ordering::Relaxed)as f64/cache.sample_rate as f64,"underrun_frames":cache.underruns.load(std::sync::atomic::Ordering::Relaxed),"failed":cache.failed.load(std::sync::atomic::Ordering::Acquire)})),
                        "instrument": node.instrument, "args": node.args, "draining": node.draining,
                        "params": params.unwrap_or_default(),
                        "arrays":node.fx.as_ref().map(|(_,p)|&p.arrays),
                        "buffers":node.fx.as_ref().map(|(_,p)|p.resources.keys().collect::<Vec<_>>()),
                    })
                })
                .collect::<Vec<_>>();
            let connections = patch
                .connections
                .iter()
                .map(|(id, c)| {
                    let (gain, target) = control(&connection_key(*client, id)).unzip();
                    serde_json::json!({
                        "id": id, "from": c.from, "to": c.to, "delayed": c.delayed,
                        "gain": gain.map_or(c.gain, f64::from), "target": target.map_or(c.gain, f64::from),
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "client": client,
                "fx_decls": patch.fx_decls.keys().collect::<Vec<_>>(),
                "buses": patch.buses.iter().map(|(id, b)| serde_json::json!({ "id": id, "channels": b.channels, "shared": b.shared })).collect::<Vec<_>>(),
                "nodes": nodes,
                "connections": connections,
            })
        });
        serde_json::json!(clients.collect::<Vec<_>>())
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
