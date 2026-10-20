# Dogfood

```sh
nimlive render examples/synth_drums.nml --out synth_drums.wav
nimlive render examples/saw_lead.nml --out saw_lead.wav
niminal daemon examples/groove.nml
# then: capture last 4bars as jam
```
