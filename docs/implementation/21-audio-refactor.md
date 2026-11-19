# 21 — The refactor of the audio subsystem

This document opens the refactor requested after #40. It exists because the #43 defect — the
voices in Turma da Mônica — **is still open**, and the investigation proved that the cause is not a
line of code: it is the **structure** of the subsystem.

## What is already ruled out, with numbers

| link | measurement | verdict |
|---|---|---|
| decoding | 122 sounds, rate, channels and duration equal to the files in the `pakz` (checked with `ffprobe`) | correct |
| mixer | `step 1.0000` on every voice (a 44,100 sound in a 44,100 mixer) | correct |
| delivery to the frontend | **98–101% of 44,100 frames per real second**, with the frontend accepting 100% | correct |
| RetroArch | input 44,100 (`Set audio input rate to: 44100.00 Hz`), CC resampler, output at 48,000 | correct |

No link in the path doubles the speed — and the symptom exists. That is only possible because the
structure allows **measuring one path and playing another**.

## What the structure allows (the three-legged 5-why, published in #43)

1. **The mixer is wired in three places** (`--dump-audio`, the interface's output and the core)
   and **the rate is decided in two** (44,100 in the core, the device's in the output). Four
   measurements in a row lied because the measured path was not the played path: a run without
   `--dump-audio` **had no mixer at all**, and reported "zero sounds decoded" with 123 sounds
   requested.
2. **The boundary with the device had no instrument at all**: nobody counted frames delivered
   against accepted, nor recorded the device's rate.
3. **The log was a 300-line ring dumped at the end of the function**: the game talks a lot at boot,
   the audio lines were overwritten, and an abrupt close of the window lost the rest. Through
   libretro it works, because the core hands the lines to the frontend every frame; on standalone,
   it does not.
4. **The BREW contract is not honored in full.** The SDK says that **only one `ISoundPlayer_Play`
   is active per object, and a new `Play` on the same object is refused**
   (`AEESoundPlayer.H:1224-1226`, with "You need to issue a stop before you can call another");
   our mixer **replaces** the voice. The decisions about buffer ownership and the exact meaning of
   `MM_PARM_PLAY_REPEAT` are also missing.

## The design

1. **A single owner for audio**: an `AudioHost` that owns the output, the canonical rate and the
   log. The frontends stop building the mixer: they ask for the host and hand it the output.
2. **An instrumented boundary**: frames produced, frames accepted and the device's rate, always
   visible and always in a file. The instrumentation no longer depends on where the mixer was born.
3. **The BREW contract as the SDK describes it**: a voice per object, `Play` refused when a sound is
   already playing, buffer ownership respected, end of sound by the **virtual clock** and not by
   the mixer.
4. **Decoders validated against the source**: rate, channels and duration checked per sound, and
   the voice's step computed from them.
5. **The log with file output**, written as the lines are born, to survive an abrupt close of the
   window.
6. **Tests that pin the contract**: `Play` refused, voice duration equal to the sound's, mono
   against stereo, the decompression bomb, and a fake consumer that measures the boundary in any
   frontend.

## Acceptance criteria

- the #43 symptom stops reproducing, with **before and after** evidence in the same session;
- the boundary measured in any frontend, with one number per second in the log;
- the three CI commands green, and each new test failing **before** the corresponding fix.
