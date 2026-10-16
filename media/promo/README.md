# MineShare desk story

Original editable 18-second workflow illustration for the repository README.
Created with the official `remotion-dev/skills` Remotion guidance. Concept:
a tired developer arrives, sets down a Windows laptop at the center of a desk,
MineShare opens, and one cursor moves from the powered desktop PC to the laptop.
Keyboard, mouse and headset cables stay attached to the PC throughout.

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
npx remotion render DeskStory ../../docs/media/desk-story.mp4 --codec=h264 --crf=20
npx remotion still DeskStory ../../docs/media/desk-story-poster.png --frame=315
ffmpeg -i ../../docs/media/desk-story.mp4 -vf "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=160[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" -loop 0 ../../docs/media/desk-story.gif
```

The GIF is silent by design and loops with a fade; the MP4 is the clearer version.
README image alt text describes the action without requiring autoplay.

## Cinematic companion

A separate, conceptual 15-second realistic desk scene was submitted to HeyGen
Video 1 (video ID `42af5dd131bc430fa7711449dad049a0`). It is not embedded here
until the completed clip can be reviewed for cable continuity, hands, app identity,
and the single-cursor handoff. Do not substitute AI footage for real acceptance
testing or assume its availability from this source file alone.
