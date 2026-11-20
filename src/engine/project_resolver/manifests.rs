use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{CachedRoot, InferenceSource};

pub fn tail_last_4kb_line(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let start = len.saturating_sub(4096);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start).min(4096) as usize);
    file.take(4096).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let last = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    let truncated: String = last.chars().take(120).collect();
    Some(truncated)
}


/// Inspects a single directory for project manifests in priority order:
/// `package.json`, `pyproject.toml`, `Cargo.toml`, `README.md`, `compose.yaml`, `go.mod`, `.sln`/`.csproj`/`pom.xml`, `.git`.
pub fn inspect_directory_manifests(dir: &Path) -> Option<CachedRoot> {
    if !dir.is_dir() {
        return None;
    }
    let git_branch = read_git_branch(dir);

    // 1. package.json (with monorepo parent workspace check)
    let pkg_json = dir.join("package.json");
    if pkg_json.is_file() {
        if let Ok(text) = fs::read_to_string(&pkg_json) {
            let pkg_val = serde_json::from_str::<serde_json::Value>(&text).ok();
            let pkg_name = pkg_val
                .as_ref()
                .and_then(|v| json_nonempty_str(v, "name"))
                .map(|n| clean_pkg_name(&n))
                .or_else(|| {
                    pkg_val
                        .as_ref()
                        .and_then(|v| json_nonempty_str(v, "description"))
                        .filter(|d| d.len() <= 32)
                })
                .unwrap_or_else(|| dir_name_or(dir, "node-project"));
            let (display, root) = if let Some(ws_root) = check_monorepo_parent_root(dir) {
                let ws_name = monorepo_root_name(&ws_root).unwrap_or(pkg_name);
                (ws_name, ws_root)
            } else {
                (pkg_name, dir.to_path_buf())
            };
            return Some(CachedRoot {
                project_name: display,
                root_path: root,
                git_branch,
                source: InferenceSource::ManifestPackageJson,
                cached_at: Instant::now(),
            });
        }
    }

    // 2. pyproject.toml
    let pyproject = dir.join("pyproject.toml");
    if pyproject.is_file() {
        if let Ok(text) = fs::read_to_string(&pyproject) {
            if let Ok(doc) = text.parse::<toml::Value>() {
                let name = doc
                    .get("project")
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        doc.get("tool")
                            .and_then(|t| t.get("poetry"))
                            .and_then(|p| p.get("name"))
                            .and_then(|v| v.as_str())
                    })
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| dir_name_or(dir, "python-project"));
                return Some(CachedRoot {
                    project_name: name,
                    root_path: dir.to_path_buf(),
                    git_branch,
                    source: InferenceSource::ManifestPyProject,
                    cached_at: Instant::now(),
                });
            }
        }
    }

    // 3. Cargo.toml
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        if let Ok(text) = fs::read_to_string(&cargo_toml) {
            if let Ok(doc) = text.parse::<toml::Value>() {
                let pkg_name = doc
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| dir_name_or(dir, "rust-project"));
                let display = check_cargo_workspace_parent(dir, &pkg_name).unwrap_or(pkg_name);
                return Some(CachedRoot {
                    project_name: display,
                    root_path: dir.to_path_buf(),
                    git_branch,
                    source: InferenceSource::ManifestCargoToml,
                    cached_at: Instant::now(),
                });
            }
        }
    }

    // 4. README.md / README first `# Heading`
    for readme in ["README.md", "readme.md", "README.txt"] {
        let p = dir.join(readme);
        if p.is_file() {
            if let Ok(text) = fs::read_to_string(&p) {
                if let Some(title) = extract_readme_heading(&text) {
                    return Some(CachedRoot {
                        project_name: title,
                        root_path: dir.to_path_buf(),
                        git_branch,
                        source: InferenceSource::ManifestReadme,
                        cached_at: Instant::now(),
                    });
                }
            }
        }
    }

    // 5. compose.yaml / docker-compose.yml
    for compose in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ] {
        let p = dir.join(compose);
        if p.is_file() {
            let name = fs::read_to_string(&p)
                .ok()
                .and_then(|t| extract_yaml_top_name(&t))
                .unwrap_or_else(|| dir_name_or(dir, "compose-stack"));
            return Some(CachedRoot {
                project_name: name,
                root_path: dir.to_path_buf(),
                git_branch,
                source: InferenceSource::ManifestCompose,
                cached_at: Instant::now(),
            });
        }
    }

    // 6. go.mod
    let gomod = dir.join("go.mod");
    if gomod.is_file() {
        if let Ok(text) = fs::read_to_string(&gomod) {
            let name = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("module "))
                .and_then(|m| m.trim().rsplit('/').next())
                .map(|s| s.to_string())
                .unwrap_or_else(|| dir_name_or(dir, "go-module"));
            return Some(CachedRoot {
                project_name: name,
                root_path: dir.to_path_buf(),
                git_branch,
                source: InferenceSource::ManifestGoMod,
                cached_at: Instant::now(),
            });
        }
    }

    // 7. .NET or JVM
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let s = fname.to_string_lossy();
            if s.ends_with(".sln") || s.ends_with(".csproj") {
                let stem = Path::new(s.as_ref())
                    .file_stem()
                    .and_then(|x| x.to_str())
                    .unwrap_or("dotnet-app")
                    .to_string();
                return Some(CachedRoot {
                    project_name: stem,
                    root_path: dir.to_path_buf(),
                    git_branch,
                    source: InferenceSource::ManifestDotNetOrJvm,
                    cached_at: Instant::now(),
                });
            }
            if s == "pom.xml" || s == "build.gradle" || s == "build.gradle.kts" {
                return Some(CachedRoot {
                    project_name: dir_name_or(dir, "jvm-app"),
                    root_path: dir.to_path_buf(),
                    git_branch,
                    source: InferenceSource::ManifestDotNetOrJvm,
                    cached_at: Instant::now(),
                });
            }
        }
    }

    // 8. .git or .venv root
    if dir.join(".git").exists() || dir.join(".venv").is_dir() {
        return Some(CachedRoot {
            project_name: dir_name_or(dir, "project"),
            root_path: dir.to_path_buf(),
            git_branch,
            source: InferenceSource::GitRoot,
            cached_at: Instant::now(),
        });
    }

    None
}

