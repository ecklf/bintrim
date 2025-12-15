use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;

#[derive(Debug, Clone)]
pub struct ArchInfo {
    pub cpu_type: String,
    pub size_bytes: u64,
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
        self.architectures.iter().any(|arch| arch.cpu_type == "x86_64")
    }

    pub fn has_arm64(&self) -> bool {
        self.architectures.iter().any(|arch| arch.cpu_type == "arm64")
    }

    pub fn x86_64_size_mb(&self) -> f64 {
        self.architectures
            .iter()
            .find(|arch| arch.cpu_type == "x86_64")
            .map(|arch| arch.size_bytes as f64 / 1024.0 / 1024.0)
            .unwrap_or(0.0)
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
                    let app_name = path.file_stem()
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
    let app_name = app_path
        .file_stem()?
        .to_str()?
        .to_string();

    // Find the binary inside Contents/MacOS/
    let macos_dir = app_path.join("Contents").join("MacOS");
    
    if !macos_dir.exists() {
        return None;
    }

    // Try to find the main binary (usually named the same as the app)
    let mut binary_path = macos_dir.join(&app_name);
    
    // If that doesn't exist, try to find any executable in MacOS directory
    if !binary_path.exists()
        && let Ok(entries) = fs::read_dir(&macos_dir) {
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

    if !output.status.success() {
        return None;
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    parse_lipo_output(&output_str)
}

fn parse_lipo_output(output: &str) -> Option<Vec<ArchInfo>> {
    let mut architectures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        
        // Look for architecture line
        if line.starts_with("architecture ") {
            let arch_name = line.strip_prefix("architecture ")?.trim().to_string();
            
            // Find the size line (should be a few lines down)
            let mut size_bytes = 0u64;
            for size_line in lines.iter().take(std::cmp::min(i + 10, lines.len())).skip(i + 1) {
                let size_line = size_line.trim();
                if size_line.starts_with("size ") {
                    // Extract size value
                    let parts: Vec<&str> = size_line.split_whitespace().collect();
                    if parts.len() >= 2
                        && let Ok(size) = parts[1].parse::<u64>() {
                            size_bytes = size;
                            break;
                        }
                }
            }
            
            architectures.push(ArchInfo {
                cpu_type: arch_name,
                size_bytes,
            });
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
        assert_eq!(archs[0].size_bytes, 9228032);
        assert_eq!(archs[1].cpu_type, "arm64");
        assert_eq!(archs[1].size_bytes, 8804432);
    }
}
