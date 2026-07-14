- Fixed the recording pill's waveform appearing flat/dead during dictation
  (issue #179, AC-7 #173): a Windows terminal diagnostic confirmed the audio
  pipeline was correct — the level poller emits real, speech-correlated RMS
  (peaking ~0.09 while speaking, ~0.00 in silence) — but that RMS range was
  too small for the pill's linear `level * HEIGHT` bar-height mapping to
  clear its minimum bar height, so every bar rendered as a flat line. A new
  pure `scaleLevelForDisplay` (`src/lib/waveform.ts`) applies a perceptual
  `sqrt(rms) * 2.5` gain so speech-level RMS now visibly fills most of the
  bar while silence stays at the floor.
