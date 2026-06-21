//! Deploy AgentZero Bridge app to Arduino Uno Q.
//!
//! Reads Bridge app from workspace/firmware/uno-q-bridge/ at runtime.
//! If host is Some, scp from workspace and ssh to start.
//! If host is None, assume we're ON the Uno Q — copy to local ArduinoApps and start.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const BRIDGE_APP_NAME: &str = "uno-q-bridge";

/// Deploy the Bridge app from workspace. If host is Some, scp and start remotely.
/// If host is None, copy to local ArduinoApps directory and start locally.
pub fn setup_uno_q_bridge(host: Option<&str>, workspace_dir: &Path) -> Result<()> {
    let bridge_dir = workspace_dir.join("firmware").join("uno-q-bridge");

    if !bridge_dir.exists() {
        anyhow::bail!(
            "Bridge app not found at {}\n\
             Ensure firmware/uno-q-bridge exists in your workspace.",
            bridge_dir.display()
        );
    }

    if let Some(h) = host {
        deploy_remote(h, &bridge_dir)?;
    } else {
        deploy_local(&bridge_dir)?;
    }
    Ok(())
}

fn deploy_remote(host: &str, bridge_dir: &std::path::Path) -> Result<()> {
    let ssh_target = if host.contains('@') {
        host.to_string()
    } else {
        format!("arduino@{}", host)
    };

    println!("Copying Bridge app to {}...", host);
    let status = Command::new("ssh")
        .args([&ssh_target, "mkdir", "-p", "~/ArduinoApps"])
        .status()
        .context("ssh mkdir failed")?;
    if !status.success() {
        anyhow::bail!("Failed to create ArduinoApps dir on Uno Q");
    }

    let status = Command::new("scp")
        .args([
            "-r",
            bridge_dir.to_str().unwrap(),
            &format!("{}:~/ArduinoApps/", ssh_target),
        ])
        .status()
        .context("scp failed")?;
    if !status.success() {
        anyhow::bail!("Failed to copy Bridge app");
    }

    println!("Starting Bridge app on Uno Q...");
    let status = Command::new("ssh")
        .args([
            &ssh_target,
            "arduino-app-cli",
            "app",
            "start",
            "~/ArduinoApps/uno-q-bridge",
        ])
        .status()
        .context("arduino-app-cli start failed")?;
    if !status.success() {
        anyhow::bail!("Failed to start Bridge app. Ensure arduino-app-cli is installed on Uno Q.");
    }

    println!("AgentZero Bridge app started. Add to config.toml:");
    println!("  [[peripherals.boards]]");
    println!("  board = \"arduino-uno-q\"");
    println!("  transport = \"bridge\"");
    Ok(())
}

fn deploy_local(bridge_dir: &std::path::Path) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/arduino".into());
    let apps_dir = std::path::Path::new(&home).join("ArduinoApps");
    let dest_dir = apps_dir.join(BRIDGE_APP_NAME);

    std::fs::create_dir_all(&dest_dir).context("create dest dir")?;

    println!("Copying Bridge app from workspace...");
    copy_dir(bridge_dir, &dest_dir)?;

    println!("Starting Bridge app...");
    let status = Command::new("arduino-app-cli")
        .args(["app", "start", dest_dir.to_str().unwrap()])
        .status()
        .context("arduino-app-cli start failed")?;
    if !status.success() {
        anyhow::bail!("Failed to start Bridge app. Ensure arduino-app-cli is installed on Uno Q.");
    }

    println!("AgentZero Bridge app started.");
    Ok(())
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let e = entry?;
        let name = e.file_name();
        let src_path = src.join(&name);
        let dst_path = dst.join(&name);
        if e.file_type()?.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
