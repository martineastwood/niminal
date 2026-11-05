# Niminal original starter recordings

Twelve original synthesized recordings: eight drum hits and four tuned instruments.
No recordings, waveforms or other assets from third-party sample packs are used.
The recordings and their source patches are distributed under the MIT license in LICENSE.

All WAVs are mono, 48 kHz, 16-bit PCM, rooted at c3 (130.81279 Hz). Each starts at its
attack, is recorded with a one-second held note plus up to four seconds of release,
then trimmed and faded over its final 10 ms. Most peaks are adjusted toward -6 dBFS, with at most 18 dB of boost. The
sub-heavy 808 is set to -1.94 dBFS peak so it remains audible beside the other hits. They are dry: no master limiter or effects are recorded.
The hat recordings share a choke group in starter.nml.

Normally the recordings can be used directly, without exporting:

    use std/samples.{drums, electric_piano}

Use drums as a clip's instrument, or drums.kick for one member. The other pitched
recordings are fm_keys, struck_string and metal_bar. They are embedded in Stage
and the CLI; no .niminal sample folder is created.

Export an editable copy from an installed niminal:

    niminal samples --out samples/starter

Use the kit in your piece (path relative to your file):

    use samples/starter/starter as sounds
    clip beat {
      instrument: sounds::drums
      notes: grid {
        kick:  x... x... x... x...
        snare: .... x... .... x...
        hat:   x.x. x.x. x.x. x.x.
      }
    }
    beat.start

The tuned recordings are also available, e.g. sounds::electric_piano. Their pitch
and playback duration can be changed using the ordinary sample controls.

Every recording has a matching standalone .nml source patch. Rebuild all recordings
from the current presets in a repository checkout with:

    cargo run -p niminal-lang --example audition -- --starter crates/niminal-lang/assets/starter

The builder applies the trimming, fade and level adjustment described above.
