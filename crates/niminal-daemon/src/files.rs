//! Bounded file transfer over the existing JSON transport.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};
const CHUNK: usize = 256 * 1024;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn decode(text: &str) -> Result<Vec<u8>, String> {
    if text.len() > CHUNK.div_ceil(3) * 4 || !text.len().is_multiple_of(4) {
        return Err("base64 chunk exceeds 256 KiB or has invalid length".into());
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for (i, chunk) in text.as_bytes().chunks(4).enumerate() {
        let mut n = 0u32;
        let mut padding = 0;
        for (j, &b) in chunk.iter().enumerate() {
            let v = if b == b'=' {
                if j < 2 || i + 1 != text.len() / 4 {
                    return Err("invalid base64 padding".into());
                }
                padding += 1;
                0
            } else {
                if padding > 0 {
                    return Err("invalid base64 padding".into());
                }
                ALPHABET
                    .iter()
                    .position(|c| *c == b)
                    .ok_or("invalid base64")? as u32
            };
            n = (n << 6) | v;
        }
        if padding > 2 || (padding == 1 && n & 255 != 0) || (padding == 2 && n & 65535 != 0) {
            return Err("invalid base64 padding bits".into());
        }
        out.push((n >> 16) as u8);
        if padding < 2 {
            out.push((n >> 8) as u8);
        }
        if padding == 0 {
            out.push(n as u8);
        }
    }
    if out.len() > CHUNK {
        return Err("chunk exceeds 256 KiB".into());
    }
    Ok(out)
}
struct Entry {
    path: PathBuf,
    ready: bool,
    temporary: bool,
}
#[derive(Default)]
pub(crate) struct Files {
    entries: BTreeMap<(u64, String), Entry>,
    next: u64,
}
impl Files {
    pub fn upload(
        &mut self,
        client: u64,
        id: Option<String>,
        offset: u64,
        data: &str,
        finish: bool,
    ) -> Result<Value, String> {
        let bytes = decode(data)?;
        let id = if let Some(id) = id {
            id
        } else {
            if offset != 0 || self.entries.keys().filter(|(c, _)| *c == client).count() >= 64 {
                return Err("start uploads at zero; at most 64 files per client".into());
            }
            self.next += 1;
            let id = format!("file:{}", self.next);
            let path = std::env::temp_dir().join(format!(
                "niminal-{}-{}-{}.wav",
                std::process::id(),
                self.next,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            self.entries.insert(
                (client, id.clone()),
                Entry {
                    path,
                    ready: false,
                    temporary: true,
                },
            );
            id
        };
        let entry = self
            .entries
            .get_mut(&(client, id.clone()))
            .ok_or("no such upload")?;
        if entry.ready {
            return Err("upload already finished".into());
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&entry.path)
            .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() != offset {
            return Err("upload offset must equal received bytes".into());
        }
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        entry.ready = finish;
        Ok(json!({"id":id,"bytes":offset+bytes.len()as u64,"ready":finish}))
    }
    pub fn path(&self, client: u64, id: &str) -> Result<PathBuf, String> {
        self.entries
            .get(&(client, id.to_string()))
            .filter(|e| e.ready)
            .map(|e| e.path.clone())
            .ok_or_else(|| "no ready file with that handle".into())
    }
    pub fn read(&self, client: u64, id: &str, offset: u64, count: usize) -> Result<Value, String> {
        if count > CHUNK {
            return Err("read at most 256 KiB".into());
        }
        let mut file = File::open(self.path(client, id)?).map_err(|e| e.to_string())?;
        let total = file.metadata().map_err(|e| e.to_string())?.len();
        if offset > total {
            return Err("offset is past EOF".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; count.min((total - offset) as usize)];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        Ok(
            json!({"id":id,"offset":offset,"data":encode(&bytes),"bytes":bytes.len(),"total":total,"eof":offset+bytes.len()as u64==total}),
        )
    }
    pub fn register(&mut self, client: u64, path: PathBuf, temporary: bool) -> String {
        self.next += 1;
        let id = format!("file:{}", self.next);
        self.entries.insert(
            (client, id.clone()),
            Entry {
                path,
                ready: true,
                temporary,
            },
        );
        id
    }
    pub fn remove(&mut self, client: u64, id: &str) -> Result<(), String> {
        let entry = self
            .entries
            .remove(&(client, id.to_string()))
            .ok_or("no file with that handle")?;
        if entry.temporary {
            let _ = std::fs::remove_file(entry.path);
        }
        Ok(())
    }
    pub fn disconnect(&mut self, client: u64) {
        let ids: Vec<_> = self
            .entries
            .keys()
            .filter(|(c, _)| *c == client)
            .map(|(_, id)| id.clone())
            .collect();
        for id in ids {
            let _ = self.remove(client, &id);
        }
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            if entry.temporary {
                let _ = std::fs::remove_file(&entry.path);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_roundtrip_and_invalid_input() {
        for n in 0..260 {
            let bytes: Vec<_> = (0..n).map(|i| i as u8).collect();
            assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
        }
        for s in ["a", "A===", "AB==", "AAA=AAAA", "!!!!"] {
            assert!(decode(s).is_err(), "{s}");
        }
    }
}
