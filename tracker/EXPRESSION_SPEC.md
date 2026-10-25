# Tracker expression

Status: proposed. This specifies new behavior, not features already available.

## Purpose

Make expressive note editing possible directly in the pattern: combine sample
offsets and retriggers, add pitch or volume movement, vary repeated phrases, and
apply swing without manually adjusting every note.

Keep the existing distinction between note settings and parameter automation.
Note settings and commands belong to a note column and describe what one row
does; lanes and envelopes address named parameters and describe change over
time. The engine owns DSP and sample-timed modulation; the tracker owns musical
timing, command interpretation and event scheduling.

Two principles keep the grid easy to reason about:

- **No hidden state.** Every command acts within its own row. A blank cell does
  nothing and nothing carries forward from earlier rows.
- **No command conflicts by construction.** Each kind of command has its own
  lane, so a cell cannot hold two commands that compete.

## Grid layout

Each note column has these optional sub-columns:

```text
Note  Ins  Vol  Pan  Dly  Start  Chance  Pitch  Rtg
C-4   01   80   C    0    50     100     U12    4:-25
```

- **Start** (per-note trigger settings): see below. Hidden initially.
- **Chance**: see Chance. Hidden initially.
- **Pitch**: one pitch command per row (`G`, `U`, `D`, and later `A`, `B`).
- **Rtg**: the retrigger command (`R`), written without its letter.
- **Gain** (added with stage 4): the tremolo command (`T`).

Pitch and Rtg are the successors of the single Cmd sub-column. The Cmd
sub-column is removed. Start, Pitch and Rtg follow the existing sub-column
visibility conventions: a track's header menu shows or hides each, and the
choice is shared across that track's note columns and across patterns.

Commands affect only their own note column. A chord's voices can therefore have
different commands. Returns and the master have no note sub-columns.

Hiding a populated sub-column keeps its behavior and shows an indicator on the
track header, with a tooltip naming the hidden content. There is no action that
removes a sub-column's content silently; clearing is always an explicit edit.

## Editing

- Arrow keys and Tab navigate cells using the existing grid conventions.
- Typing a letter or number starts direct text entry. Enter commits and advances
  by Step; Escape cancels.
- Enter on an idle command cell, or double-click, opens a small editor. An empty
  cell starts with a searchable picker of what the lane accepts. A populated
  cell opens its settings, including a way to change command type.
- The editor uses named fields and units, shows a compact text preview, and has
  Apply and Cancel. Apply is one undo operation and returns focus to the cell.
  Enter applies unless the picker is selecting; Escape cancels.
- Every populated command cell also shows, on hover and in the editor, a
  plain-language reading such as "4 hits, each 25% quieter", "up 12 semitones"
  or "glide over 1 row". The raw text is the editing form; the reading is how
  users learn it.
- Invalid input remains in the editor with an inline explanation. The previously
  committed value continues to play. No partial edit reaches the scheduler.
- Record mode gates mutations just as it does elsewhere in the grid. Inspecting
  settings remains possible with record mode off.
- Selection, clear, copy, cut, paste, duplication, row insertion/deletion,
  reverse, and undo include the new cells. Numeric randomization must not mutate
  command syntax or choose random command types.

Copy/paste uses tab-separated cells. Cell text is canonical and decimal, with no
hexadecimal values. A paste that contains malformed or conflicting commands is
rejected as one transaction with an explanation. Paste retains existing clipping
at pattern bounds. When nonempty cells would be clipped it asks for confirmation
before applying, as one undo operation.

## Start

A per-note trigger setting, like Vol, Pan and Dly. Text forms:

| Text | Meaning |
| --- | --- |
| `50` | Start the sample 50% in (0–99). |
| `<` | Play the sample backwards. |
| `50<` | Both. |

Start configures a new trigger on its row. On a row without a new note it does
nothing. Each retrigger hit uses the same start and direction. Unsupported
instrument controls produce a visible warning and skip the setting, rather than
blocking unrelated notes.

## Commands

Commands are a letter followed by decimal numbers. These are niminal syntax, not
Renoise command codes. Pitch commands live in the Pitch lane, retrigger in Rtg,
tremolo in Gain; each lane holds one command per row per note column.

