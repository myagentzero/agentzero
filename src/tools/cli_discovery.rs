//! CLI tool auto-discovery — scans PATH for known CLI tools.
//! Zero external dependencies (uses `std::process::Command` + `std::env`).

use std::path::PathBuf;

/// Category of a discovered CLI tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum CliCategory {
    VersionControl,
    Language,
    PackageManager,
    Container,
    Build,
    Cloud,
}

impl std::fmt::Display for CliCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionControl => write!(f, "Version Control"),
            Self::Language => write!(f, "Language"),
            Self::PackageManager => write!(f, "Package Manager"),
            Self::Container => write!(f, "Container"),
            Self::Build => write!(f, "Build"),
            Self::Cloud => write!(f, "Cloud"),
        }
    }
}

/// A discovered CLI tool with metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredCli {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub category: CliCategory,
}

/// Known CLI tools to scan for.
struct KnownCli {
    name: &'static str,
    version_args: &'static [&'static str],
    category: CliCategory,
}

const KNOWN_CLIS: &[KnownCli] = &[
    KnownCli {
        name: "git",
        version_args: &["--version"],
        category: CliCategory::VersionControl,
    },
    KnownCli {
        name: "python",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "python3",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "node",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "npm",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "pip",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "pip3",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "make",
        version_args: &["--version"],
        category: CliCategory::Build,
    },
];

/// Discover available CLI tools on the system.
/// Scans PATH for known tools and returns metadata for each found.
pub fn discover_cli_tools(additional: &[String], excluded: &[String]) -> Vec<DiscoveredCli> {
    let mut results = Vec::new();

    for known in KNOWN_CLIS {
        if excluded.iter().any(|e| e == known.name) {
            continue;
        }
        if let Some(cli) = probe_cli(known.name, known.version_args, known.category.clone()) {
            results.push(cli);
        }
    }

    // Probe additional user-specified tools
    for tool_name in additional {
        if excluded.iter().any(|e| e == tool_name) {
            continue;
        }
        // Skip if already discovered
        if results.iter().any(|r| r.name == *tool_name) {
            continue;
        }
        if let Some(cli) = probe_cli(tool_name, &["--version"], CliCategory::Build) {
            results.push(cli);
        }
    }

    results
}

/// Probe a single CLI tool: check if it exists and get its version.
fn probe_cli(name: &str, version_args: &[&str], category: CliCategory) -> Option<DiscoveredCli> {
    // Try to find the tool using `which` (Unix) or `where` (Windows)
    let path = find_executable(name)?;

    // Try to get version
    let version = get_version(name, version_args);

    Some(DiscoveredCli {
        name: name.to_string(),
        path,
        version,
        category,
    })
}

/// Find an executable on PATH by walking PATH entries — no subprocess needed.
fn find_executable(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        #[cfg(target_os = "windows")]
        let candidate = {
            let p = dir.join(name);
            if p.extension().is_none() {
                p.with_extension("exe")
            } else {
                p
            }
        };
        #[cfg(not(target_os = "windows"))]
        let candidate = dir.join(name);

        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Get the version string of a CLI tool.
/// Enforces a 2-second timeout to avoid hanging on slow or broken tools.
fn get_version(name: &str, args: &[&str]) -> Option<String> {
    let mut child = std::process::Command::new(name)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }

    let output = child.wait_with_output().ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Some tools print version to stderr (e.g., pip)
    let version_text = if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };

    // Extract first line only
    let first_line = version_text.lines().next()?.trim().to_string();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_vec() {
        // Just verify it runs without panic
        let results = discover_cli_tools(&[], &[]);
        // We can't assert specific tools exist in CI, but structure is valid
        for cli in &results {
            assert!(!cli.name.is_empty());
        }
    }

    #[test]
    fn excluded_tools_are_skipped() {
        let results = discover_cli_tools(&[], &["git".to_string()]);
        assert!(!results.iter().any(|r| r.name == "git"));
    }

    #[test]
    fn category_display() {
        assert_eq!(CliCategory::VersionControl.to_string(), "Version Control");
        assert_eq!(CliCategory::Language.to_string(), "Language");
        assert_eq!(CliCategory::PackageManager.to_string(), "Package Manager");
        assert_eq!(CliCategory::Container.to_string(), "Container");
        assert_eq!(CliCategory::Build.to_string(), "Build");
        assert_eq!(CliCategory::Cloud.to_string(), "Cloud");
    }
}
