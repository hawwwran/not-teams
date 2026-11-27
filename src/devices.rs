//! Choosing the microphone, speaker and camera for calls; an empty choice means the system
//! default. Cameras are kept by their `/dev/video*` path.
//!
//! Audio: on PipeWire (most current desktops) the choices are PipeWire's sinks and sources,
//! kept as `pw:<node.name>`. The call opens the default ALSA device (which is PipeWire) and
//! then points its streams at the chosen nodes through the `target.object` metadata, the way
//! the desktop's sound settings move a running stream. Without PipeWire, ALSA's hardware
//! devices are listed by cpal's device id.
//!
//! Bluetooth headsets are listed by device (`bt:<device.name>`), not by node: in their
//! high-quality listening profile (A2DP) they have no microphone node at all, only in the
//! headset profile (HSP/HFP). Choosing one as the microphone switches it to the headset
//! profile for the call and back afterwards, as Teams and phones do.

use std::sync::mpsc::{Receiver, channel};

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
use v4l::video::Capture;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceChoice {
    pub microphone: String,
    pub speaker: String,
    pub camera: String,
}

impl DeviceChoice {
    /// Earlier builds saved Bluetooth headsets by node (`pw:bluez_output.<address>.1`),
    /// which disappears when the headset changes profile; they're kept by device now.
    pub fn normalize(&mut self) {
        for id in [&mut self.microphone, &mut self.speaker] {
            let address = id
                .strip_prefix("pw:bluez_output.")
                .or_else(|| id.strip_prefix("pw:bluez_input."))
                .and_then(|rest| rest.split('.').next())
                .map(str::to_string);
            if let Some(address) = address {
                *id = format!("{BLUETOOTH}bluez_card.{address}");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lists {
    pub microphones: Vec<Device>,
    pub speakers: Vec<Device>,
    pub cameras: Vec<Device>,
}

const PIPEWIRE: &str = "pw:";
const BLUETOOTH: &str = "bt:";

/// PipeWire's objects (`pw-dump`), or `None` without PipeWire.
fn pw_dump() -> Option<Vec<serde_json::Value>> {
    let out = std::process::Command::new("pw-dump").stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then_some(())?;
    serde_json::from_slice(&out.stdout).ok()
}

fn props(o: &serde_json::Value) -> &serde_json::Value {
    &o["info"]["props"]
}

fn audio_devices(input: bool) -> Vec<Device> {
    if let Some(objects) = pw_dump() {
        let class = if input { "Audio/Source" } else { "Audio/Sink" };
        let mut list: Vec<Device> = objects
            .iter()
            .map(props)
            .filter(|p| p["media.class"] == class)
            .filter(|p| !p["node.name"].as_str().unwrap_or("").starts_with("bluez_"))
            .filter_map(|p| {
                let node = p["node.name"].as_str()?;
                // The profile's name ("Speaker + Headphones") reads better than the full
                // description, which repeats the sound card's long chip name each time.
                let name = p["device.profile.description"]
                    .as_str()
                    .or_else(|| p["node.description"].as_str())
                    .or_else(|| p["node.nick"].as_str())
                    .unwrap_or(node);
                Some(Device { id: format!("{PIPEWIRE}{node}"), name: name.to_string() })
            })
            .collect();
        for dev in objects.iter().filter(|o| is_bluetooth(o)) {
            let p = props(dev);
            let (Some(card), Some(name)) = (p["device.name"].as_str(), p["device.description"].as_str()) else { continue };
            if input && headset_profile(dev).is_none() {
                continue;
            }
            list.push(Device { id: format!("{BLUETOOTH}{card}"), name: name.to_string() });
        }
        return list;
    }
    alsa_devices(input)
}

fn is_bluetooth(o: &serde_json::Value) -> bool {
    o["type"].as_str().is_some_and(|t| t.ends_with(":Device")) && props(o)["device.api"] == "bluez5"
}

/// The best headset (microphone) profile's index: mSBC (wideband) first.
fn headset_profile(dev: &serde_json::Value) -> Option<u64> {
    let profiles = dev["info"]["params"]["EnumProfile"].as_array()?;
    let name = |p: &&serde_json::Value| p["name"].as_str().unwrap_or("").to_string();
    let available = |p: &&serde_json::Value| p["available"].as_str() != Some("no");
    ["headset-head-unit-msbc", "headset-head-unit", "headset-head-unit-cvsd"]
        .iter()
        .find_map(|want| profiles.iter().filter(available).find(|p| name(p) == *want))
        .or_else(|| profiles.iter().filter(available).find(|p| name(p).starts_with("headset-head-unit")))
        .and_then(|p| p["index"].as_u64())
}

fn current_profile(dev: &serde_json::Value) -> Option<(u64, String)> {
    let p = dev["info"]["params"]["Profile"].as_array()?.first()?;
    Some((p["index"].as_u64()?, p["name"].as_str().unwrap_or("").to_string()))
}

/// A PipeWire tool that changes something. Inside a Flatpak, WirePlumber lets a client only
/// read unless it says it's a manager (its access rules for `pipewire.access = flatpak`).
fn pw_writer(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    if crate::desktop::in_flatpak() {
        cmd.env("PIPEWIRE_PROPS", "{ media.category = Manager }");
    }
    cmd
}

/// Through WirePlumber's `wpctl`, or PipeWire's own `pw-cli` where there's no `wpctl` (the
/// Flatpak runtime).
fn set_profile(device: u64, index: u64) -> bool {
    let run = |program: &str, args: &[&str]| {
        pw_writer(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let (device, index) = (device.to_string(), index.to_string());
    run("wpctl", &["set-profile", &device, &index])
        || run("pw-cli", &["set-param", &device, "Profile", &format!("{{ index: {index}, save: false }}")])
}

/// Puts a Bluetooth headset back in the profile it had before the call.
pub struct ProfileGuard {
    device: u64,
    restore: u64,
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        set_profile(self.device, self.restore);
    }
}

/// Turns Bluetooth choices into the nodes to route to: switches a headset chosen as the
/// microphone to its headset profile (waiting for its microphone to appear) and resolves
/// `bt:` ids to the device's current nodes. The guard restores the profile when dropped.
pub fn prepare(choice: &DeviceChoice) -> (DeviceChoice, Option<ProfileGuard>) {
    let mut normalized = choice.clone();
    normalized.normalize();
    let choice = &normalized;
    let mut resolved = choice.clone();
    if !choice.microphone.starts_with(BLUETOOTH) && !choice.speaker.starts_with(BLUETOOTH) {
        return (resolved, None);
    }
    let Some(objects) = pw_dump() else { return (resolved, None) };
    let device_of = |id: &str| {
        let card = id.strip_prefix(BLUETOOTH)?;
        objects.iter().filter(|o| is_bluetooth(o)).find(|o| props(o)["device.name"] == card).cloned()
    };
    let mut guard = None;
    if let Some(dev) = device_of(&choice.microphone) {
        let id = dev["id"].as_u64().unwrap_or_default();
        if let (Some((now, name)), Some(headset)) = (current_profile(&dev), headset_profile(&dev)) {
            if !name.starts_with("headset-head-unit") && set_profile(id, headset) {
                guard = Some(ProfileGuard { device: id, restore: now });
            }
        }
    }
    // Resolve each Bluetooth choice to that device's node now (after any profile switch).
    let node_for = |choice: &str, class: &str| -> Option<String> {
        let card = choice.strip_prefix(BLUETOOTH)?;
        for _ in 0..20 {
            let objects = pw_dump()?;
            let dev = objects.iter().filter(|o| is_bluetooth(o)).find(|o| props(o)["device.name"] == card)?;
            let dev_id = dev["id"].as_u64()?;
            let node = objects
                .iter()
                .map(props)
                .find(|p| p["media.class"] == class && p["device.id"].as_u64() == Some(dev_id))
                .and_then(|p| p["node.name"].as_str().map(str::to_string));
            if node.is_some() {
                return node;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        None
    };
    if choice.microphone.starts_with(BLUETOOTH) {
        resolved.microphone = node_for(&choice.microphone, "Audio/Source").map(|n| format!("{PIPEWIRE}{n}")).unwrap_or_default();
    }
    // With the headset in its call profile, its sound comes from the headset's own sink too.
    let speaker = if choice.speaker.starts_with(BLUETOOTH) {
        choice.speaker.clone()
    } else if guard.is_some() && choice.speaker.is_empty() {
        choice.microphone.clone()
    } else {
        String::new()
    };
    if !speaker.is_empty() {
        resolved.speaker = node_for(&speaker, "Audio/Sink").map(|n| format!("{PIPEWIRE}{n}")).unwrap_or_default();
    }
    (resolved, guard)
}

/// Points this process's audio streams at the chosen PipeWire nodes (default when empty).
/// Run after the streams start; returns how many streams were routed.
pub fn route_streams(choice: &DeviceChoice) -> usize {
    let Some(objects) = pw_dump() else { return 0 };
    let pid = std::process::id().to_string();
    // A stream's process id may be on the node or only on its client.
    let ours: std::collections::HashSet<u64> = objects
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with(":Client")))
        .filter(|o| props(o)["application.process.id"].as_str() == Some(&pid) || props(o)["application.process.id"].as_u64().map(|p| p.to_string()) == Some(pid.clone()))
        .filter_map(|o| o["id"].as_u64())
        .collect();
    let mut routed = 0;
    for o in &objects {
        let p = props(o);
        let mine = p["client.id"].as_u64().is_some_and(|c| ours.contains(&c))
            || p["application.process.id"].as_str() == Some(&pid);
        let target = match p["media.class"].as_str() {
            Some("Stream/Input/Audio") if mine => &choice.microphone,
            Some("Stream/Output/Audio") if mine => &choice.speaker,
            _ => continue,
        };
        let Some(id) = o["id"].as_u64() else { continue };
        let mut cmd = pw_writer("pw-metadata");
        match target.strip_prefix(PIPEWIRE) {
            Some(node) => cmd.args([id.to_string().as_str(), "target.object", node]),
            None => cmd.args(["-d", id.to_string().as_str(), "target.object"]),
        };
        if cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) {
            routed += 1;
        }
    }
    routed
}

/// Is this a PipeWire choice (or the default), handled by routing rather than opening?
pub fn routed(id: &str) -> bool {
    id.is_empty() || id.starts_with(PIPEWIRE) || id.starts_with(BLUETOOTH)
}

fn alsa_devices(input: bool) -> Vec<Device> {
    let host = cpal::default_host();
    let devices = if input { host.input_devices() } else { host.output_devices() };
    let mut out: Vec<Device> = devices
        .map(|d| {
            d.filter_map(|dev| {
                let id = dev.id().ok()?.to_string();
                let name = dev.description().ok().map(|d| d.name().to_string()).unwrap_or_else(|| id.clone());
                Some(Device { id, name })
            })
            .collect()
        })
        .unwrap_or_default();
    // ALSA also lists its plugins (rate converters, JACK, OSS...); offer the hardware.
    out.retain(|d| d.id.contains(":hw:") || d.id.contains("plughw:"));
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert(d.name.clone()));
    out
}

/// Cameras that deliver pictures; a webcam also exposes metadata nodes with no formats.
fn cameras() -> Vec<Device> {
    v4l::context::enum_devices()
        .into_iter()
        .filter_map(|node| {
            let dev = v4l::Device::with_path(node.path()).ok()?;
            if dev.enum_formats().map(|f| f.is_empty()).unwrap_or(true) {
                return None;
            }
            let name = dev.query_caps().map(|c| c.card).ok().or_else(|| node.name()).unwrap_or_default();
            Some(Device { id: node.path().display().to_string(), name })
        })
        .collect()
}

/// All three lists now, on this thread (for `--list-devices`).
pub fn list_now() -> Lists {
    Lists {
        microphones: audio_devices(true),
        speakers: audio_devices(false),
        cameras: cameras(),
    }
}

/// The device lists, refreshed on a thread (probing ALSA can take a moment).
#[derive(Default)]
pub struct Catalog {
    pub lists: Option<Lists>,
    pending: Option<Receiver<Lists>>,
}

impl Catalog {
    pub fn refresh(&mut self, ctx: &eframe::egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let lists = Lists {
                microphones: audio_devices(true),
                speakers: audio_devices(false),
                cameras: cameras(),
            };
            let _ = tx.send(lists);
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            if let Ok(lists) = rx.try_recv() {
                self.lists = Some(lists);
                self.pending = None;
            }
        }
    }
}

/// Three pickers (microphone, speaker, camera); true when the choice changed. Each is a
/// short list of choices rather than a dropdown: a dropdown's list opens as its own popup,
/// and inside the call bar's popup that counts as a click outside and closes everything.
pub fn pickers(ui: &mut eframe::egui::Ui, choice: &mut DeviceChoice, catalog: &mut Catalog) -> bool {
    catalog.poll();
    if catalog.lists.is_none() {
        catalog.refresh(ui.ctx());
        ui.label(eframe::egui::RichText::new("Looking for devices…").color(crate::theme::FG_3));
        return false;
    }
    let lists = catalog.lists.clone().unwrap_or_default();
    let before = choice.clone();
    choice.normalize();
    let mut changed = *choice != before;
    changed |= picker(ui, "Microphone", &mut choice.microphone, &lists.microphones);
    ui.add_space(6.0);
    changed |= picker(ui, "Speaker", &mut choice.speaker, &lists.speakers);
    ui.add_space(6.0);
    changed |= picker(ui, "Camera", &mut choice.camera, &lists.cameras);
    changed
}

fn picker(ui: &mut eframe::egui::Ui, label: &str, value: &mut String, devices: &[Device]) -> bool {
    use eframe::egui::RichText;
    ui.label(RichText::new(label).font(crate::theme::strong(13.0)));
    let before = value.clone();
    ui.radio_value(value, String::new(), "System default");
    for d in devices {
        ui.radio_value(value, d.id.clone(), &d.name);
    }
    // A saved device that isn't plugged in stays selected, shown as such.
    if !value.is_empty() && !devices.iter().any(|d| d.id == *value) {
        let _ = ui.radio(true, RichText::new("Previous choice (not connected)").color(crate::theme::FG_3));
    }
    *value != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_bluetooth_node_choices_become_devices() {
        let mut c = DeviceChoice { speaker: "pw:bluez_output.00_BB_43_60_BC_7F.1".into(), ..Default::default() };
        c.normalize();
        assert_eq!(c.speaker, "bt:bluez_card.00_BB_43_60_BC_7F");
        let mut keep = DeviceChoice { speaker: "pw:alsa_output.x".into(), ..Default::default() };
        keep.normalize();
        assert_eq!(keep.speaker, "pw:alsa_output.x");
    }
}
