//! Reading SFZ multisample files: the text of the file into regions, with every opcode resolved
//! through the `<control>`, `<global>`, `<master>` and `<group>` headers above it.

use std::collections::HashMap;

use niminal_engine::ops::AmpEnv;

/// How a region treats its sample's end and the end of the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    /// Plays the sample through once; a released note fades it.
    NoLoop,
    /// Plays the sample through once and ignores the end of the note.
    OneShot,
    /// Repeats the loop for as long as the note lasts.
    Continuous,
}

/// One `<region>`, with the opcodes this player understands as numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct SfzRegion {
    /// The audio file, as the file wrote it (`default_path` applied, `\` turned to `/`).
    pub sample: String,
    pub keys: (u8, u8),
    pub vels: (u8, u8),
    pub rand: (f32, f32),
    /// `(seq_length, seq_position)` when the region takes turns with others.
    pub seq: Option<(u32, u32)>,
    /// The key at which the sample sounds as recorded; `None` for `pitch_keycenter=sample`, which
    /// takes the note the file itself names.
    pub keycenter: Option<u8>,
    /// Semitones, from `tune` (cents) and `transpose`.
    pub tune: f32,
    /// Cents of pitch per key, 100 for a pitched instrument and 0 for a drum.
    pub keytrack: f32,
    pub volume_db: f32,
    pub offset: usize,
    pub end: Option<usize>,
    /// `None` means the file's own loop, if it has one.
    pub loop_kind: Option<LoopKind>,
    pub loop_start: Option<usize>,
    pub loop_end: Option<usize>,
    pub env: AmpEnv,
    /// The choke group this region belongs to, and the group whose notes cut it.
    pub group: Option<i64>,
    pub off_by: Option<i64>,
}

/// A parsed file: its regions and what was left out.
#[derive(Debug, Default)]
pub struct Sfz {
    pub regions: Vec<SfzRegion>,
    /// How many regions this player can't play, such as release-triggered ones.
    pub skipped: usize,
}

type Opcodes = HashMap<String, String>;

/// Parse the text of an SFZ file. `include` reads the text of a `#include`d file.
pub fn parse(
    text: &str,
    include: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Sfz, String> {
    let mut defines: Vec<(String, String)> = Vec::new();
    let source = preprocess(text, include, &mut defines, 0)?;
    let (mut control, mut global, mut master, mut group) = (
        Opcodes::new(),
        Opcodes::new(),
        Opcodes::new(),
        Opcodes::new(),
    );
    let mut region: Option<Opcodes> = None;
    let mut regions: Vec<Opcodes> = Vec::new();
    // the header whose opcodes are being read
    let mut header = String::new();
    let flush = |region: &mut Option<Opcodes>, regions: &mut Vec<Opcodes>| {
        if let Some(r) = region.take() {
            regions.push(r);
        }
    };
    for (kind, body) in tokens(&source) {
        match kind {
            Token::Header => {
                flush(&mut region, &mut regions);
                header = body.to_ascii_lowercase();
                match header.as_str() {
                    "global" => {
                        global.clear();
                        master.clear();
                        group.clear();
                    }
                    "master" => {
                        master.clear();
                        group.clear();
                    }
                    "group" => group.clear(),
                    "region" => {
                        let mut merged = global.clone();
                        merged.extend(master.clone());
                        merged.extend(group.clone());
                        region = Some(merged);
                    }
                    _ => {}
                }
            }
            Token::Opcode => {
                let (name, value) = body.split_once('=').unwrap_or((body, ""));
                let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_string());
                let target = match header.as_str() {
                    "control" => &mut control,
                    "global" => &mut global,
                    "master" => &mut master,
                    "group" => &mut group,
                    "region" => match &mut region {
                        Some(r) => r,
                        None => continue,
                    },
                    _ => continue,
                };
                target.insert(name, value);
            }
        }
    }
    flush(&mut region, &mut regions);
    let default_path = control.get("default_path").cloned().unwrap_or_default();
    let mut sfz = Sfz::default();
    for (n, opcodes) in regions.iter().enumerate() {
        if opcodes.get("sample").is_none_or(String::is_empty) {
            continue;
        }
        match region_of(opcodes, &default_path, &control) {
            Ok(Some(region)) => sfz.regions.push(region),
            Ok(None) => sfz.skipped += 1,
            Err(e) => return Err(format!("region {}: {e}", n + 1)),
        }
    }
    if sfz.regions.is_empty() {
        return Err(if sfz.skipped > 0 {
            "none of its regions is one this player can play".into()
        } else {
            "it has no <region> with a sample".into()
        });
    }
    Ok(sfz)
}

