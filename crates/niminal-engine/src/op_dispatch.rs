use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::ops;

macro_rules! op_enum {
    (
        inline { $( $IV:ident($IT:ty) ),* $(,)? }
        boxed  { $( $BV:ident($BT:ty) ),* $(,)? }
    ) => {
        pub(crate) enum Op {
            $( $IV($IT), )*
            $( $BV(Box<$BT>), )*
            Dynamic(Box<dyn Opcode>),
        }

        impl Op {
            pub fn from_boxed(boxed: Box<dyn Opcode>) -> Self {
                use std::any::TypeId;
                let tid = (*boxed).type_id();
                $(
                    if tid == TypeId::of::<$IT>() {
                        // SAFETY: TypeId verified — the fat-to-thin pointer cast
                        // extracts the data pointer, which points to a valid $IT
                        // that was heap-allocated by the same global allocator.
                        let ptr = Box::into_raw(boxed) as *mut $IT;
                        return Op::$IV(unsafe { *Box::from_raw(ptr) });
                    }
                )*
                $(
                    if tid == TypeId::of::<$BT>() {
                        let ptr = Box::into_raw(boxed) as *mut $BT;
                        return Op::$BV(unsafe { Box::from_raw(ptr) });
                    }
                )*
                Op::Dynamic(boxed)
            }

            /// The concrete opcode behind the enum, for `carry_state` which
            /// downcasts its argument via `Any`.
            pub fn inner_mut(&mut self) -> &mut dyn Opcode {
                match self {
                    $( Op::$IV(v) => v, )*
                    $( Op::$BV(v) => &mut **v, )*
                    Op::Dynamic(v) => &mut **v,
                }
            }
        }

        impl Opcode for Op {
            fn name(&self) -> &str {
                match self {
                    $( Op::$IV(v) => v.name(), )*
                    $( Op::$BV(v) => v.name(), )*
                    Op::Dynamic(v) => v.name(),
                }
            }
            fn control_info(&self) -> Option<(&str, f32)> {
                match self {
                    $( Op::$IV(v) => v.control_info(), )*
                    $( Op::$BV(v) => v.control_info(), )*
                    Op::Dynamic(v) => v.control_info(),
                }
            }
            fn buffer_bindings(&self, key: u64) -> usize {
                match self {
                    $( Op::$IV(v) => v.buffer_bindings(key), )*
                    $( Op::$BV(v) => v.buffer_bindings(key), )*
                    Op::Dynamic(v) => v.buffer_bindings(key),
                }
            }
            fn buffer_channels(&self, key: u64) -> usize {
                match self {
                    $( Op::$IV(v) => v.buffer_channels(key), )*
                    $( Op::$BV(v) => v.buffer_channels(key), )*
                    Op::Dynamic(v) => v.buffer_channels(key),
                }
            }
            fn stream_cache(&self) -> Option<&std::sync::Arc<crate::ops::StreamCache>> {
                match self {
                    $( Op::$IV(v) => v.stream_cache(), )*
                    $( Op::$BV(v) => v.stream_cache(), )*
                    Op::Dynamic(v) => v.stream_cache(),
                }
            }
            fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
                match self {
                    $( Op::$IV(v) => v.swap_buffers(key, banks, cursor), )*
                    $( Op::$BV(v) => v.swap_buffers(key, banks, cursor), )*
                    Op::Dynamic(v) => v.swap_buffers(key, banks, cursor),
                }
            }
            fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
                match self {
                    $( Op::$IV(v) => v.reports(visit), )*
                    $( Op::$BV(v) => v.reports(visit), )*
                    Op::Dynamic(v) => v.reports(visit),
                }
            }
            fn ports(&self) -> &[Port] {
                match self {
                    $( Op::$IV(v) => v.ports(), )*
                    $( Op::$BV(v) => v.ports(), )*
                    Op::Dynamic(v) => v.ports(),
                }
            }
            fn box_clone(&self) -> Box<dyn Opcode> {
                match self {
                    $( Op::$IV(v) => v.box_clone(), )*
                    $( Op::$BV(v) => v.box_clone(), )*
                    Op::Dynamic(v) => v.box_clone(),
                }
            }
            fn prepare(&mut self, sample_rate: f32) {
                match self {
                    $( Op::$IV(v) => v.prepare(sample_rate), )*
                    $( Op::$BV(v) => v.prepare(sample_rate), )*
                    Op::Dynamic(v) => v.prepare(sample_rate),
                }
            }
            fn reset(&mut self) {
                match self {
                    $( Op::$IV(v) => v.reset(), )*
                    $( Op::$BV(v) => v.reset(), )*
                    Op::Dynamic(v) => v.reset(),
                }
            }
            fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
                match self {
                    $( Op::$IV(v) => v.process(ctx, inputs, out), )*
                    $( Op::$BV(v) => v.process(ctx, inputs, out), )*
                    Op::Dynamic(v) => v.process(ctx, inputs, out),
                }
            }
            fn outputs(&self) -> usize {
                match self {
                    $( Op::$IV(v) => v.outputs(), )*
                    $( Op::$BV(v) => v.outputs(), )*
                    Op::Dynamic(v) => v.outputs(),
                }
            }
            fn process_outputs(
                &mut self,
                ctx: &ProcessCtx,
                inputs: &[&[f32]],
                outs: &mut [Vec<f32>],
                frames: usize,
            ) {
                match self {
                    $( Op::$IV(v) => v.process_outputs(ctx, inputs, outs, frames), )*
                    $( Op::$BV(v) => v.process_outputs(ctx, inputs, outs, frames), )*
                    Op::Dynamic(v) => v.process_outputs(ctx, inputs, outs, frames),
                }
            }
            fn max_inputs(&self) -> usize {
                match self {
                    $( Op::$IV(v) => v.max_inputs(), )*
                    $( Op::$BV(v) => v.max_inputs(), )*
                    Op::Dynamic(v) => v.max_inputs(),
                }
            }
            fn process_slots(
                &mut self,
                ctx: &ProcessCtx,
                slots: &[Vec<f32>],
                indices: &[usize],
                outs: &mut [Vec<f32>],
                frames: usize,
            ) {
                match self {
                    $( Op::$IV(v) => v.process_slots(ctx, slots, indices, outs, frames), )*
                    $( Op::$BV(v) => v.process_slots(ctx, slots, indices, outs, frames), )*
                    Op::Dynamic(v) => v.process_slots(ctx, slots, indices, outs, frames),
                }
            }
            fn has_side_effects(&self) -> bool {
                match self {
                    $( Op::$IV(v) => v.has_side_effects(), )*
                    $( Op::$BV(v) => v.has_side_effects(), )*
                    Op::Dynamic(v) => v.has_side_effects(),
                }
            }
            fn is_active(&self) -> bool {
                match self {
                    $( Op::$IV(v) => v.is_active(), )*
                    $( Op::$BV(v) => v.is_active(), )*
                    Op::Dynamic(v) => v.is_active(),
                }
            }
            fn is_envelope(&self) -> bool {
                match self {
                    $( Op::$IV(v) => v.is_envelope(), )*
                    $( Op::$BV(v) => v.is_envelope(), )*
                    Op::Dynamic(v) => v.is_envelope(),
                }
            }
            fn has_active_envelope(&self) -> bool {
                match self {
                    $( Op::$IV(v) => v.has_active_envelope(), )*
                    $( Op::$BV(v) => v.has_active_envelope(), )*
                    Op::Dynamic(v) => v.has_active_envelope(),
                }
            }
            fn carry_state(&mut self, old: &mut dyn Opcode) {
                match self {
                    $( Op::$IV(v) => v.carry_state(old), )*
                    $( Op::$BV(v) => v.carry_state(old), )*
                    Op::Dynamic(v) => v.carry_state(old),
                }
            }
            fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
                match self {
                    $( Op::$IV(v) => v.set_control(key, start, target, frames, elapsed), )*
                    $( Op::$BV(v) => v.set_control(key, start, target, frames, elapsed), )*
                    Op::Dynamic(v) => v.set_control(key, start, target, frames, elapsed),
                }
            }
            fn input_is_init(&self, index: usize) -> bool {
                match self {
                    $( Op::$IV(v) => v.input_is_init(index), )*
                    $( Op::$BV(v) => v.input_is_init(index), )*
                    Op::Dynamic(v) => v.input_is_init(index),
                }
            }
            fn latency(&self) -> usize {
                match self {
                    $( Op::$IV(v) => v.latency(), )*
                    $( Op::$BV(v) => v.latency(), )*
                    Op::Dynamic(v) => v.latency(),
                }
            }
            fn kernel_stats(&self) -> Option<crate::opcode::KernelStats> {
                match self {
                    $( Op::$IV(v) => v.kernel_stats(), )*
                    $( Op::$BV(v) => v.kernel_stats(), )*
                    Op::Dynamic(v) => v.kernel_stats(),
                }
            }
            fn step_warnings(&self, step: f32) -> Vec<String> {
                match self {
                    $( Op::$IV(v) => v.step_warnings(step), )*
                    $( Op::$BV(v) => v.step_warnings(step), )*
                    Op::Dynamic(v) => v.step_warnings(step),
                }
            }
            fn control_warnings(&self, sample_rate: f32) -> Vec<String> {
                match self {
                    $( Op::$IV(v) => v.control_warnings(sample_rate), )*
                    $( Op::$BV(v) => v.control_warnings(sample_rate), )*
                    Op::Dynamic(v) => v.control_warnings(sample_rate),
                }
            }
        }
    };
}

