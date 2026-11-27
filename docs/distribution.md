# Distributing NotTeams on Linux

Notes from 2026-11-27, checked against this code and the Flatpak runtimes installed on the
development machine (org.freedesktop.Platform 25.08 and 26.08).

## Decision

Flatpak on hwnhub (`hawwwran.github.io/hwnhub`), the self-hosted channel Easy Player and
Flatpal ship on, released with Project Manager. Done on 2026-11-27:

- `com.hawwwran.notteams.yaml`: the manifest, modelled on Easy Player's (cargo fetches
  crates during the build, so no vendoring; the llvm22 SDK extension supplies libclang for
  bindgen).
- `packaging/com.hawwwran.notteams.metainfo.xml`: AppStream data; each release adds its notes
  on top of `<releases>`.
- `.project-manager/release`: Easy Player's script with NotTeams' names. `release` bumps,
  tags, builds and signs into a clone of hwnhub, then pushes and makes the GitHub release;
  `hwnhub channel` publishes an existing tag again.
- Code: `desktop::in_flatpak()` skips the self-installed menu entry and registers the tray
  icon without its own bus name; the headset profile falls back to `pw-cli`, and PipeWire
  writes (`pw-cli`, `pw-metadata`) say `media.category = Manager`, since WirePlumber gives a
  Flatpak client read-only access otherwise; the ringback falls back to a generated tone;
  H.264 comes from Cisco's downloaded binary (below).

Flathub itself would need vendored crates (`flatpak-cargo-generator.py`), screenshots in
the metainfo, and a review of `--device=all`, `--filesystem=home:ro` and of an unofficial
Teams client; hwnhub needs none of that.

## What the Flatpak runtime already has

NotTeams starts these programs; the Freedesktop runtime ships them in `/usr/bin`:

| Program | Used for | In runtime |
|---|---|---|
| `pw-play`, `paplay` | sounds (`sounds.rs`) | yes |
| `pw-dump`, `pw-metadata`, `pw-cli` | listing, picking and switching audio devices (`devices.rs`) | yes |
| `gst-launch-1.0` + `pipewiresrc` | screen sharing (`screen.rs`), fed by the ScreenCast portal (`ashpd`) | yes |
| `gdbus` | taskbar unread badge (`notify.rs`), desktop idle time (`activity.rs`) | yes |
| `bzip2` | unpacking Cisco's codec download (`codec.rs`) | yes |
| `canberra-gtk-play` | "calling..." ringback (`sounds.rs`) | no: NotTeams plays its own two-tone ring instead |
| `wpctl` | switching a Bluetooth headset to its call profile (`devices.rs`) | no: falls back to `pw-cli set-param <id> Profile` |
| `update-desktop-database` | the self-installed menu entry (`desktop.rs`) | not needed in a Flatpak |

## H.264 and its patent

Cisco pays the H.264 patent royalties only for its own OpenH264 binary, downloaded from
Cisco by each user's machine. NotTeams doesn't compile OpenH264 in (`openh264` crate with
`libloading` instead of `source`). `src/codec.rs` downloads
`libopenh264-2.7.0-<platform>.8.so.bz2` from ciscobinary.openh264.org on first start into
the data folder (`~/.local/share/notteams/openh264`, or under `~/.var/app/` in the
Flatpak); the crate checks its hash against Cisco's builds before loading it, on every
start. Settings → Calls can turn it off, which removes the folder; calls then go on without
video, and the call bar's camera button says why. About and CREDITS show "OpenH264 Video
Codec provided by Cisco Systems, Inc." and link Cisco's binary licence.

## Permissions (finish-args)

- `--socket=x11 --socket=wayland`: X11 first, as Easy Player does, because winit only
  delivers dropped files on X11; `--device=all` covers the GPU too
- `--filesystem=home:ro`: files dropped on the window arrive as plain paths (the attach
  button goes through the file chooser portal instead)
- `--filesystem=xdg-download`: saved attachments
- `--socket=pulseaudio`, `--filesystem=xdg-run/pipewire-0`: sound, calls, device lists
- `--device=all`: cameras (`v4l` opens `/dev/video*` directly)
- `--share=network`, `--share=ipc`
- `--talk-name=org.kde.StatusNotifierWatcher`, `org.freedesktop.Notifications`,
  `com.canonical.Unity`, `org.gnome.Mutter.IdleMonitor`: tray, notifications, badge, idle
- The settings and cache folders (`dirs`) move into `~/.var/app/com.hawwwran.notteams/`
  by themselves. Moving from a self-built copy means signing in once more.

## Not yet tried inside the sandbox

Camera, screen sharing, the tray, sounds, device switching and drag and drop. A local
`flatpak-builder` build succeeded on 2026-11-27, before the later permission changes.

One open question for Flathub specifically: NotTeams talks to Microsoft's services as an
unofficial client, which Flathub may question. hwnhub, or a `.flatpak` bundle file shared
directly, avoids that review.

## Store listing

The listing comes from `packaging/com.hawwwran.notteams.metainfo.xml`, built into each
release, so a change shows in GNOME Software only after the next release. Its screenshots
are fetched from `packaging/screenshots/` on GitHub's `main`. `packaging/screenshots/take.sh`
retakes them from the demo mode (`NOTTEAMS_DEMO=1`, `src/demo.rs`): made-up people and
chats, no network (not even the codec download), and settings, cache and data in a scratch
folder of its own, so no real account appears in them.

The `.flatpakref` file on the hwnhub page carries the app's id, title, channel address and
signing key, and (since hwnhub 7c7a43c, from each app's next publish) its summary,
description, homepage and icon, taken from the metainfo built into the release.
Screenshots, licence and age rating come from GNOME Software's local copy of the channel's
catalog, which Flatpak refreshes about once a day; until then a newly published app's page
lacks them, and before hwnhub 7c7a43c it was bare (NotTeams' first release).
`flatpak update --appstream <remote>` refreshes the copy at once (hwnhub's remote may be
named after the app it was first installed with), then GNOME Software needs a restart
(`gnome-software --quit`).
