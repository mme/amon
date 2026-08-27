//! Physical status devices on the desk (ADR-0016).
//!
//! A registry of device modules keyed by what the hardware announces:
//! today the Work Louder Creator Micro 2, wired (`303a:8297`) or Bluetooth
//! (`303a:8298`). Discovery polls the hidraw layer — cheap, and it makes
//! hotplug, Bluetooth reconnects, and cable switches all the same event:
//! a node appearing or disappearing. Wired is preferred when both faces of
//! one device are up.
//!
//! Zero setup by design: a device that appears gets attached and lit. The
//! one thing amon cannot do itself is grant access to the node — that is a
//! udev rule (`amon setup` offers it), and a device amon can see but not
//! open earns a single desktop notification saying exactly that.

pub mod actions;
pub mod micro2;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use crate::registry::Registry;

/// The registry connection id devices subscribe under. Never handed to a
/// socket connection: those count up from zero.
const DEVICES_CONNECTION: u64 = u64::MAX;

const VENDOR_WORK_LOUDER_V2: u32 = 0x303A;
const PRODUCT_MICRO2_USB: u32 = 0x8297;
const PRODUCT_MICRO2_BLE: u32 = 0x8298;

/// One discovered vendor node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub node: PathBuf,
    /// HID bus: 0x03 USB, 0x05 Bluetooth.
    pub bus: u32,
}

