<div align="center">
<h1>Clips</h1>

Polish your videos.

<img src="data/resources/icons/hicolor/scalable/apps/io.gitlab.adhami3310.Clips.svg" width="128" height="128" alt="Clips icon">

This fork is based on [Footage](https://gitlab.com/adhami3310/Footage).

</div>


## Installation

Clips is currently installed from source. Flathub and distribution packages
under the name Footage provide the upstream application, not this fork.


## About

Trim, flip, rotate and crop individual clips. Clips is a useful tool for quickly editing short videos and screencasts. It's also capable of exporting any video into a format of your choice. See [Press](PRESS.md) for coverage of the upstream project.

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
ninja -C _build install
```

The desktop file is installed at
`~/.local/share/applications/io.gitlab.adhami3310.Clips.desktop` and the binary
at `~/.local/bin/clips`.

## Credits

Actively developed by Khaleel Al-Adhami.

Logo desgined by kramo.

Huge thanks to all of the translators who brought Footage to many other languages!
