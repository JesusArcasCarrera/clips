<div align="center">
<h1>Clips</h1>

Polish your videos.

<img src="data/resources/icons/hicolor/scalable/apps/io.gitlab.adhami3310.Clips.svg" width="128" height="128" alt="Clips icon">

This fork is based on [Footage](https://gitlab.com/adhami3310/Footage).

</div>

> Personal fork of Footage, renamed **Clips** (binary `clips`, gettext domain
> `clips`; the app id `io.gitlab.adhami3310.Clips` keeps the upstream prefix).
> Not affiliated with the Footage author. No warranty, no support. Commits are atomic per
> feature so anything can be cherry-picked.

> **All modifications in this fork were made by LLM agents** (several
> models and coding agents) under my direction, on top of the upstream code.
>
> <sub>For that very reason I am not submitting them upstream: right now I
> don't have the time to review every change and send it the way it should be
> sent, and I don't want to add noise to the projects or their communities. I
> am only after tools that fit my own workflow better, and I leave them public
> here in case any of these changes inspires or helps someone else.</sub>

## What's different from Footage

- **Multi-section editing**: trim and export one or several sections of the
  same video, and **multi-source sequences** joining clips from several
  files with **audio normalised** across sources.
- **Colour and playback**: brightness, contrast, saturation, hue, gamma and
  sharpness previewed live through GStreamer (WebM sharpness preview forced
  through system-memory AYUV to avoid corrupted VP8/VP9 frames); slow
  motion, repeat and boomerang playback with synchronised audio.
- **FFmpeg renderer** replacing the fixed-bitrate GES export: container,
  video and audio codecs, frame rate, quality targets computed from
  resolution and frame rate, hardware encoders with software fallback,
  **intelligent stream-copy export** when no re-encoding is needed, staged
  progress, cancellation and temporary-file cleanup. Large-file export
  stalls and hardware-encoder availability fixed.
- **Subtitles**: embedded tracks probed asynchronously and a single-track
  import model (in progress).
- **UI**: responsive layout with `Adw.Breakpoint` and a sidebar toggle;
  sidebar organised into Sections, Video and Export pages; a quality combo
  with bitrate presets; post-export menu (Open, Show in Folder, Back to
  Editing, Finish) through desktop portals. Spanish translation.


## About

Clips is a focused editor for short videos and screencasts. It can:

- trim and export one or several sections;
- crop, rotate, flip, and resize video;
- adjust brightness, contrast, saturation, hue, gamma, and sharpness;
- create slow-motion, repeated, and boomerang playback;
- choose the container, video and audio codecs, frame rate, and output quality;
- use a supported hardware encoder and fall back to software automatically.

After rendering, the result can be opened, shown in its containing folder, or
sent back to the editor with the current settings intact. See [Press](PRESS.md)
for coverage of the upstream project.

## What's different from Footage

- **Multi-section editing**: trim and export one or several sections of the
  same video, and **multi-source sequences** joining clips from several
  files with **audio normalised** across sources.
- **Colour and playback**: brightness, contrast, saturation, hue, gamma and
  sharpness previewed live through GStreamer (WebM sharpness preview forced
  through system-memory AYUV to avoid corrupted VP8/VP9 frames); slow
  motion, repeat and boomerang playback with synchronised audio.
- **FFmpeg renderer** replacing the fixed-bitrate GES export: container,
  video and audio codecs, frame rate, quality targets computed from
  resolution and frame rate, hardware encoders with software fallback,
  **intelligent stream-copy export** when no re-encoding is needed, staged
  progress, cancellation and temporary-file cleanup. Large-file export
  stalls and hardware-encoder availability fixed.
- **Subtitles**: embedded tracks probed asynchronously and a single-track
  import model (in progress).
- **UI**: responsive layout with `Adw.Breakpoint` and a sidebar toggle;
  sidebar organised into Sections, Video and Export pages; a quality combo
  with bitrate presets; post-export menu (Open, Show in Folder, Back to
  Editing, Finish) through desktop portals. Spanish translation.


## Installation

Clips is currently installed from source. Flathub and distribution packages
under the name Footage provide the upstream application, not this fork.


## About

Clips is a focused editor for short videos and screencasts. It can:

- trim and export one or several sections;
- crop, rotate, flip, and resize video;
- adjust brightness, contrast, saturation, hue, gamma, and sharpness;
- create slow-motion, repeated, and boomerang playback;
- choose the container, video and audio codecs, frame rate, and output quality;
- use a supported hardware encoder and fall back to software automatically.

After rendering, the result can be opened, shown in its containing folder, or
sent back to the editor with the current settings intact. See [Press](PRESS.md)
for coverage of the upstream project.

<img src="data/resources/screenshots/0.png" alt="Main screen with a chosen ISO and one USB memory">

## Building and installing this fork

Clips is written in Rust (GTK4/libadwaita, GStreamer, GES) and built with
Meson. On Fedora:

```sh
sudo dnf install meson cargo rust blueprint-compiler gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-bad-free-devel \
    gstreamer1-plugins-good gstreamer1-plugins-ugly gstreamer1-libav ges-devel ffmpeg
meson setup _build --prefix="$HOME/.local"
meson compile -C _build
cargo test
meson install -C _build
clips
```

The binary is `clips`, the application id `io.gitlab.adhami3310.Clips` (the
upstream prefix is kept so schemas and resources keep working), and the
desktop file lands in `~/.local/share/applications/`. A Flatpak manifest for
local builds is in `flatpak/io.gitlab.adhami3310.Clips.json`; nothing is
published on Flathub, and the Flathub package named *Footage* is the upstream
application, not this fork.

## Credits and license

This is a downstream fork of **[Footage](https://gitlab.com/adhami3310/Footage)**. All the
credit for the application itself goes to its authors and contributors
(Khaleel Al-Adhami; logo by kramo; translators of Footage); this repository only adds the changes listed above. The upstream
project is the place to get the official application; nothing here is
published on Flathub or in any distribution.

The code inherits the upstream license, **GPL-3.0-or-later** (see `COPYING`). Original
copyright headers are preserved in every file; the fork's changes are in the
git history of this repository.

## Reporting issues

Report problems with this fork **here**, not upstream. If you can reproduce
the problem on the official build, report it there instead.
