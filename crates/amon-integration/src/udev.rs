//! The one root step desk devices can need: a udev rule.
//!
//! `/dev/hidraw*` and `/dev/uinput` are root-only on a stock system. amon
//! never runs as root, so granting the seated user access is udev's job —
//! one rule file, written once with the user's own sudo, disclosed in full
//! before it happens. Everything else about device support is zero-setup;
//! this is the single exception, and machines that already have equivalent
//! rules (the Work Louder Input app installs some) never see it.

use std::path::Path;

/// Where the rule lives. Removal is the mirror image:
/// `sudo rm` this path, documented wherever the rule is mentioned.
pub const RULES_PATH: &str = "/etc/udev/rules.d/70-amon-devices.rules";

/// The rule itself: seat access (`uaccess`) for exactly the devices amon
/// speaks to — the Creator Micro 2's USB and Bluetooth faces — and for the
/// uinput node amon's virtual keyboard and scroll wheel need. Product ids,
/// not the whole vendor: `303a` is Espressif's shared VID, and a vendor
/// wildcard would hand raw HID access to every ESP32 gadget on the desk.
/// One line per face that exists — `8297` is the wired product and `8298`
/// the Bluetooth one, so a wired `8298` or a Bluetooth `8297` would be a
/// rule matching nothing. Tags, not mode changes — access follows the
/// login seat the way the rest of the desktop does.
pub const RULES: &str = r#"# amon: desk devices (Work Louder Creator Micro 2) and the virtual input device.
# Installed by `amon setup`; remove with: sudo rm /etc/udev/rules.d/70-amon-devices.rules
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="303a", ATTRS{idProduct}=="8297", TAG+="uaccess"
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", KERNELS=="0005:303A:8298.*", TAG+="uaccess"
KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"
"#;

/// Whether amon's own rule file is on disk.
pub fn installed() -> bool {
    Path::new(RULES_PATH).exists()
}

/// Whether a path is openable read-write by this user — the only test that
/// matters, run against the actual node rather than inferred from rules.
pub fn accessible(path: &Path) -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .is_ok()
}

/// The command a user (or setup, with their sudo) runs to install the rule
/// and make it take effect without replugging.
pub fn install_command() -> String {
    format!(
        "sudo sh -c 'cat > {RULES_PATH} <<\"EOF\"\n{RULES}EOF\nudevadm control --reload-rules && udevadm trigger'"
    )
}
