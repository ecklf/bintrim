use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ArchInfo {
    pub cpu_type: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    #[allow(dead_code)]
    pub path: PathBuf,
    #[allow(dead_code)]
    pub binary_path: PathBuf,
    pub architectures: Vec<ArchInfo>,
    pub selected: bool,
}

impl AppInfo {
    pub fn has_x86_64(&self) -> bool {
        self.architectures
            .iter()
            .any(|arch| arch.cpu_type == "x86_64")
    }

    pub fn has_arm64(&self) -> bool {
        self.architectures
            .iter()
            .any(|arch| arch.cpu_type.starts_with("arm64"))
    }

    pub fn x86_64_size_mb(&self) -> Option<f64> {
        self.architectures
            .iter()
            .find(|arch| arch.cpu_type == "x86_64")
            .and_then(|arch| arch.size_bytes.map(|size| size as f64 / 1024.0 / 1024.0))
    }

    pub fn architectures_display(&self) -> String {
        self.architectures
            .iter()
            .map(|arch| arch.cpu_type.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn scan_applications_with_progress<F>(mut progress_callback: F) -> Vec<AppInfo>
where
    F: FnMut(usize, usize, &str),
{
    let apps_dir = Path::new("/Applications");
    let mut apps = Vec::new();

    if let Ok(entries) = fs::read_dir(apps_dir) {
        let entries: Vec<_> = entries.flatten().collect();
        let total = entries.len();

        for (index, entry) in entries.iter().enumerate() {
            if let Ok(file_type) = entry.file_type() {
                let path = entry.path();

                // Check if it's an .app bundle
                if file_type.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("app") {
                    let app_name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown");

                    progress_callback(index + 1, total, app_name);

                    if let Some(app_info) = analyze_app(&path) {
                        // Only include apps that have arm64 architecture
                        if app_info.has_arm64() {
                            apps.push(app_info);
                        }
                    }
                }
            }
        }
    }

    // Sort by name
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

fn analyze_app(app_path: &Path) -> Option<AppInfo> {
    let app_name = app_path.file_stem()?.to_str()?.to_string();

    // Find the binary inside Contents/MacOS/
    let macos_dir = app_path.join("Contents").join("MacOS");

    if !macos_dir.exists() {
        return None;
    }

    // Try to find the main binary (usually named the same as the app)
    let mut binary_path = macos_dir.join(&app_name);

    // If that doesn't exist, try to find any executable in MacOS directory
    if !binary_path.exists()
        && let Ok(entries) = fs::read_dir(&macos_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                // Check if file is executable
                if is_executable(&path) {
                    binary_path = path;
                    break;
                }
            }
        }
    }

    if !binary_path.exists() {
        return None;
    }

    let architectures = extract_architectures(&binary_path)?;

    Some(AppInfo {
        name: app_name,
        path: app_path.to_path_buf(),
        binary_path,
        architectures,
        selected: false,
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let permissions = metadata.permissions();
            // Check if any execute bit is set
            return permissions.mode() & 0o111 != 0;
        }
    }
    false
}

fn extract_architectures(binary_path: &Path) -> Option<Vec<ArchInfo>> {
    let output = Command::new("lipo")
        .arg("-detailed_info")
        .arg(binary_path)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check if this is a non-fat file (single architecture)
    // lipo can exit with success (0) for non-fat files, so check the output content
    if stdout.contains("is not a fat file")
        || stdout.contains("Non-fat file")
        || stderr.contains("is not a fat file")
        || stderr.contains("Non-fat file")
    {
        // Try to extract the architecture from stdout first, then stderr
        let output_to_parse = if !stdout.is_empty() { &stdout } else { &stderr };
        return extract_single_architecture(binary_path, output_to_parse);
    }

    // Check if command failed for other reasons
    if !output.status.success() {
        return None;
    }

    parse_lipo_output(&stdout)
}

fn extract_single_architecture(binary_path: &Path, stderr: &str) -> Option<Vec<ArchInfo>> {
    // First, try to parse the architecture from stderr
    // Example: "Non-fat file: /path/to/binary is architecture: arm64"
    if let Some(arch) = parse_architecture_from_stderr(stderr) {
        return Some(vec![ArchInfo {
            cpu_type: arch,
            size_bytes: None,
        }]);
    }

    // Fallback: Use lipo -archs to get the architecture of a non-fat file
    let output = Command::new("lipo")
        .arg("-archs")
        .arg(binary_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let arch_str = String::from_utf8_lossy(&output.stdout);
    let arch_name = arch_str.trim();

    if arch_name.is_empty() {
        return None;
    }

    // For non-fat files, we don't have accurate per-architecture size
    // Set size_bytes to None
    Some(vec![ArchInfo {
        cpu_type: arch_name.to_string(),
        size_bytes: None,
    }])
}

fn parse_architecture_from_stderr(stderr: &str) -> Option<String> {
    // Parse messages like:
    // "Non-fat file: /path/to/binary is architecture: arm64"
    for line in stderr.lines() {
        if line.contains("is architecture:") {
            if let Some(arch_part) = line.split("is architecture:").nth(1) {
                let arch = arch_part.trim();
                if !arch.is_empty() {
                    return Some(arch.to_string());
                }
            }
        }
    }
    None
}

fn parse_lipo_output(output: &str) -> Option<Vec<ArchInfo>> {
    let mut architectures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // Look for architecture line
        if line.starts_with("architecture ") {
            if let Some(arch_name) = line.strip_prefix("architecture ") {
                let arch_name = arch_name.trim().to_string();

                // Find the size line (should be a few lines down)
                let mut size_bytes = None;
                for j in (i + 1)..std::cmp::min(i + 10, lines.len()) {
                    let size_line = lines[j].trim();
                    if size_line.starts_with("size ") {
                        // Extract size value
                        let parts: Vec<&str> = size_line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            if let Ok(size) = parts[1].parse::<u64>() {
                                size_bytes = Some(size);
                                break;
                            }
                        }
                    }
                }

                architectures.push(ArchInfo {
                    cpu_type: arch_name,
                    size_bytes,
                });
            }
        }

        i += 1;
    }

    if architectures.is_empty() {
        None
    } else {
        Some(architectures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lipo_output() {
        let output = r#"Fat header in: /Applications/NotchNook.app/Contents/MacOS/NotchNook
fat_magic 0xcafebabe
nfat_arch 2
architecture x86_64
    cputype CPU_TYPE_X86_64
    cpusubtype CPU_SUBTYPE_X86_64_ALL
    capabilities 0x0
    offset 16384
    size 9228032
    align 2^14 (16384)
architecture arm64
    cputype CPU_TYPE_ARM64
    cpusubtype CPU_SUBTYPE_ARM64_ALL
    capabilities 0x0
    offset 9256960
    size 8804432
    align 2^14 (16384)"#;

        let archs = parse_lipo_output(output).unwrap();
        assert_eq!(archs.len(), 2);
        assert_eq!(archs[0].cpu_type, "x86_64");
        assert_eq!(archs[0].size_bytes, Some(9228032));
        assert_eq!(archs[1].cpu_type, "arm64");
        assert_eq!(archs[1].size_bytes, Some(8804432));
    }

    #[test]
    fn test_parse_lipo_output_single_arch() {
        // Test that a single architecture (non-fat) file is handled correctly
        // This would be handled by extract_single_architecture in practice
        let output = "Non-fat file: /path/to/binary is architecture: arm64";

        // parse_lipo_output should return None for non-fat file output
        assert!(parse_lipo_output(output).is_none());
    }

    #[test]
    fn test_parse_architecture_from_stderr() {
        let stderr = "input file /Applications/Beekeeper Studio.app/Contents/MacOS/Beekeeper Studio is not a fat file\nNon-fat file: /Applications/Beekeeper Studio.app/Contents/MacOS/Beekeeper Studio is architecture: arm64";

        let arch = parse_architecture_from_stderr(stderr).unwrap();
        assert_eq!(arch, "arm64");
    }

    #[test]
    fn test_parse_architecture_from_stderr_x86() {
        let stderr = "input file /Applications/Dia.app/Contents/MacOS/Dia is not a fat file\nNon-fat file: /Applications/Dia.app/Contents/MacOS/Dia is architecture: x86_64";

        let arch = parse_architecture_from_stderr(stderr).unwrap();
        assert_eq!(arch, "x86_64");
    }

    #[test]
    fn test_parse_lipo_output_fat_binary() {
        let output = r#"Fat header in: /Applications/WezTerm.app/Contents/MacOS/wezterm-gui
fat_magic 0xcafebabe
nfat_arch 2
architecture x86_64
    cputype CPU_TYPE_X86_64
    cpusubtype CPU_SUBTYPE_X86_64_ALL
architecture arm64
    cputype CPU_TYPE_ARM64
    cpusubtype CPU_SUBTYPE_ARM64_ALL"#;

        let archs = parse_lipo_output(output).unwrap();
        assert_eq!(archs.len(), 2);
        assert_eq!(archs[0].cpu_type, "x86_64");
        assert_eq!(archs[1].cpu_type, "arm64");
    }
}
