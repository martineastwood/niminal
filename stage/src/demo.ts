// The stage's opening piece. Definitions only: it loads silently, then the scenes are yours to launch.
export const demoSource = `// niminal stage · click a scene to play, then evaluate changes below.
config { channels: stereo }
tempo 112bpm
meter 4/4

bus space: stereo
fx space_return {
  out = space.reverb(room: 0.82, damp: 0.35).gain(0.7)
}

instr pluck(freq: hz, bright: 0..1 = 0.5, az: deg = 0deg) {
  level = perc(decay: 340ms)
  voice = osc(saw, freq).lpf(cutoff: freq * (2 + bright * 8), res: 0.25) * level
  out = voice.pan(azimuth: az)
  space += out.gain(-18db)
}

instr pad(freq: hz, bright: 0..1 = 0.5) {
  level = env[0 40ms 0.65 1beat 0.35 | 600ms 0]
  voice = osc(saw, freq).lpf(cutoff: freq * (3 + bright * 4), res: 0.15) * level
  out = voice
}

// A clip is lanes sharing a timeline. One cycle is one bar.
clip bass {
  instrument: pluck(bright: 0.18)
  notes:  [c2 ~ c2 eb2 ~ g1 bb1 ~]
  gain:   [0db -6db -3db]
  bright: [0.18 0.32 0.5]
}

clip lead {
  instrument: pluck(bright: 0.8, az: 22deg)
  notes: [c4 e4 g4 b4].every(2, reverse)
  gain:  [0db -3db 0db -6db]
}

// A two-bar pad, thinned so the gaps are part of the riff.
clip shimmer {
  instrument: pad(bright: 0.55)
  length: 2bars
  notes: [c5 e5 g5 b5].drop(25%)
  gain:  [-14db]
}

// Derived clips keep the instrument and lanes, and add one change.
clip backwards = lead.reverse
clip ghost = shimmer.drop(50%)

scene verse  { bass, lead }
scene chorus { bass, lead, shimmer }
scene mirror { bass, backwards }
scene air    { ghost }
`;
