<CsoundSynthesizer>
<CsOptions>
-odac -+rtmidi=coremidi -M0
</CsOptions>

<CsInstruments>
sr = 48000
ksmps = 32
nchnls = 2
0dbfs = 1

instr 1
    iFreq cpsmidi
    iAmp ampmidi 0.3

    aEnv madsr 0.01, 0.1, 0.7, 0.3
    aSig vco2 iAmp, iFreq

    outs aSig * aEnv, aSig * aEnv
endin

; Route MIDI channel 1 to instrument 1
massign 1, 1

</CsInstruments>

<CsScore>
f 0 z
</CsScore>
</CsoundSynthesizer>