/// Scans the hidraw layer for a Creator Micro 2 vendor node: matching
/// VID/PID, and a report descriptor that declares the `0xFF00` vendor page
/// (over USB the keyboard interfaces match the IDs too; only the vendor
/// node speaks RPC).
pub fn discover() -> Option<Discovered> {
    let entries = std::fs::read_dir("/sys/class/hidraw").ok()?;
    let mut found: Vec<Discovered> = Vec::new();
    for entry in entries.flatten() {
        let sys = entry.path();
        let Ok(uevent) = std::fs::read_to_string(sys.join("device/uevent")) else {
            continue;
        };
        let Some(id_line) = uevent.lines().find_map(|line| line.strip_prefix("HID_ID=")) else {
            continue;
        };
        let mut parts = id_line.split(':');
        let (Some(bus), Some(vendor), Some(product)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let parse = |text: &str| u32::from_str_radix(text, 16).ok();
        let (Some(bus), Some(vendor), Some(product)) = (parse(bus), parse(vendor), parse(product))
        else {
            continue;
        };
        if vendor != VENDOR_WORK_LOUDER_V2
            || !matches!(product, PRODUCT_MICRO2_USB | PRODUCT_MICRO2_BLE)
        {
            continue;
        }
        let Ok(descriptor) = std::fs::read(sys.join("device/report_descriptor")) else {
            continue;
        };
        // Usage Page (Vendor 0xFF00) is the bytes 06 00 FF.
        if !descriptor.windows(3).any(|w| w == [0x06, 0x00, 0xFF]) {
            continue;
        }
        let Some(name) = sys.file_name() else {
            continue;
        };
        found.push(Discovered {
            node: PathBuf::from("/dev").join(name),
            bus,
        });
    }
    // Wired beats Bluetooth when both faces of the device are present.
    found.sort_by_key(|d| d.bus);
    found.into_iter().next()
}

/// Tells the user why their device is dark, once per appearance. Through
/// notify-send because the one person who needs this is at a desktop.
fn notify_no_access() {
    actions::reap(
        std::process::Command::new("notify-send")
            .args([
                "-a",
                "amon",
                "Creator Micro 2 detected",
                "amon can see it but not open it - run `amon setup` to finish (one udev rule).",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn(),
    );
}

pub struct DevicesHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DevicesHandle {
    /// Asks the discovery loop to wind down — it darkens any attached
    /// device on the way out — and waits for it briefly. The daemon calls
    /// this after its accept loop ends, so a shutdown leaves no zombie
    /// colors on the desk.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the discovery loop. It owns everything device: scanning,
/// attaching, feeding rosters and config, and shutdown.
pub fn start(registry: Registry, config: crate::config::Watcher) -> DevicesHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();

    let thread = std::thread::spawn(move || {
        // The roster reaches devices through the registry's own event
        // stream: any event means "look again", and the snapshot API gives
        // a consistent answer.
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        registry.subscribe(DEVICES_CONNECTION, events_tx);

        let mut attached: Option<(
            Sender<micro2::ToDevice>,
            std::thread::JoinHandle<()>,
            PathBuf,
        )> = None;
        let mut warned_about: Option<PathBuf> = None;
        let mut last_config = config.current().devices.micro2.clone();
        let mut unsupported_at: Option<PathBuf> = None;
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                if let Some((to_device, thread, _)) = attached.take() {
                    let _ = to_device.send(micro2::ToDevice::Shutdown);
                    let _ = thread.join();
                }
                return;
            }

            // Device outcomes: a detach clears the slot; unsupported
            // firmware marks the node so it is not re-probed every scan.
            while let Ok(outcome) = outcome_rx.try_recv() {
                if let Some((_, thread, node)) = attached.take() {
                    let _ = thread.join();
                    if outcome == micro2::DeviceOutcome::Unsupported {
                        tracing::info!(
                            "micro2 at {} has firmware without lighting support",
                            node.display()
                        );
                        unsupported_at = Some(node);
                    }
                }
            }

            // Roster or config changes flow to the attached device.
            let mut roster_changed = false;
            while let Ok(event) = events_rx.try_recv() {
                match event {
                    amon_protocol::Event::ConfigChanged(_) => {
                        let next = config.current().devices.micro2.clone();
                        if next != last_config {
                            last_config = next.clone();
                            unsupported_at = None; // a config change re-opens the question
                            if let Some((to_device, _, _)) = attached.as_ref() {
                                if next.enabled {
                                    let _ =
                                        to_device.send(micro2::ToDevice::Config(Box::new(next)));
                                } else {
                                    // Disabling detaches: the thread darkens
                                    // what we lit and exits, and the attach
                                    // guard below keeps us from reopening.
                                    let _ = to_device.send(micro2::ToDevice::Shutdown);
                                }
                            }
                        }
                    }
                    _ => roster_changed = true,
                }
            }
            if roster_changed {
                if let Some((to_device, _, _)) = attached.as_ref() {
                    let _ = to_device.send(micro2::ToDevice::Agents(registry.agents()));
                }
            }

            // Discovery: attach what appeared, notice what vanished.
            let present = discover();
            match (&attached, &present) {
                (Some((_, _, node)), Some(found)) if *node == found.node => {}
                (Some(_), _) => {
                    // Node changed or vanished; the device loop notices the
                    // dead fd on its own, and the outcome above clears us.
                }
                (None, Some(found)) => {
                    if !last_config.enabled {
                        // `enabled = false` means exactly that: the node is
                        // never opened, no lighting is written, no virtual
                        // input is created — the device is someone else's.
                    } else if unsupported_at.as_ref() == Some(&found.node) {
                        // Known unsupported; leave it alone until it moves.
                    } else {
                        match std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(&found.node)
                        {
                            Ok(file) => {
                                let (to_device, inbox) = std::sync::mpsc::channel();
                                let _ = to_device.send(micro2::ToDevice::Agents(registry.agents()));
                                let outcome = outcome_tx.clone();
                                let device_config = last_config.clone();
                                let thread = std::thread::spawn(move || {
                                    micro2::run(file, inbox, outcome, device_config);
                                });
                                tracing::info!("micro2 attached at {}", found.node.display());
                                attached = Some((to_device, thread, found.node.clone()));
                                warned_about = None;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                                if warned_about.as_ref() != Some(&found.node) {
                                    warned_about = Some(found.node.clone());
                                    notify_no_access();
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
                (None, None) => {
                    warned_about = None;
                    unsupported_at = None;
                }
            }

            std::thread::sleep(Duration::from_millis(1500));
        }
    });

    DevicesHandle {
        stop,
        thread: Some(thread),
    }
}
