# Tracker expression

Status: proposed. This specifies new behavior, not features already available.

## Purpose

Make expressive note editing possible directly in the pattern: combine sample
offsets and retriggers, add pitch or volume movement, vary repeated phrases, and
apply swing without manually adjusting every note.

Keep the existing distinction between note commands and parameter automation.
Commands belong to a note column; lanes and envelopes address named parameters.
The engine owns DSP and sample-timed modulation; the tracker owns musical timing,
command interpretation and event scheduling.

## Grid layout

Each note column has one to four command slots. Start with one; retain the current
default visibility of the Cmd sub-column. A track's header menu offers **Add
command slot** and shows or hides individual slots. Slot count and visibility are
shared across that track's note columns and across patterns, like existing note
sub-column layout settings.

```text
Note  Ins  Vol  Pan  Dly  Chance  Cmd 1  Cmd 2
C-4   01   80   C    0    100     O50    R4
```

Commands affect only their own note column. A chord's voices can therefore have
different commands. Returns and the master have no note command slots.

Hide empty slots freely. Hiding populated slots retains their behavior and shows
an indicator on the track header, with a tooltip naming the hidden content.
Removing a slot is allowed only when it is empty in every pattern, including
inactive rows retained after pattern shortening. No removal action silently
deletes commands.

Chance is a separate optional sub-column, hidden initially. Swing lives in timing
controls. Neither consumes a command slot or an automation lane.

## Editing

- Arrow keys and Tab navigate command cells using the existing grid conventions.
- Typing a letter or number starts direct text entry. Enter commits and advances
  by Step; Escape cancels. Existing compact commands remain accepted.
- Enter on an idle command cell, or double-click, opens a small command editor.
  An empty cell starts with a searchable command picker. A populated cell opens
  its settings, including a way to change command type.
- The editor uses named fields and units, shows a compact text preview, and has
  Apply and Cancel. Apply is one undo operation and returns focus to the cell.
  Enter applies unless the picker is selecting a command; Escape cancels.
- Invalid input remains in the editor with an inline explanation. The previously
  committed value continues to play. No partial edit reaches the scheduler.
- Record mode gates mutations just as it does elsewhere in the grid. Inspecting
  settings remains possible with record mode off.
- Selection, clear, copy, cut, paste, duplication, row insertion/deletion,
  reverse, and undo include the new cells. Numeric randomization must not mutate
  command syntax or choose random command types.

Copy/paste uses tab-separated cells. Command text is canonical and decimal, with
no hexadecimal values. A paste that contains malformed or conflicting commands
is rejected as one transaction with an explanation. Paste retains existing
clipping at pattern bounds. When nonempty cells would be clipped it asks for
confirmation before applying, as one undo operation.

## Command vocabulary

Preserve the meanings and ranges of O, V, R, G, U and D. Add the following
forms; these are niminal syntax, not Renoise command codes.

| Text | Editor fields | Meaning |
| --- | --- | --- |
| `O50` | Offset, 0–99% | Existing sample offset. |
| `V` | Reverse | Existing reverse playback. |
| `R4` | Hits, 2–32; change per hit, −100 to +100% | Existing retrigger count, with optional dynamics. |
| `R4:-25` | Same | Four hits; each successive hit has 75% of the previous hit's gain. Positive changes are clamped so no hit exceeds the first hit's gain. |
| `G8` | Duration, in eighths of a row | Existing glide to the row's note. |
| `U12`, `D3` | Semitones | Existing upward/downward slide over the row. |
| `A4,7:1` | Two offsets, −24 to +24 semitones; step length, 1–32 eighths of a row | Cycle original pitch, first offset, second offset. |
| `B4:25` | Period, 1–64 rows; depth, 0–200 cents | Vibrato: sine pitch movement around the base pitch. Depth 0 ends it. |
| `T1:50` | Period, 1–64 rows; depth, 0–100% | Tremolo: sine gain movement from full gain to the chosen attenuation. Depth 0 ends it. |