#[derive(Clone, Copy)]
enum Token {
    Header,
    Opcode,
}

/// Cut the text into `<headers>` and `name=value` opcodes. A value ends where the next opcode
/// begins (so a sample path may have spaces in it), at the next header, or at the end of the line.
fn tokens(text: &str) -> Vec<(Token, &str)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('<') {
                let Some(end) = after.find('>') else { break };
                out.push((Token::Header, after[..end].trim()));
                rest = after[end + 1..].trim_start();
                continue;
            }
            // an opcode runs to the start of the next one, or the next header
            let Some(eq) = rest.find('=') else { break };
            let mut end = rest.len();
            let mut from = eq + 1;
            while let Some(at) = rest[from..].find(['=', '<']) {
                let at = from + at;
                if rest.as_bytes()[at] == b'<' {
                    end = at;
                    break;
                }
                // the name of the next opcode is the word just before this `=`
                let name_start = rest[..at]
                    .rfind(|c: char| c.is_whitespace())
                    .map_or(0, |i| i + 1);
                if name_start > eq {
                    end = name_start;
                    break;
                }
                from = at + 1;
            }
            out.push((Token::Opcode, rest[..end].trim_end()));
            rest = rest[end..].trim_start();
        }
    }
    out
}

/// Take out comments, follow `#include`, and replace `#define`d names.
fn preprocess(
    text: &str,
    include: &mut dyn FnMut(&str) -> Result<String, String>,
    defines: &mut Vec<(String, String)>,
    depth: usize,
) -> Result<String, String> {
    if depth > 8 {
        return Err("#include goes more than 8 files deep".into());
    }
    let mut without_blocks = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        without_blocks.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    without_blocks.push_str(rest);
    let mut out = String::new();
    for line in without_blocks.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if let Some(define) = line.strip_prefix("#define") {
            let mut parts = define.trim().splitn(2, char::is_whitespace);
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                defines.push((name.to_string(), value.trim().to_string()));
            }
        } else if let Some(file) = line.strip_prefix("#include") {
            let file = file.trim().trim_matches('"');
            let included = include(file)?;
            out.push_str(&preprocess(&included, include, defines, depth + 1)?);
            out.push('\n');
        } else {
            let mut line = line.to_string();
            // longest names first, so `$KEY_LO` isn't eaten by `$KEY`
            let mut names: Vec<_> = defines.iter().collect();
            names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
            for (name, value) in names {
                if line.contains(name.as_str()) {
                    line = line.replace(name.as_str(), value);
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// A MIDI note number from a number or a name: `60`, `c4`, `c#4`, `Db3`, `a-1`.
pub fn key_number(text: &str) -> Option<i32> {
    let text = text.trim();
    if let Ok(n) = text.parse::<i32>() {
        return Some(n);
    }
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_lowercase();
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut rest = chars.as_str();
    let shift = match rest.chars().next()? {
        '#' => 1,
        'b' => -1,
        _ => 0,
    };
    if shift != 0 {
        rest = &rest[1..];
    }
    let octave: i32 = rest.parse().ok()?;
    Some(12 * (octave + 1) + base + shift)
}

fn region_of(
    opcodes: &Opcodes,
    default_path: &str,
    control: &Opcodes,
) -> Result<Option<SfzRegion>, String> {
    let text = |name: &str| opcodes.get(name).map(String::as_str);
    let number = |names: &[&str]| -> Result<Option<f64>, String> {
        for name in names {
            if let Some(v) = text(name) {
                return v
                    .parse::<f64>()
                    .map(Some)
                    .map_err(|_| format!("`{name}={v}` should be a number"));
            }
        }
        Ok(None)
    };
    let key = |names: &[&str]| -> Result<Option<i32>, String> {
        for name in names {
            if let Some(v) = text(name) {
                return key_number(v)
                    .map(Some)
                    .ok_or_else(|| format!("`{name}={v}` should be a note number or name"));
            }
        }
        Ok(None)
    };

    let sample = text("sample").unwrap_or_default();
    // a silent region, written `sample=*silence`, or a generator, has nothing to play
    if sample.starts_with('*') {
        return Ok(None);
    }
    // release triggers play when a key comes up, and keyswitched regions only for the default switch
    let default_switch = text("sw_default")
        .or(control.get("sw_default").map(String::as_str))
        .and_then(key_number);
    if matches!(text("trigger"), Some("release" | "release_key"))
        || text("sw_last").is_some_and(|last| key_number(last) != default_switch)
    {
        return Ok(None);
    }
    if number(&["end"])?.is_some_and(|e| e < 0.0) {
        return Ok(None);
    }
    let shift = control
        .get("note_offset")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
        + 12 * control
            .get("octave_offset")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
    let clamp_key = |k: i32| (k + shift).clamp(0, 127) as u8;
    let single = key(&["key"])?;
    let lokey = clamp_key(single.or(key(&["lokey"])?).unwrap_or(0));
    let hikey = clamp_key(single.or(key(&["hikey"])?).unwrap_or(127));
    let keycenter = match text("pitch_keycenter") {
        Some("sample") => None,
        _ => Some(clamp_key(
            key(&["pitch_keycenter"])?.or(single).unwrap_or(60),
        )),
    };
    let vel = |name: &str, default: f64| -> Result<u8, String> {
        Ok(number(&[name])?
            .unwrap_or(default)
            .round()
            .clamp(0.0, 127.0) as u8)
    };
    let (lovel, hivel) = (vel("lovel", 0.0)?.max(1), vel("hivel", 127.0)?);
    let rand = (
        number(&["lorand"])?.unwrap_or(0.0) as f32,
        number(&["hirand"])?.unwrap_or(1.0) as f32,
    );
    let seq = match (number(&["seq_length"])?, number(&["seq_position"])?) {
        (Some(length), position) if length >= 2.0 => {
            let position = position.unwrap_or(1.0);
            if position < 1.0 || position > length {
                return Err(format!(
                    "`seq_position={position}` should be from 1 to `seq_length={length}`"
                ));
            }
            Some((length as u32, position as u32 - 1))
        }
        _ => None,
    };
    let loop_kind = match text("loop_mode").or(text("loopmode")) {
        None => None,
        Some("no_loop") => Some(LoopKind::NoLoop),
        Some("one_shot") => Some(LoopKind::OneShot),
        Some("loop_continuous" | "loop_sustain") => Some(LoopKind::Continuous),
        Some(other) => return Err(format!("`loop_mode={other}` isn't one this player knows")),
    };
    let frames = |names: &[&str]| -> Result<Option<usize>, String> {
        Ok(number(names)?.map(|v| v.max(0.0) as usize))
    };
    let percent = |name: &str, default: f64| -> Result<f32, String> {
        Ok((number(&[name])?.unwrap_or(default) / 100.0) as f32)
    };
    let seconds =
        |name: &str| -> Result<f32, String> { Ok(number(&[name])?.unwrap_or(0.0).max(0.0) as f32) };
    let group_of = |name: &str| -> Result<Option<i64>, String> {
        Ok(number(&[name])?.map(|v| v as i64).filter(|&g| g > 0))
    };
    let amplitude = number(&["amplitude"])?.unwrap_or(100.0) / 100.0;
    let path = format!("{default_path}{sample}").replace('\\', "/");
    Ok(Some(SfzRegion {
        sample: path,
        keys: (lokey.min(hikey), hikey.max(lokey)),
        vels: (lovel, hivel.max(lovel)),
        rand,
        seq,
        keycenter,
        tune: (number(&["tune", "pitch"])?.unwrap_or(0.0) / 100.0
            + number(&["transpose"])?.unwrap_or(0.0)) as f32,
        keytrack: number(&["pitch_keytrack"])?.unwrap_or(100.0) as f32 / 100.0,
        volume_db: (number(&["volume"])?.unwrap_or(0.0) + 20.0 * amplitude.max(1e-6).log10())
            as f32,
        offset: frames(&["offset"])?.unwrap_or(0),
        end: frames(&["end"])?,
        loop_kind,
        loop_start: frames(&["loop_start", "loopstart"])?,
        loop_end: frames(&["loop_end", "loopend"])?,
        env: AmpEnv {
            delay: seconds("ampeg_delay")?,
            attack: seconds("ampeg_attack")?,
            hold: seconds("ampeg_hold")?,
            decay: seconds("ampeg_decay")?,
            sustain: percent("ampeg_sustain", 100.0)?,
            release: seconds("ampeg_release")?,
        },
        group: group_of("group")?,
        off_by: group_of("off_by")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<Sfz, String> {
        parse(text, &mut |f| Err(format!("no file {f}")))
    }

    #[test]
    fn regions_inherit_from_the_headers_above_them() {
        let sfz = read(
            "<control> default_path=samples/\n\
             <global> ampeg_release=0.4 volume=-3\n\
             <group> lovel=1 hivel=63\n\
             <region> sample=a.wav key=c4\n\
             <region> sample=b.wav lokey=d4 hikey=e4 pitch_keycenter=e4 ampeg_release=0.1\n\
             <group> lovel=64 hivel=127\n\
             <region> sample=c.wav key=60\n",
        )
        .unwrap();
        assert_eq!(sfz.regions.len(), 3);
        let a = &sfz.regions[0];
        assert_eq!(a.sample, "samples/a.wav");
        assert_eq!((a.keys, a.keycenter, a.vels), ((60, 60), Some(60), (1, 63)));
        assert!((a.env.release - 0.4).abs() < 1e-6 && a.volume_db == -3.0);
        let b = &sfz.regions[1];
        assert_eq!((b.keys, b.keycenter), ((62, 64), Some(64)));
        assert!((b.env.release - 0.1).abs() < 1e-6);
        assert_eq!(sfz.regions[2].vels, (64, 127));
    }

    #[test]
    fn a_sample_path_may_have_spaces_and_backslashes() {
        let sfz = read(
            "<region> sample=Grand Piano\\C4 soft.wav lokey=60 hikey=60 // the middle\n\
             <region> sample=x.wav key=61\n",
        )
        .unwrap();
        assert_eq!(sfz.regions[0].sample, "Grand Piano/C4 soft.wav");
        assert_eq!(sfz.regions[0].keys, (60, 60));
    }

    #[test]
    fn defines_includes_and_comments() {
        let sfz = parse(
            "#define $LO 48\n/* all of\nthis */\n#include \"hi.sfz\"\n<region> sample=a.wav lokey=$LO hikey=$HI\n",
            &mut |f| {
                assert_eq!(f, "hi.sfz");
                Ok("#define $HI 72\n".into())
            },
        )
        .unwrap();
        assert_eq!(sfz.regions[0].keys, (48, 72));
    }

    #[test]
    fn keys_by_name_follow_the_convention_that_c4_is_60() {
        assert_eq!(key_number("c4"), Some(60));
        assert_eq!(key_number("C#4"), Some(61));
        assert_eq!(key_number("Db4"), Some(61));
        assert_eq!(key_number("a-1"), Some(9));
        assert_eq!(key_number("a4"), Some(69));
        assert_eq!(key_number("h4"), None);
    }

    #[test]
    fn release_triggers_and_keyswitches_are_left_out() {
        let sfz = read(
            "<region> sample=a.wav key=60\n\
             <region> sample=r.wav key=60 trigger=release\n\
             <region> sample=s.wav key=60 sw_last=36\n",
        )
        .unwrap();
        assert_eq!((sfz.regions.len(), sfz.skipped), (1, 2));
        let only = read("<region> sample=r.wav trigger=release");
        assert!(only.unwrap_err().contains("can play"));
    }

    #[test]
    fn choke_groups_are_read() {
        let sfz = read(
            "<group> group=2 off_by=3\n<region> sample=a.wav key=60\n\
             <group> group=3\n<region> sample=b.wav key=62\n<region> sample=c.wav key=64 group=0\n",
        )
        .unwrap();
        let groups: Vec<_> = sfz.regions.iter().map(|r| (r.group, r.off_by)).collect();
        assert_eq!(groups, [(Some(2), Some(3)), (Some(3), None), (None, None)]);
    }

    #[test]
    fn round_robin_loops_and_envelopes_are_read() {
        let sfz = read(
            "<region> sample=a.wav seq_length=3 seq_position=2 lorand=0.25 hirand=0.5\n\
             <region> sample=b.wav loop_mode=loop_continuous loop_start=100 loop_end=900 \
             ampeg_attack=0.01 ampeg_sustain=50 tune=-50 transpose=2 pitch_keytrack=0\n",
        )
        .unwrap();
        assert_eq!(sfz.regions[0].seq, Some((3, 1)));
        assert_eq!(sfz.regions[0].rand, (0.25, 0.5));
        let b = &sfz.regions[1];
        assert_eq!(b.loop_kind, Some(LoopKind::Continuous));
        assert_eq!((b.loop_start, b.loop_end), (Some(100), Some(900)));
        assert_eq!((b.env.attack, b.env.sustain), (0.01, 0.5));
        assert!((b.tune - 1.5).abs() < 1e-6 && b.keytrack == 0.0);
    }

    #[test]
    fn mistakes_name_the_opcode() {
        let e = read("<region> sample=a.wav lokey=banana").unwrap_err();
        assert!(e.contains("region 1") && e.contains("lokey=banana"), "{e}");
        assert!(
            read("<region> volume=3")
                .unwrap_err()
                .contains("no <region>")
        );
    }
}
