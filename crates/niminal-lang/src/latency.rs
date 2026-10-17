//! Compute feed-forward timing once, when compiling the project.
use crate::Program;
use niminal_engine::{ChainInput, GraphBuilder, Route, execution_order};
use std::sync::Arc;

impl Program {
    pub(crate) fn compensate_latency(&mut self) {
        let voices = self
            .instruments
            .iter()
            .map(|i| i.graph.latency())
            .max()
            .unwrap_or(0);
        if voices == 0
            && self
                .tracks
                .iter()
                .all(|t| t.def.chain.as_ref().is_none_or(|g| g.latency() == 0))
        {
            return;
        }
        for i in &mut self.instruments {
            let mut graph = i
                .graph
                .with_input_latencies(&vec![0; i.graph.input_count()]);
            graph.pad_sends(&vec![voices; self.buses.len()]);
            graph.pad_outputs(voices);
            i.graph = Arc::new(graph);
        }
        let defs: Vec<_> = self.tracks.iter().map(|t| t.def.clone()).collect();
        let order = execution_order(&defs).expect("compiler checked routing");
        let mut buses = vec![0usize; self.buses.len()];
        let mut tracks = vec![0usize; self.tracks.len()];
        for &t in &order {
            let info = &mut self.tracks[t];
            let voice_latency = if t == 0 || info.instrument.is_some() {
                voices
            } else {
                0
            };
            for &bus in &info.def.voice_sends {
                buses[bus] = buses[bus].max(voice_latency);
            }
            let latency = if let Some(chain) = &info.def.chain {
                let inputs: Vec<_> = info
                    .def
                    .inputs
                    .iter()
                    .map(|i| match i {
                        ChainInput::It { .. } => voice_latency,
                        ChainInput::Bus { bus, .. } => buses[*bus],
                        ChainInput::Audio { .. } => 0,
                    })
                    .collect();
                let graph = chain.with_input_latencies(&inputs);
                let latency = graph.latency();
                for bus in graph.send_buses() {
                    buses[bus] = buses[bus].max(latency);
                }
                info.def.chain = Some(Arc::new(graph));
                latency
            } else {
                voice_latency
            };
            tracks[t] = latency;
            if let Route::Bus(bus) = info.def.route {
                buses[bus] = buses[bus].max(latency);
            }
        }
        let master = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.def.route == Route::Master)
            .map(|(i, _)| tracks[i])
            .max()
            .unwrap_or(0);
        for (t, info) in self.tracks.iter_mut().enumerate() {
            let target = match info.def.route {
                Route::Master => master,
                Route::Bus(bus) => buses[bus],
            };
            if let Some(chain) = &defs[t].chain {
                let mut graph = chain.with_input_latencies(
                    &info
                        .def
                        .inputs
                        .iter()
                        .map(|i| match i {
                            ChainInput::It { .. } => {
                                if t == 0 || info.instrument.is_some() {
                                    voices
                                } else {
                                    0
                                }
                            }
                            ChainInput::Bus { bus, .. } => buses[*bus],
                            ChainInput::Audio { .. } => 0,
                        })
                        .collect::<Vec<_>>(),
                );
                graph.pad_sends(&buses);
                graph.pad_outputs(target);
                info.def.chain = Some(Arc::new(graph));
            } else if target > tracks[t] {
                let mut graph = GraphBuilder::new();
                let mut outputs = Vec::new();
                for channel in 0..self.master.channels() {
                    outputs.push(graph.input_with_latency(tracks[t]));
                    info.def.inputs.push(ChainInput::It { channel });
                }
                let mut graph = graph.build_channels(&outputs);
                graph.pad_outputs(target);
                info.def.chain = Some(Arc::new(graph));
            }
        }
        for instrument in &mut self.instruments {
            let mut graph =
                instrument
                    .graph
                    .with_input_latencies(&vec![0; instrument.graph.input_count()]);
            graph.pad_sends(&buses);
            instrument.graph = Arc::new(graph);
        }
    }

    pub fn processing_latency(&self) -> usize {
        self.tracks
            .iter()
            .filter(|t| t.def.route == Route::Master)
            .map(|t| {
                t.def.chain.as_ref().map_or_else(
                    || {
                        if t.name.is_empty() || t.instrument.is_some() {
                            self.instruments
                                .iter()
                                .map(|i| i.graph.latency())
                                .max()
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    },
                    |g| g.latency(),
                )
            })
            .max()
            .unwrap_or(0)
    }
}