All command values are decimal, as the existing commands already are. `G`
keeps its current meaning: the glide lasts the given number of eighths of a row
and may therefore span several rows (`G16` is two rows).

For example, `A4,7:1` cycles C/E/G when applied to C, changing pitch every
eighth of a row. `B4:25` has a four-row cycle with ±25-cent depth. `T1:50`
cycles between full gain and half gain over one row. Vibrato starts at zero
pitch displacement, rising; tremolo starts at full gain. Arpeggio starts at the
original pitch. Phases reset when their command is issued.

Blank command cells do not repeat earlier commands implicitly. Retrigger,
arpeggio and slides act over their own row; arpeggio movement returns to
neutral at the row boundary, so repeat it on later rows to continue it.
Vibrato and tremolo persist, with continuous phase, until the column's next
note, OFF or CUT, the voice ending, or a later `B` or `T` of the same type in
that column (depth 0 ends it without replacing it). A new `B` or `T` restarts
the phase.

Pitch commands on a row without a new note target its column's held voice.
Retrigger without a new note uses the held voice's instrument, pitch and sample
start settings. With no held voice it does nothing. Offset and reverse configure
new triggers on their row; alone on an otherwise empty row they do nothing.
OFF and CUT retain their existing release/stop behavior and reject commands on
the same row. Unsupported instrument controls produce a visible warning and
skip that command, rather than blocking unrelated notes.

## Combining commands

Slots are presentation positions, not an execution pipeline. Swapping populated
slots must not change playback.

Resolve each row into these independent properties:

1. Determine whether the note triggers, using Chance.
2. Apply trigger delay and sample start/direction settings.
3. Establish base pitch movement: normal note, glide, or slide.
4. Add arpeggio offsets and vibrato to that base pitch in semitone space.
5. Expand retriggers and apply their gain changes; tremolo multiplies voice gain.

Each retrigger starts with the same offset and direction, and chokes the
previous hit, so a retrigger group is one sounding voice at a time and
modulation addresses that group. Slides, arpeggio, vibrato and tremolo follow
the original row timeline, not each hit's own start, and do not reset at each
retrigger.
Retriggers are evenly spaced within the interval from the delayed note start to
the end of its row. The final hit holds according to existing note/OFF rules.
Existing behavior for an unmodified R command must be covered by regression tests.

Allow at most one command of each type per row and note column. G, U and D are
mutually exclusive. Glide plus retrigger is also excluded initially: retaining
one voice and restarting it have competing meanings. The editor explains the
conflict and offers to replace the existing command; replacement requires an
explicit choice and is one undo operation. Other combinations above are valid.

File loading and backend validation reject conflicting command sets. Never
silently choose the leftmost or rightmost command.

## Chance

Accept whole percentages from 0 to 100. Blank means 100%; values do not carry
forward. Chance applies only to a pitched note on that row. Reject nonempty
Chance on an empty, OFF or CUT row in both UI and file validation.

A failed roll skips the new note and all its commands. It does not terminate
the previously held note or change the column's remembered instrument. A passed
roll applies to the whole retrigger group, not separately to each hit. Note-hold
lookahead must account for skipped notes; it cannot stop a voice at an event
that will not actually trigger.

Use a saved unsigned 32-bit song seed. Resolve each roll from that seed and the
logical playback occurrence: sequence position, loop iteration, pattern row,
track and note column. Tracks and note columns carry a stable saved id, so
adding, removing or reordering them does not change other columns' rolls.
Loop iteration counts passes of the canonical traversal from song start, so
starting inside a loop uses that position's canonical iteration number. Use a
fixed integer mixing algorithm (a 32-bit murmur3-style finalizer over the
occurrence fields), documented with test vectors and implemented identically
in TypeScript (`Math.imul`, `>>> 0`) and Rust wherever probability is evaluated.
Chance exists only on pitched notes, so rows with only a held-voice command
(such as a bare `R`) are never probabilistic.
Audio blocks, scheduler lookahead and UI timing must not affect the result.

