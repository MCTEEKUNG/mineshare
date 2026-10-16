# MineShare desk story

Original editable 18-second workflow illustration for the repository README.
Created with the official `remotion-dev/skills` Remotion guidance. Concept:
a round mascot arrives, sets down a Windows laptop at the center of a desk,
MineShare opens, and one cursor moves from the powered desktop PC to the laptop.
Keyboard, mouse and headset cables stay attached to the PC throughout.

The current character is a single mint-colored round mascot with stubby arms,
blush cheeks and tiny feet, not a human in clothing. It enters from the bottom
center at a fixed X of 640 in the 1280-wide design canvas, walking straight away
from the viewer toward the desk with small alternating steps. There is no
sideways entrance or layer-switch teleport. Ubuntu and Windows platform marks replace the
device-name labels above the displays and track their positions during the zoom.

Ubuntu Circle of Friends artwork: [Canonical asset](https://assets.ubuntu.com/v1/ce518a18-CoF-2022_solid+O.svg).
The Windows four-pane identification mark is authored as simple vector geometry.
Ubuntu and Windows marks belong to their respective trademark owners; their use
identifies the illustrated platforms, not endorsement. These third-party marks
are not covered by the original illustration's MIT license claim below.

This is an illustration, not a live UI recording, timing benchmark, or compatibility
acceptance test. No personal desktop footage or user clipboard content is included.
The logo is the existing MineShare icon. Source and original illustration assets
follow the repository's MIT license; Remotion has its own software licensing terms.

## Preview and edit

```sh
cd media/promo
npm ci
npm run dev
```

Select **DeskStory**. Edit `src/Composition.tsx`; all motion is derived from the
current video frame, not browser-time CSS animation. No paid rendering service,
API keys, fonts, or external stock assets are needed to reproduce the illustration.

## Export

```sh
npx remotion render DeskStory ../../docs/media/desk-story.mp4 --codec=h264 --crf=14 --image-format=png
npx remotion still DeskStory ../../docs/media/desk-story-poster.png --frame=315
ffmpeg -i ../../docs/media/desk-story.mp4 -vf "fps=30,split[a][b];[a]palettegen=max_colors=256[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" -loop 0 ../../docs/media/desk-story-mascot-7f1ae438.gif
```

The scene renders from vector artwork at 1920×1080, without upscaling the old GIF.
A 1.32× camera push-in from 6.35s highlights the PC-to-laptop cursor handoff;
the camera eases back by 11.6s while the headline and footer remain stationary.
The GIF uses the full 256-color palette with no patterned dithering to keep
letter edges clean; subtle gradients can still exhibit GIF color banding.
The GIF retains all 540 frames from the 30 fps, 18-second source, rather than
dropping frames to 12 fps. GIF timestamps use centiseconds, so 30 fps is represented
by alternating 30/40 ms delays. The GIF is silent and loops with a fade; MP4 is
still preferable for precise playback timing, clarity and smaller file size.
README image alt text describes the action without requiring autoplay.
The published GIF uses a content-versioned filename to avoid stale GitHub/browser
image caches. When replacing its content, update the filename and README link too.

## Cinematic companion

A separate, conceptual 15-second realistic desk scene was submitted to HeyGen
Video 1 (video ID `42af5dd131bc430fa7711449dad049a0`). It is not embedded here
until the completed clip can be reviewed for cable continuity, hands, app identity,
and the single-cursor handoff. Do not substitute AI footage for real acceptance
testing or assume its availability from this source file alone.
