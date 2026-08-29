# ff-analysis

Analyzes the content of media and extracts information usable for editing and organizing decisions (scene boundaries, silent segments, waveforms, and the like).

## Purpose

To edit and organize sources, you first need to know content features such as "where the scene changes," "where it is silent," and "where the highlights are." Finding these by eye and by hand is laborious and prone to oversights. ff-analysis **automatically extracts such analysis information** from sources, so that tools and users can make accurate editing decisions based on it.

## What it solves

- **Detecting scene boundaries** — automatically find the positions where scenes switch, usable for cut candidates and chapter structuring.
- **Detecting silent segments** — identify ranges where sound is absent, usable for removing unneeded parts and automatic splitting.
- **Grasping black frames and keyframes** — surface nearly pitch-black segments and positions suited to seeking.
- **Visualizing video and audio distributions** — extract distributions of brightness, color, and volume, usable as material for quality checks and color adjustment.
- **Analysis that does not rely on guesswork** — obtain results as numbers based on the actual content, with failures returned in a form whose cause is clear.

## Capabilities

- Detection of scene boundaries (cuts)
- Detection of silent segments
- Detection of nearly black frames
- Enumeration of keyframe positions
- Extraction of brightness and RGB histograms
- Extraction of audio amplitude waveforms (peak / RMS)
- Video scopes (waveform, vectorscope, RGB parade, histogram)
- Result representation of tempo (BPM)

## Out of scope

- Editing, processing, or converting sources (only reading and reporting)
- Reading in (decoding) or writing out (encoding) sources themselves
- The editing model such as timeline, editing, and history
- How the analysis results are used (decisions such as applying cuts are delegated to the caller)