Restarting the same playback range repeats its outcomes. Successive loop
iterations may differ. Starting partway through a song uses the same first-pass
results for that position as playing from the beginning. Pattern-only playback
uses a distinct stable occurrence namespace. Matrix aliases use the destination
sequence occurrence. Full export uses the first, non-looping traversal; every
stem shares exactly the mix's decisions. The UI offers **New chance seed** as
an undoable song setting.

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
within the resulting swung row, and remain intact when swing is turned off.
A matrix alias plays with the swing of its destination sequence position. Live changes affect only events
beyond the existing scheduling lookahead, following current timing-edit rules.

## Persistence and implementation boundaries

Replace the single command array with `commands: (Command | null)[][][]` per
track, indexed by note column, then slot, then row. Only the last slot can be
removed. Command is a small tagged union with numeric fields, not an
unparsed string or an extensible command framework. Use one parser/formatter at
the text boundary. Chance is a parallel nullable integer array. Keep layout
settings on the track, and timing overrides with pattern timing.

Bump the project version and local autosave key when implementing this. Older
formats are rejected with a clear message; no migration shim is required.
Validate ranges, array dimensions, conflicts and finite numeric values on both
the TypeScript and Rust boundaries.

Extend the existing scheduler rather than introducing another sequencer.
Per-voice modulation must address a retrigger group as one unit; settle this
engine primitive before starting stage 4.
Resolve probability before voice-duration lookahead. Schedule pitch/gain
modulation through native engine controls; never generate modulation using UI
timers or change a shared track parameter to imitate per-note expression.
Avoid approximating vibrato or tremolo with dense client-generated events. Add
an engine primitive only if existing voice controls cannot express it.

## Delivery order

1. Multiple slots, editor, parser/formatter, combination rules and existing
   commands. Deliver offset plus retrigger as the first complete workflow.
2. Chance and retrigger dynamics, including deterministic offline rendering.
3. Swing and its shared timing map.
4. Arpeggio, vibrato and tremolo using native per-voice modulation.

Each stage is usable independently. Do not expose unfinished commands in the
picker. Update the tracker manual as each stage ships.

## Acceptance criteria

- Offset plus retrigger produces the same rendered samples when slots swap.
- Chord voices can have different commands without changing each other.
- Hidden commands still play; populated slots cannot be removed accidentally.
- Every editor action, paste, slot addition and seed change undoes atomically.
- Invalid input and conflicts are caught by UI and backend validation.
- Retrigger gain progression matches its multiplicative definition, including
  −100% silencing later hits and the instrument's existing velocity response.
- Probability preserves held voices across failed triggers, varies by loop
  occurrence and reproduces across transport restarts and scheduler chunk sizes.
- Export mix and stems use identical probability decisions.
- Chord columns roll chance independently, and rolls are unchanged when other
  columns or tracks are added, removed or reordered.
- Retrigger plus tremolo or vibrato keeps one continuous phase across hits.
- Vibrato and tremolo continue across rows until replaced, ended or the voice
  ends, with no jump at row boundaries.
- At constant tempo, 0% swing preserves timing and 50% gives a 3:1 row ratio;
  tempo ramps preserve pair durations, and odd final rows remain unchanged.
- Delays, releases, automation, commands and export agree at swung boundaries,
  including pattern changes and matrix aliases.
- Native modulation affects only the intended voice, cleans up at its end
  (row for arpeggio, voice end or replacement for vibrato and tremolo), and behaves identically in live playback and offline rendering.
- Existing O/V/R/G/U/D behavior and block-editing workflows remain covered by
  meaningful scheduler/audio and grid tests.

For implementation changes, run `scripts/check.sh --test`, tracker frontend
tests/build and tracker Rust checks/tests. Then install the normal release
engine with `cargo install --path crates/niminal-cli`, build/install the tracker
for dogfooding, and audition the examples above. This document alone requires
no binary rebuild.
