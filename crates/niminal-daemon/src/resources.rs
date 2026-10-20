//! Buffer resource work stays on the server thread. Audio changes use logged, timed events.
use crate::engine_server::{ClientId, INVALID_PARAMS};
use crate::session::definitions::{local, scoped};
use crate::{At, EngineError, EngineRequest, Op, Session};
use serde_json::{Value, json};

fn bad(message: &str) -> EngineError {
    EngineError::new(INVALID_PARAMS, message)
}
fn buffer(
    session: &Session,
    client: ClientId,
    name: &str,
) -> Result<std::sync::Arc<niminal_engine::Buffer>, EngineError> {
    session
        .program()
        .buffers
        .iter()
        .find(|(n, _)| *n == scoped(client, name) && !session.buffer_is_hidden(n))
        .map(|(_, b)| b.clone())
        .ok_or_else(|| bad(&format!("no buffer `{name}`")))
}
fn allocate(name: &str, frames: usize, channels: usize) -> Result<Op, EngineError> {
    if name.is_empty()
        || !name
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    {
        return Err(bad("a buffer name is an identifier"));
    }
    if !(1..=16).contains(&channels)
        || frames == 0
        || frames
            .checked_mul(channels)
            .and_then(|n| n.checked_mul(4))
            .is_none()
    {
        return Err(bad(
            "buffer allocation requires positive frames, 1–16 channels and a representable size",
        ));
    }
    Ok(Op::Define {
        source: format!("buffer {name} = zeros({frames}, channels: {channels})"),
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
        EngineRequest::BuffersBudget { bytes } => {
            if let Some(bytes) = bytes {
                session
                    .set_buffer_budget(client, bytes)
                    .map_err(|e| bad(&e))?;
            }
            let (bytes, retained) = session.buffer_budget(client);
            Ok(json!({"bytes":bytes,"retained_bytes":retained}))
        }
        EngineRequest::BuffersList {} => Ok(
            json!({ "buffers": session.program().buffers.iter().filter_map(|(name, b)| {
            if session.buffer_is_hidden(name) {return None;}
            let name = local(client,name)?;
            Some(json!({ "name": name, "frames": b.len(), "channels":b.channels(), "sample_rate": b.sample_rate_or(session.sample_rate()) }))
        }).collect::<Vec<_>>() }),
        ),
        EngineRequest::BuffersRead {
            name,
            offset,
            count,
        } => {
            let b = buffer(session, client, &name)?;
            if count.saturating_mul(b.channels()) > 65536
                || offset > b.len()
                || count > b.len() - offset
            {
                return Err(bad(
                    "read at most 65536 interleaved samples within the buffer",
                ));
            }
            Ok(
                json!({ "name": name, "offset": offset, "channels":b.channels(), "values": (offset*b.channels()..(offset+count)*b.channels()).map(|i| b.read_sample(i)).collect::<Vec<_>>() }),
            )
        }
        EngineRequest::BuffersAlloc {
            name,
            frames,
            channels,
        } => {
            if session
                .program()
                .buffers
                .iter()
                .any(|(n, _)| *n == scoped(client, &name) && !session.buffer_is_hidden(n))
            {
                return Err(bad("buffer already exists; redefine it to change its size"));
            }
            session
                .send_events(client, vec![allocate(&name, frames, channels)?])
                .map_err(|p| EngineError::rejected(&p))?;
            session.publish_buffer(&scoped(client, &name));
            Ok(json!({ "name": name, "frames": frames, "channels":channels }))
        }
        EngineRequest::BuffersLoad {
            name,
            path,
            channel,
        } => {
            let resolved = session.sample_files_for(client).resolve_path(&path);
            let header = hound::WavReader::open(&resolved).map_err(|e| bad(&e.to_string()))?;
            let width = if channel == 0 {
                header.spec().channels as usize
            } else {
                1
            };
            let frames = (header.duration() as f64 * f64::from(session.sample_rate())
                / f64::from(header.spec().sample_rate))
            .round() as usize;
            let (budget, retained) = session.buffer_budget(client);
            let additional = if buffer(session, client, &name).is_ok() {
                0
            } else {
                frames.saturating_mul(width).saturating_mul(4)
            };
            if additional > budget.saturating_sub(retained) {
                return Err(bad("buffer import exceeds client memory budget"));
            }
            let loaded = session
                .sample_files_for(client)
                .load(&path)
                .map_err(|e| bad(&e))?;
            let tables: Vec<_> = if channel == 0 {
                loaded.iter().collect()
            } else {
                vec![
                    loaded
                        .get(channel - 1)
                        .ok_or_else(|| bad("channel does not exist"))?,
                ]
            };
            let width = tables.len();
            if !(1..=16).contains(&width) {
                return Err(bad("load 1–16 channels"));
            }
            let source_rate = tables[0].sample_rate_or(session.sample_rate());
            let frames = (tables[0].len() as f64 * f64::from(session.sample_rate())
                / f64::from(source_rate))
            .round() as usize;
            let tables: Vec<_> = tables.iter().map(|t| t.resample(frames)).collect();
            let mut ops = Vec::new();
            if let Ok(b) = buffer(session, client, &name) {
                if b.len() != frames || b.channels() != width {
                    return Err(bad(
                        "buffer shape differs; free it before loading replacement",
                    ));
                }
            } else {
                ops.push(allocate(&name, frames, width)?);
            }
            let chunk = 65536 / width;
            for offset in (0..frames).step_by(chunk) {
                let mut values = Vec::new();
                for frame in offset..(offset + chunk).min(frames) {
                    for table in &tables {
                        values.push(table.get(frame));
                    }
                }
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
            session.publish_buffer(&scoped(client, &name));
            Ok(
                json!({"name":name,"frames":frames,"channels":width,"source_sample_rate":source_rate,"sample_rate":session.sample_rate()}),
            )
        }
        EngineRequest::BuffersFree { name } => {
            buffer(session, client, &name)?;
            session.hide_buffer(scoped(client, &name));
            Ok(json!({}))
        }
        EngineRequest::BuffersSave { name, path } => {
            let b = buffer(session, client, &name)?;

            let spec = hound::WavSpec {
                channels: b.channels() as u16,
                sample_rate: b.sample_rate_or(session.sample_rate()).round() as u32,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer =
                hound::WavWriter::create(&path, spec).map_err(|e| bad(&e.to_string()))?;
            for i in 0..b.len() * b.channels() {
                writer
                    .write_sample(b.read_sample(i))
                    .map_err(|e| bad(&e.to_string()))?;
            }
            writer.finalize().map_err(|e| bad(&e.to_string()))?;
            Ok(json!({ "name": name, "path": path, "frames": b.len(), "channels":b.channels() }))
        }
        _ => unreachable!("buffer request dispatch"),
    }
}
