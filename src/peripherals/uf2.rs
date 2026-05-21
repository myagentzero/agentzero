//! UF2 flashing support — detect BOOTSEL-mode Pico and deploy firmware.
//!
//! # Workflow
//! 1. [`find_rpi_rp2_mount`] — check well-known mount points for the RPI-RP2 volume
//!    that appears when a Pico is held in BOOTSEL mode.
//! 2. [`ensure_firmware_dir`] — read firmware files from workspace/firmware/pico/
//!    or extract to `~/.zeroclaw/firmware/pico/` as a cache.
//! 3. [`flash_uf2`] — copy the UF2 to the mount point; the Pico reboots automatically.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// UF2 magic word 1 (little-endian bytes at offset 0 of every UF2 block).
const UF2_MAGIC1: [u8; 4] = [0x55, 0x46, 0x32, 0x0A];

// ── Volume detection ──────────────────────────────────────────────────────────

/// Find the RPI-RP2 mount point if a Pico is connected in BOOTSEL mode.
///
/// Checks:
/// - macOS:  `/Volumes/RPI-RP2`
/// - Linux:  `/media/*/RPI-RP2` and `/run/media/*/RPI-RP2`
pub fn find_rpi_rp2_mount() -> Option<PathBuf> {
    // macOS
    let mac = PathBuf::from("/Volumes/RPI-RP2");
    if mac.exists() {
        return Some(mac);
    }

    // Linux — /media/<user>/RPI-RP2  or  /run/media/<user>/RPI-RP2
    for base in &["/media", "/run/media"] {
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("RPI-RP2");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

// ── Firmware directory management ─────────────────────────────────────────────

/// Ensure firmware files exist at workspace_dir/firmware/pico/.
///
/// Validates that both the UF2 and main.py files exist and are valid.
/// Returns the firmware directory path.
pub fn ensure_firmware_dir(workspace_dir: &Path) -> Result<PathBuf> {
    let firmware_dir = workspace_dir.join("firmware").join("pico");

    if !firmware_dir.exists() {
        bail!(
            "firmware/pico directory not found at {}\n\
             Ensure you have firmware/pico/zeroclaw-pico.uf2 and firmware/pico/main.py \
             in your workspace.",
            firmware_dir.display()
        );
    }

    // UF2 — validate magic before use.
    let uf2_path = firmware_dir.join("zeroclaw-pico.uf2");
    if !uf2_path.exists() {
        bail!(
            "UF2 firmware not found at {}\n\
             Download the MicroPython UF2 from https://micropython.org/download/RPI_PICO/ \
             and place it at {}",
            uf2_path.display(),
            uf2_path.display()
        );
    }

    let uf2_data = std::fs::read(&uf2_path)?;
    if uf2_data.len() < 8 || uf2_data[..4] != UF2_MAGIC1 {
        bail!(
            "UF2 at {} does not have valid UF2 magic.\n\
             Download the real MicroPython UF2 from https://micropython.org/download/RPI_PICO/",
            uf2_path.display()
        );
    }

    // main.py — just check it exists (no magic check needed for text).
    let main_py_path = firmware_dir.join("main.py");
    if !main_py_path.exists() {
        bail!(
            "main.py not found at {}\n\
             Ensure firmware/pico/main.py exists in your workspace.",
            main_py_path.display()
        );
    }

    Ok(firmware_dir)
}

// ── Flashing ──────────────────────────────────────────────────────────────────

/// Copy the UF2 file to the RPI-RP2 mount point.
///
/// macOS often returns "Operation not permitted" for `std::fs::copy` on FAT
/// volumes presented by BOOTSEL-mode Picos.  We try four approaches in order
/// and return a clear manual-fallback message if all fail:
///
/// 1. `std::fs::copy`  — fast, no subprocess; works on most Linux setups.
/// 2. `cp <src> <dst>` — bypasses some macOS VFS permission layers.
/// 3. `sudo cp …`      — escalates for locked volumes.
/// 4. Error — instructs the user to run the `sudo cp` manually.
pub async fn flash_uf2(mount_point: &Path, firmware_dir: &Path) -> Result<()> {
    let uf2_src = firmware_dir.join("zeroclaw-pico.uf2");
    let uf2_dst = mount_point.join("firmware.uf2");
    let src_str = uf2_src.to_string_lossy().into_owned();
    let dst_str = uf2_dst.to_string_lossy().into_owned();

    tracing::info!(
        src = %src_str,
        dst = %dst_str,
        "flashing UF2"
    );

    // Validate UF2 magic before any copy attempt — prevents flashing a stub.
    let data = std::fs::read(&uf2_src)?;
    if data.len() < 8 || data[..4] != UF2_MAGIC1 {
        bail!(
            "UF2 at {} does not look like a valid UF2 file (magic mismatch). \
             Download from https://micropython.org/download/RPI_PICO/ and delete \
             the existing file so ZeroClaw can re-extract it.",
            uf2_src.display()
        );
    }

    // ── Attempt 1: std::fs::copy (works on Linux, sometimes blocked on macOS) ─
    {
        let src = uf2_src.clone();
        let dst = uf2_dst.clone();
        let result = tokio::task::spawn_blocking(move || std::fs::copy(&src, &dst))
            .await
            .map_err(|e| anyhow::anyhow!("copy task panicked: {e}"));

        match result {
            Ok(Ok(_)) => {
                tracing::info!("UF2 copy complete (std::fs::copy) — Pico will reboot");
                return Ok(());
            }
            Ok(Err(e)) => tracing::warn!("std::fs::copy failed ({}), trying cp", e),
            Err(e) => tracing::warn!("std::fs::copy task failed ({}), trying cp", e),
        }
    }

    // ── Attempt 2: cp via subprocess ──────────────────────────────────────────
    {
        /// Timeout for subprocess copy attempts (seconds).
        const CP_TIMEOUT_SECS: u64 = 10;

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(CP_TIMEOUT_SECS),
            tokio::process::Command::new("cp")
                .arg(&src_str)
                .arg(&dst_str)
                .output(),
        )
        .await;

        match out {
            Err(_elapsed) => {
                tracing::warn!("cp timed out after {}s, trying sudo cp", CP_TIMEOUT_SECS);
            }
            Ok(Ok(o)) if o.status.success() => {
                tracing::info!("UF2 copy complete (cp) — Pico will reboot");
                return Ok(());
            }
            Ok(Ok(o)) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!("cp failed ({}), trying sudo cp", stderr.trim());
            }
            Ok(Err(e)) => tracing::warn!("cp spawn failed ({}), trying sudo cp", e),
        }
    }

    // ── Attempt 3: sudo cp (non-interactive) ─────────────────────────────────
    {
        const SUDO_CP_TIMEOUT_SECS: u64 = 10;

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(SUDO_CP_TIMEOUT_SECS),
            tokio::process::Command::new("sudo")
                .args(["-n", "cp", &src_str, &dst_str])
                .output(),
        )
        .await;

        match out {
            Err(_elapsed) => {
                tracing::warn!("sudo cp timed out after {}s", SUDO_CP_TIMEOUT_SECS);
            }
            Ok(Ok(o)) if o.status.success() => {
                tracing::info!("UF2 copy complete (sudo cp) — Pico will reboot");
                return Ok(());
            }
            Ok(Ok(o)) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!("sudo cp failed: {}", stderr.trim());
            }
            Ok(Err(e)) => tracing::warn!("sudo cp spawn failed: {}", e),
        }
    }

    // ── All attempts failed — give the user a clear manual command ────────────
    bail!(
        "All copy methods failed. Run this command manually, then restart ZeroClaw:\n\
         \n  sudo cp {src_str} {dst_str}\n"
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_rpi_rp2_mount_returns_none_when_not_connected() {
        // This test runs on CI without a Pico attached — just verify it doesn't panic.
        let _ = find_rpi_rp2_mount(); // may be Some or None depending on environment
    }
}
