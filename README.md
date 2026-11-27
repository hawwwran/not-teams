# NotTeams

An unofficial native desktop client for Microsoft Teams chats, written in Rust. It talks to the
same Microsoft cloud services as the Teams web app at teams.cloud.microsoft, so chats and
messages are the same ones you see there. It is not made, endorsed or supported by Microsoft.

## Install

From hwnhub, a small self-hosted Flatpak channel:

```sh
flatpak install --user https://hawwwran.github.io/hwnhub/com.hawwwran.notteams.flatpakref
```

That adds the channel on the first install; `flatpak update` and GNOME Software bring new
versions after that.

## Build it yourself

Build it with `cargo build --release`, then run `target/release/notteams`.

On its first start it adds itself to your app menu (for your user only, no admin rights
needed), which also gives its window the NotTeams icon in the taskbar. To also install the
program under `~/.local/bin`, run `packaging/install.sh` after building.

## Sign in

1. Press **Continue**. The app shows a short code and copies it.
2. Press **Open sign-in page**, paste the code on Microsoft's page and sign in as usual.
3. The app notices on its own and opens your chats. It remembers you until you choose
   **Sign out** from the round picture at the top right.

## What works so far

- Your chats, with your own "(You)" chat at the top, kept up to date every few seconds.
- Reading and sending text messages (Enter sends, Shift+Enter starts a new line).
- Pictures and files: shown in chats, opened with your own apps, sent with the paperclip or
  by dropping them onto the window.
- Reactions: shown under messages; hover a message to add one.
- Presence: your status shows on Teams while NotTeams runs (Away after 5 minutes without
  keyboard or mouse, like Teams), photos and status dots for everyone, and setting your own
  status from your picture's menu.
- Tray: closing the window keeps NotTeams running in the tray; click the tray icon (or start
  the app again) to bring it back, and quit from the tray icon's menu.
- Settings (cog in the top bar): close to tray, "Always available", and how long downloaded
  pictures and files are kept, with clearing them.
- Calls: the Call and Video buttons at the top of a one-to-one chat ring that person; the
  camera button in the call bar turns your camera on or off, with a self view at the bottom
  right; the other side's video shows above it. Choose the microphone, speaker and camera in
  Settings → Calls, or during a call with the cog in the call bar (switches at once). Settings →
  Calls → "Make a test call" calls Teams' Echo service, which plays a message, records you for
  a few seconds and plays it back. Uses the default microphone and speaker; mute and hang up
  from the bar under the title bar. Incoming calls ring with a card at the top right
  (Accept / Decline) while NotTeams runs, also from the tray; answered calls use
  phone-quality audio (G.711) for now.

Building needs the ALSA and Opus development packages on Linux (`libasound2-dev`,
`libopus-dev`, `pkg-config`).

## Where it keeps things

- Sign-in and settings: `~/.config/notteams`
- Downloaded pictures and opened files: `~/.cache/notteams`
- Cisco's H.264 video codec, downloaded on the first start: `~/.local/share/notteams/openh264`
- The Flatpak keeps all of these under `~/.var/app/com.hawwwran.notteams/` instead, so
  moving from a self-built copy to the Flatpak means signing in once more.
- A self-built copy adds `~/.local/share/applications/com.hawwwran.notteams.desktop` and its
  icon in `~/.local/share/icons/hicolor/scalable/apps/`. That entry wins over the Flatpak's
  in the app menu, so remove both when switching to the Flatpak, then run
  `update-desktop-database ~/.local/share/applications`.

Builds from before the rename used `teams-rs` for both folders; the app moves them on its
first start, so you stay signed in.

## Releasing

Releases are made with Project Manager, which runs `.project-manager/release`
(`~/git/project-manager/docs/project-folder.md`): it bumps the version in `Cargo.toml`,
`Cargo.lock` and the metainfo's release list, tags `vX.Y.Z`, builds the Flatpak from
`com.hawwwran.notteams.yaml` with hwnhub's `publish.sh`, signs it into the channel and makes
the GitHub release. Its "hwnhub channel" action publishes an existing tag again.