op_enum! {
    inline {
        Add(ops::Add),
        Sub(ops::Sub),
        Mul(ops::Mul),
        Div(ops::Div),
        Rem(ops::Rem),
        Gain(ops::Gain),
        Min(ops::Min),
        Max(ops::Max),
        Pow(ops::Pow),
        Compare(ops::Compare),
        Select(ops::Select),
        Fold(ops::Fold),
        Clip(ops::Clip),
        Func(ops::Func),
        Hold(ops::Hold),
        BlockHold(ops::BlockHold),
        Delay1(ops::Delay1),
        NoteStart(ops::NoteStart),
        FinishWhen(ops::FinishWhen),
        ReadContext(ops::ReadContext),
        Released(ops::Released),
        SampleRate(ops::SampleRate),
        NoteTime(ops::NoteTime),
        Bitcrush(ops::Bitcrush),
        Compressor(ops::Compressor),
        Waveshape(ops::Waveshape),
        TableRead(ops::TableRead),
        SampleHold(ops::SampleHold),
        Impulse(ops::Impulse),
        DcBlock(ops::DcBlock),
        Distortion(ops::Distortion),
    }
    boxed {
        Osc(ops::Osc),
        Noise(ops::Noise),
        Env(ops::Env),
        Svf(ops::Svf),
        Delay(ops::Delay),
        Reverb(ops::Reverb),
        Chorus(ops::Chorus),
        Custom(ops::Custom),
        Control(ops::Control),
        PanChannel(ops::PanChannel),
        TapOut(ops::TapOut),
        ResetOn(ops::ResetOn),
        Spawn(ops::Spawn),
        Oversample(ops::Oversample),
        SampleDelay(ops::SampleDelay),
        ReportOut(ops::ReportOut),
        StreamOut(ops::StreamOut),
        Sampler(ops::Sampler),
        KitSampler(ops::KitSampler),
        BufferRead(ops::BufferRead),
        BufferWrite(ops::BufferWrite),
        BufferRecord(ops::BufferRecord),
        BufferReadBank(ops::BufferReadBank),
        ArrayRead(ops::ArrayRead),
        Lag(ops::Lag),
        Comb(ops::Comb),
        Allpass(ops::Allpass),
        Eq(ops::Eq),
        Limiter(ops::Limiter),
        Gate(ops::Gate),
        Follow(ops::Follow),
        Phaser(ops::Phaser),
        PitchShift(ops::PitchShift),
        Formant(ops::Formant),
        Resonator(ops::Resonator),
        Convolve(ops::Convolve),
        Fft(ops::Fft),
        Ifft(ops::Ifft),
        SpectralFreeze(ops::SpectralFreeze),
        SpectralDelay(ops::SpectralDelay),
    }
}
