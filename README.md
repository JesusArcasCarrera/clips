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

## Contributing
Issues and merge requests are more than welcome. However, please take the following into consideration:

- This project follows the [GNOME Code of Conduct](https://wiki.gnome.org/Foundation/CodeOfConduct)
- Only Flatpak is supported

## Development

### GNOME Builder
The recommended method is to use GNOME Builder:

1. Install [GNOME Builder](https://apps.gnome.org/app/org.gnome.Builder/) from Flathub
1. Open Builder and select "Clone Repository..."
1. Open this checkout in Builder.
1. Press "Run Project" (▶) at the top, or `Ctrl`+`Shift`+`[Spacebar]`.

### Flatpak
You can install Clips from the latest commit:

1. Install [`org.flatpak.Builder`](https://github.com/flathub/org.flatpak.Builder) from Flathub
1. Open a terminal in the repository root.
1. Run `flatpak run org.flatpak.Builder --install --user --force-clean build-dir flatpak/io.gitlab.adhami3310.Clips.json`.

### Meson
You can build and install on your host system by directly using the Meson buildsystem:

1. Install `blueprint-compiler` and other relevant codecs.
1. Run the following commands (with `/usr` prefix):
```
meson --prefix=/usr build
ninja -C build
sudo ninja -C build install
```

### Local user install

This fork ships as **Clips** throughout: its binary is `clips`, its application
ID is `io.gitlab.adhami3310.Clips`, and its data and gettext domains are
`clips`.

To build and install into `~/.local` so the launcher shows it as *Clips*:

```
meson setup _build --prefix="$HOME/.local"
meson compile -C _build
cargo test
meson install -C _build
```

The desktop file is installed at
`~/.local/share/applications/io.gitlab.adhami3310.Clips.desktop` and the binary
at `~/.local/bin/clips`.

## Credits

Actively developed by Khaleel Al-Adhami.

Logo desgined by kramo.

Huge thanks to all of the translators who brought Footage to many other languages!