fn json_nonempty_str(val: &serde_json::Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn monorepo_root_name(ws_root: &Path) -> Option<String> {
    let txt = fs::read_to_string(ws_root.join("package.json")).ok()?;
    let val = serde_json::from_str::<serde_json::Value>(&txt).ok()?;
    Some(
        json_nonempty_str(&val, "name")
            .map(|n| clean_pkg_name(&n))
            .unwrap_or_else(|| dir_name_or(ws_root, "workspace")),
    )
}

pub(super) fn check_monorepo_parent_root(dir: &Path) -> Option<PathBuf> {
    let mut cur = dir.parent()?;
    for _ in 0..3 {
        let p = cur.join("package.json");
        if p.is_file() {
            if let Ok(txt) = fs::read_to_string(&p) {
                if serde_json::from_str::<serde_json::Value>(&txt)
                    .ok()
                    .is_some_and(|v| v.get("workspaces").is_some())
                {
                    return Some(cur.to_path_buf());
                }
            }
        }
        cur = cur.parent()?;
    }
    None
}

pub(super) fn check_cargo_workspace_parent(dir: &Path, child_name: &str) -> Option<String> {
    let mut cur = dir.parent()?;
    for _ in 0..2 {
        let p = cur.join("Cargo.toml");
        if p.is_file() {
            if let Ok(txt) = fs::read_to_string(&p) {
                if let Ok(doc) = txt.parse::<toml::Value>() {
                    if doc.get("workspace").is_some() {
                        let ws_name = dir_name_or(cur, "workspace");
                        if ws_name != child_name {
                            return Some(format!("{ws_name} / {child_name}"));
                        }
                    }
                }
            }
        }
        cur = cur.parent()?;
    }
    None
}

pub(super) fn extract_readme_heading(text: &str) -> Option<String> {
    for line in text.lines().take(15) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let base = rest.split("(`").next().unwrap_or(rest);
            let clean = base.trim().trim_matches('`').trim();
            if !clean.is_empty() && clean.len() <= 40 {
                return Some(clean.to_string());
            }
        }
    }
    None
}

pub(super) fn read_git_branch(dir: &Path) -> Option<String> {
    let git_path = dir.join(".git");
    let head_path = if git_path.is_dir() {
        git_path.join("HEAD")
    } else if git_path.is_file() {
        let content = fs::read_to_string(&git_path).ok()?;
        let rel = content.trim().strip_prefix("gitdir:")?.trim();
        let resolved = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            dir.join(rel)
        };
        resolved.join("HEAD")
    } else {
        return None;
    };

    let head = fs::read_to_string(head_path).ok()?;
    let trimmed = head.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else if trimmed.chars().count() >= 7 {
        Some(trimmed.chars().take(7).collect())
    } else {
        None
    }
}

pub(super) fn extract_yaml_top_name(yaml: &str) -> Option<String> {
    for line in yaml.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let val = rest.trim().trim_matches(['"', '\'']);
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

pub(super) fn clean_pkg_name(raw: &str) -> String {
    raw.trim_start_matches('@').to_string()
}

pub(super) fn dir_name_or(dir: &Path, fallback: &str) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

