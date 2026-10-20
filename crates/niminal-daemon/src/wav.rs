//! Streaming IEEE-float WAV, promoted to RF64 when the payload exceeds 4 GiB.
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
};
pub(crate) struct Wav {
    file: std::io::BufWriter<File>,
    channels: usize,
    frames: u64,
}
impl Wav {
    pub fn create(path: &Path, channels: usize, rate: u32) -> Result<Self, String> {
        let mut file = File::create(path).map_err(|e| e.to_string())?;
        let mut header = Vec::new();
        header.extend(b"RIFF");
        header.extend(0u32.to_le_bytes());
        header.extend(b"WAVEJUNK");
        header.extend(28u32.to_le_bytes());
        header.extend([0; 28]);
        header.extend(b"fmt ");
        header.extend(16u32.to_le_bytes());
        header.extend(3u16.to_le_bytes());
        header.extend((channels as u16).to_le_bytes());
        header.extend(rate.to_le_bytes());
        header.extend((rate * channels as u32 * 4).to_le_bytes());
        header.extend((channels as u16 * 4).to_le_bytes());
        header.extend(32u16.to_le_bytes());
        header.extend(b"data");
        header.extend(0u32.to_le_bytes());
        file.write_all(&header).map_err(|e| e.to_string())?;
        Ok(Self {
            file: std::io::BufWriter::with_capacity(65536, file),
            channels,
            frames: 0,
        })
    }
    pub fn write(&mut self, values: &[f32]) -> Result<(), String> {
        if values.len() % self.channels != 0 {
            return Err("partial audio frame".into());
        }
        let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.frames += (values.len() / self.channels) as u64;
        Ok(())
    }
    pub fn finish(mut self, frames: Option<u64>) -> Result<(), String> {
        self.finalize(frames)
    }
    fn finalize(&mut self, frames: Option<u64>) -> Result<(), String> {
        let frames = frames.unwrap_or(self.frames).min(self.frames);
        self.frames = frames;
        let size = frames * self.channels as u64 * 4;
        let riff = size + 72;
        self.file.flush().map_err(|e| e.to_string())?;
        self.file
            .get_ref()
            .set_len(size + 80)
            .map_err(|e| e.to_string())?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        self.file
            .write_all(if riff > u32::MAX as u64 {
                b"RF64"
            } else {
                b"RIFF"
            })
            .map_err(|e| e.to_string())?;
        self.file
            .write_all(&(riff.min(u32::MAX as u64) as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        if riff > u32::MAX as u64 {
            self.file
                .seek(SeekFrom::Start(12))
                .map_err(|e| e.to_string())?;
            self.file.write_all(b"ds64").map_err(|e| e.to_string())?;
            self.file
                .seek(SeekFrom::Start(20))
                .map_err(|e| e.to_string())?;
            for n in [riff, size, frames] {
                self.file
                    .write_all(&n.to_le_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }
        self.file
            .seek(SeekFrom::Start(76))
            .map_err(|e| e.to_string())?;
        self.file
            .write_all(
                &(if riff > u32::MAX as u64 {
                    u32::MAX
                } else {
                    size as u32
                })
                .to_le_bytes(),
            )
            .map_err(|e| e.to_string())?;
        self.file.flush().map_err(|e| e.to_string())
    }
}

impl Drop for Wav {
    fn drop(&mut self) {
        let _ = self.finalize(None);
    }
}