| Lane | Text | Editor fields | Meaning |
| --- | --- | --- | --- |
| Rtg | `4` | Hits, 2–32; change per hit, −100 to +100% | Retrigger. |
| Rtg | `4:-25` | Same | Four hits; each successive hit has 75% of the previous hit's gain. Positive changes are clamped so no hit exceeds the first hit's gain. |
| Pitch | `G8` | Duration, in eighths of a row | Existing glide to the row's note. May span several rows (`G16` is two). |
| Pitch | `U12`, `D3` | Semitones | Existing upward/downward slide over the row. |
| Pitch | `A4,7:1` | Two offsets, −24 to +24 semitones; step length, 1–32 eighths of a row | Arpeggio: cycle original pitch, first offset, second offset. |
| Pitch | `B2:25` | Cycles per row, 0.25–16; depth, 0–200 cents | Vibrato: sine pitch movement around the base pitch. |
| Gain | `T1:50` | Cycles per row, 0.25–16; depth, 0–100% | Tremolo: sine gain movement from full gain to the chosen attenuation. |

Retrigger was written `R4` in the old Cmd sub-column; Rtg drops the letter
because the lane already says what it is. All values are decimal, as the
existing commands already are.

For example, `A4,7:1` cycles C/E/G when applied to C, changing pitch every
eighth of a row. `B2:25` completes two vibrato cycles per row at ±25 cents.
`T1:50` cycles between full gain and half gain once per row. Vibrato starts at
zero pitch displacement, rising; tremolo starts at full gain; arpeggio starts at
the original pitch. Phases start at the row's note start. Movement from
arpeggio, vibrato and tremolo returns to neutral at the row boundary, so repeat
the command on later rows to continue it. Slow movement across many rows
belongs to automation lanes and envelopes, not commands.

Arpeggio is picked from presets (major, minor, octave and so on) that fill in the
fields; custom offsets use the editor fields.

Pitch commands on a row without a new note target its column's held voice.
Retrigger without a new note uses the held voice's instrument, pitch and sample
start settings. With no held voice it does nothing. OFF and CUT retain their
existing release/stop behavior and reject commands on the same row.

## Combining commands

Resolve each row into these independent properties:

1. Determine whether the note triggers, using Chance.
2. Apply trigger delay and Start settings.
3. Establish base pitch movement: normal note, glide, or slide.
4. Add arpeggio offsets and vibrato to that base pitch in semitone space.
5. Expand retriggers and apply their gain changes; tremolo multiplies voice gain.

Each retrigger starts with the same start and direction, and chokes the
previous hit, so a retrigger group is one sounding voice at a time and
modulation addresses that group. Slides, arpeggio, vibrato and tremolo follow
the original row timeline, not each hit's own start, and do not reset at each
retrigger. Retriggers are evenly spaced within the interval from the delayed
note start to the end of its row. The final hit holds according to existing
note/OFF rules. Existing behavior for an unmodified retrigger must be covered by
regression tests.

Because each lane holds one command, a row already has at most one pitch command
and one retrigger. The only conflict is **glide with retrigger**: retaining one
voice and restarting it have competing meanings. The editor explains the
conflict and offers to replace the existing command; replacement requires an
explicit choice and is one undo operation. Every other combination above is
valid. File loading and backend validation reject the conflicting pair. Never
silently choose one.

## Chance

Accept whole percentages from 0 to 100. Blank means 100%; values do not carry
forward. Chance applies only to a pitched note on that row. Reject nonempty
Chance on an empty, OFF or CUT row in both UI and file validation.

A failed roll skips the new note and all its commands. It does not terminate
the previously held note or change the column's remembered instrument. A passed
roll applies to the whole retrigger group, not separately to each hit. Note-hold
lookahead must account for skipped notes; it cannot stop a voice at an event
that will not actually trigger.

Rolls are unseeded, as in Renoise's Yxx: each play-through draws fresh random
numbers, so every loop pass and every transport restart can differ. Chance
exists only on pitched notes, so rows with only a held-voice command (such as a
bare retrigger) are never probabilistic. The scheduler rolls each occurrence
once and remembers the outcome until that row has played, so note-hold lookahead
and the event itself agree.

Full export draws once for the whole export, and every stem is rendered in that
same pass so the mix and stems share their decisions. A rendered file is the
only way to keep a particular set of outcomes; baking chance into the pattern
is outside this scope.

## Swing

Add a song **Swing** setting, 0–75%, default 0. Patterns can inherit it or use an
override. An explicit pattern 0 means straight timing, even if the song swings.
Swing is a simple alternating-row control; arbitrary groove templates and
per-track swing are outside this scope.

