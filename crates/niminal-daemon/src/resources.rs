//! Buffer resource work stays on the server thread. Audio changes use logged, timed events.
use crate::engine_server::{ClientId, INVALID_PARAMS};
use crate::{At, EngineError, EngineRequest, Op, Session};
use serde_json::{Value, json};

fn bad(message: &str) -> EngineError {
    EngineError::new(INVALID_PARAMS, message)
}
fn buffer(
    session: &Session,
    name: &str,
) -> Result<std::sync::Arc<niminal_engine::Buffer>, EngineError> {
    session
        .program()
        .buffers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b.clone())
        .ok_or_else(|| bad(&format!("no buffer `{name}`")))
}
fn allocate(name: &str, frames: usize) -> Result<Op, EngineError> {
    if name.is_empty()
        || !name
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    {
        return Err(bad("a buffer name is an identifier"));
    }
    if !(1..=1 << 22).contains(&frames) {
        return Err(bad("a buffer has from 1 to 4194304 frames"));
    }
    Ok(Op::Define {
        source: format!("buffer {name} = zeros({frames})"),
        at: At::Now,
        dir: None,
        tag: None,
    })
}

pub(crate) fn request(
    session: &mut Session,
    client: ClientId,
    request: EngineRequest,
) -> Result<Value, EngineError> {
    match request {
        EngineRequest::BuffersList {} => Ok(
            json!({ "buffers": session.program().buffers.iter().map(|(name, b)| {
            json!({ "name": name, "frames": b.len(), "sample_rate": b.sample_rate_or(session.sample_rate()) })
        }).collect::<Vec<_>>() }),
        ),
        EngineRequest::BuffersRead {
            name,
            offset,
            count,
        } => {
            let b = buffer(session, &name)?;
            if count > 65536 || offset > b.len() || count > b.len() - offset {
                return Err(bad("read at most 65536 frames within the buffer"));
            }
            Ok(
                json!({ "name": name, "offset": offset, "values": (offset..offset + count).map(|i| b.read(i)).collect::<Vec<_>>() }),
            )
        }
        EngineRequest::BuffersAlloc { name, frames } => {
            if session.program().buffers.iter().any(|(n, _)| *n == name) {
                return Err(bad("buffer already exists; redefine it to change its size"));
            }
            session
                .send_events(client, vec![allocate(&name, frames)?])
                .map_err(|p| EngineError::rejected(&p))?;
            Ok(json!({ "name": name, "frames": frames }))
        }
        EngineRequest::BuffersLoad {
            name,
            path,
            channel,
        } => {
            let channels = session.sample_files().load(&path).map_err(|e| bad(&e))?;
            let table = channel
                .checked_sub(1)
                .and_then(|c| channels.get(c))
                .ok_or_else(|| bad("channel does not exist in the file"))?;
            let source_rate = table.sample_rate_or(session.sample_rate());
            let frames = (table.len() as f64 * f64::from(session.sample_rate())
                / f64::from(source_rate))
            .round() as usize;
            if !(1..=1 << 22).contains(&frames) {
                return Err(bad("loaded buffer has from 1 to 4194304 frames"));
            }
            let table = table.resample(frames);
            let mut ops = Vec::new();
            if let Ok(b) = buffer(session, &name) {
                if b.len() != table.len() {
                    return Err(bad(
                        "file and buffer lengths differ; allocate a matching buffer first",
                    ));
                }
            } else {
                ops.push(allocate(&name, table.len())?);
            }
            for offset in (0..table.len()).step_by(65536) {
                let values = (offset..(offset + 65536).min(table.len()))
                    .map(|i| table.get(i))
                    .collect();
                ops.push(Op::BufferWrite {
                    name: name.clone(),
                    at: At::Now,
                    offset,
                    values,
                    tag: None,
                });
            }
            session
                .send_events(client, ops)
                .map_err(|p| EngineError::rejected(&p))?;
            Ok(
                json!({ "name": name, "frames": table.len(), "source_sample_rate": source_rate, "sample_rate": session.sample_rate() }),
            )
        }
        EngineRequest::BuffersSave { name, path } => {
            let b = buffer(session, &name)?;
            let table = b.snapshot();
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: b.sample_rate_or(session.sample_rate()).round() as u32,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer =
                hound::WavWriter::create(&path, spec).map_err(|e| bad(&e.to_string()))?;
            for i in 0..table.len() {
                writer
                    .write_sample(table.get(i))
                    .map_err(|e| bad(&e.to_string()))?;
            }
            writer.finalize().map_err(|e| bad(&e.to_string()))?;
            Ok(json!({ "name": name, "path": path, "frames": table.len() }))
        }
        _ => unreachable!("buffer request dispatch"),
    }
}