For each pair of rows, first calculate its unswung duration using the existing
tempo integration. If the two durations are a and b, transfer
`min(a, b) * swing / 100` from the second row to the first. This preserves the
pair's total duration, including during tempo ramps. With equal straight rows,
50% produces a 3:1 duration ratio. Label this as swing amount, not as a
percentage position within the pair.

Pairs begin at row zero of every pattern. An unmatched final row is unchanged.
Use one authoritative musical-time mapping for note starts/holds, note delays,
retrigger positions, command phases, envelopes, tempo-synced modulation,
playhead display and export. Fractional row positions map proportionally within
the swung row. Tempo ramps retain their existing row-space definition; swing
redistributes their integrated row durations.

The timing panel explains that swing acts on rows: LPB determines the musical
subdivision being swung. Manual note delays are a fraction of the row, applied
within the resulting swung row, and remain intact when swing is turned off. A
matrix alias plays with the swing of its destination sequence position. Live
changes affect only events beyond the existing scheduling lookahead, following
current timing-edit rules.

## Persistence and implementation boundaries

Replace the single `cmd` array with parallel per-column arrays, one per
sub-column: `start`, `chance`, `pitch`, `retrigger` and `gain`. Each holds one
nullable entry per row. `pitch`, `retrigger` and `gain` entries are small tagged
unions with numeric fields, not unparsed strings or an extensible command
framework; `start` is a nullable object with an optional percentage and a
reverse flag; `chance` is a nullable integer. Use one parser/formatter per
lane at the text boundary. Keep layout settings on the track, and timing
overrides with pattern timing.

Bump the project version and local autosave key when implementing this. Older
formats are rejected with a clear message; no migration shim is required.
Validate ranges, array dimensions, conflicts and finite numeric values on both
the TypeScript and Rust boundaries.

Extend the existing scheduler rather than introducing another sequencer.
Per-voice modulation must address a retrigger group as one unit; settle this
engine primitive before starting stage 4. Resolve probability before
voice-duration lookahead. Schedule pitch/gain modulation through native engine
controls; never generate modulation using UI timers or change a shared track
parameter to imitate per-note expression. Avoid approximating vibrato or
tremolo with dense client-generated events. Add an engine primitive only if
existing voice controls cannot express it.

## Delivery order

1. Start, Pitch (`G`, `U`, `D`) and Rtg sub-columns, editor, parser/formatter,
   plain-language readings and the glide/retrigger rule. Deliver start plus
   retrigger as the first complete workflow.
2. Chance and retrigger dynamics, including shared decisions across export stems.
3. Swing and its shared timing map.
4. Arpeggio, vibrato and tremolo (and the Gain lane) using native per-voice
   modulation. Start only after stages 1–3 have been used for real songs.

Each stage is usable independently. Do not expose unfinished commands in the
picker. Update the tracker manual as each stage ships.

## Acceptance criteria

- Start plus retrigger renders each hit with the same start and direction.
- Chord voices can have different commands without changing each other.
- Hidden sub-columns still play; hiding never deletes content.
- Every editor action and paste undoes atomically.
- Invalid input and the glide/retrigger conflict are caught by UI and backend
  validation.
- Retrigger gain progression matches its multiplicative definition, including
  −100% silencing later hits and the instrument's existing velocity response.
- Probability preserves held voices across failed triggers and varies between
  loop passes and transport restarts.
- Export mix and stems use identical probability decisions.
- Chord columns roll chance independently.
- Retrigger plus tremolo or vibrato keeps one continuous phase across hits.
- Arpeggio, vibrato and tremolo return to neutral at the row boundary with no
  jump, and rows without a command are unaffected.
- At constant tempo, 0% swing preserves timing and 50% gives a 3:1 row ratio;
  tempo ramps preserve pair durations, and odd final rows remain unchanged.
- Delays, releases, automation, commands and export agree at swung boundaries,
  including pattern changes and matrix aliases.
- Native modulation affects only the intended voice, cleans up at the row end,
  and behaves identically in live playback and offline rendering.
- Existing offset, reverse, retrigger, glide and slide behavior and
  block-editing workflows remain covered by meaningful scheduler/audio and grid
  tests.

For implementation changes, run `scripts/check.sh --test`, tracker frontend
tests/build and tracker Rust checks/tests. Then install the normal release
engine with `cargo install --path crates/niminal-cli`, build/install the tracker
for dogfooding, and audition the examples above. This document alone requires
no binary rebuild.
