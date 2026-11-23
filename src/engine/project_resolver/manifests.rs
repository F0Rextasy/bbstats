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
                        .filter(|d| d.chars().count() <= 32)
                })
                .unwrap_or_else(|| dir_name_or(dir, "node-project"));
            let (display, root) = if let Some(ws_root) = check_monorepo_parent_root(dir) {
                let ws_name = monorepo_root_name(&ws_root).unwrap_or(pkg_name);
                (ws_name, ws_root)
            } else {
                (pkg_name, dir.to_path_buf())
            };
            let git_branch = read_git_branch_ancestors(&root);
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
                let root = dir.to_path_buf();
                let git_branch = read_git_branch_ancestors(&root);
                return Some(CachedRoot {
                    project_name: name,
                    root_path: root,
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
                let root = dir.to_path_buf();
                let git_branch = read_git_branch_ancestors(&root);
                return Some(CachedRoot {
                    project_name: display,
                    root_path: root,
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
                    let root = dir.to_path_buf();
                    let git_branch = read_git_branch_ancestors(&root);
                    return Some(CachedRoot {
                        project_name: title,
                        root_path: root,
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
            let root = dir.to_path_buf();
            let git_branch = read_git_branch_ancestors(&root);
            return Some(CachedRoot {
                project_name: name,
                root_path: root,
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
            let name = extract_go_module_name(&text)
                .unwrap_or_else(|| dir_name_or(dir, "go-module"));
            let root = dir.to_path_buf();
            let git_branch = read_git_branch_ancestors(&root);
            return Some(CachedRoot {
                project_name: name,
                root_path: root,
                git_branch,
                source: InferenceSource::ManifestGoMod,
                cached_at: Instant::now(),
            });
        }
    }

    // 7. .NET or JVM (deterministic: .sln > .csproj > JVM, sorted alphabetically)
    if let Ok(entries) = fs::read_dir(dir) {
        let mut slns: Vec<String> = Vec::new();
        let mut csprojs: Vec<String> = Vec::new();
        let mut has_jvm = false;
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            let fname = entry.file_name();
            let s = fname.to_string_lossy();
            let low = s.to_ascii_lowercase();
            if low.ends_with(".sln") {
                if let Some(stem) = Path::new(s.as_ref()).file_stem().and_then(|x| x.to_str()) {
                    slns.push(stem.to_string());
                }
            } else if low.ends_with(".csproj") {
                if let Some(stem) = Path::new(s.as_ref()).file_stem().and_then(|x| x.to_str()) {
                    csprojs.push(stem.to_string());
                }
            } else if matches!(low.as_str(), "pom.xml" | "build.gradle" | "build.gradle.kts") {
                has_jvm = true;
            }
        }
        slns.sort_by_key(|a| a.to_ascii_lowercase());
        csprojs.sort_by_key(|a| a.to_ascii_lowercase());
        if let Some(name) = slns.into_iter().next().or_else(|| csprojs.into_iter().next()) {
            let root = dir.to_path_buf();
            let git_branch = read_git_branch_ancestors(&root);
            return Some(CachedRoot {
                project_name: name,
                root_path: root,
                git_branch,
                source: InferenceSource::ManifestDotNetOrJvm,
                cached_at: Instant::now(),
            });
        }
        if has_jvm {
            let root = dir.to_path_buf();
            let git_branch = read_git_branch_ancestors(&root);
            return Some(CachedRoot {
                project_name: dir_name_or(dir, "jvm-app"),
                root_path: root,
                git_branch,
                source: InferenceSource::ManifestDotNetOrJvm,
                cached_at: Instant::now(),
            });
        }
    }

    // 8. .git or .venv root
    if dir.join(".git").exists() || dir.join(".venv").is_dir() {
        let root = dir.to_path_buf();
        let git_branch = read_git_branch_ancestors(&root);
        return Some(CachedRoot {
            project_name: dir_name_or(dir, "project"),
            root_path: root,
            git_branch,
            source: InferenceSource::GitRoot,
            cached_at: Instant::now(),
        });
    }

    None
}

fn extract_go_module_name(text: &str) -> Option<String> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("module "))?;
    let without_comment = line.split("//").next().unwrap_or(line).trim();
    let cleaned = without_comment.trim_matches(['"', '\'']).trim_end_matches('/');
    let mut segs = cleaned.rsplit('/');
    let last = segs.next()?.trim();
    if last.len() >= 2
        && last.starts_with('v')
        && last[1..].bytes().all(|b| b.is_ascii_digit())
    {
        if let Some(prev) = segs.next().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(prev.to_string());
        }
    }
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

fn json_nonempty_str(val: &serde_json::Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn monorepo_root_name(ws_root: &Path) -> Option<String> {
    if let Ok(txt) = fs::read_to_string(ws_root.join("package.json")) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
            if let Some(name) = json_nonempty_str(&val, "name") {
                return Some(clean_pkg_name(&name));
            }
        }
    }
    Some(dir_name_or(ws_root, "workspace"))
}

pub(super) fn check_monorepo_parent_root(dir: &Path) -> Option<PathBuf> {
    let mut cur = dir.parent()?;
    for _ in 0..3 {
        if cur.join("pnpm-workspace.yaml").is_file() {
            return Some(cur.to_path_buf());
        }
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
            if !clean.is_empty() && clean.chars().count() <= 40 {
                return Some(clean.to_string());
            }
        }
    }
    None
}

pub(super) fn read_git_branch_ancestors(start: &Path) -> Option<String> {
    let mut cur = Some(start);
    for _ in 0..4 {
        let dir = cur?;
        if let Some(branch) = read_git_branch(dir) {
            return Some(branch);
        }
        cur = dir.parent();
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
    } else if let Some(sym) = trimmed.strip_prefix("ref: ") {
        sym.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
    } else if trimmed.len() >= 7 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(trimmed[..7].to_string())
    } else {
        None
    }
}

pub(super) fn extract_yaml_top_name(yaml: &str) -> Option<String> {
    for line in yaml.lines().take(20) {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let val_raw = rest.trim();
            let val = if let Some(q) = val_raw.strip_prefix('"') {
                q.split('"').next().unwrap_or("")
            } else if let Some(q) = val_raw.strip_prefix('\'') {
                q.split('\'').next().unwrap_or("")
            } else {
                val_raw.split('#').next().unwrap_or(val_raw).trim()
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_yaml_top_name_ignores_indented_keys_and_strips_comments() {
        assert_eq!(
            extract_yaml_top_name("name: my-stack # prod\nservices:\n  db:\n"),
            Some("my-stack".to_string())
        );
        assert_eq!(
            extract_yaml_top_name("name: 'quoted-stack' # comment\n"),
            Some("quoted-stack".to_string())
        );
        assert_eq!(
            extract_yaml_top_name("services:\n  db:\n    image: pg\nnetworks:\n  default:\n    name: custom_net\n"),
            None
        );
    }

    #[test]
    fn extract_readme_heading_counts_unicode_chars_not_bytes() {
        assert_eq!(extract_readme_heading("# MyApp (`v1`)\n"), Some("MyApp".into()));
        assert_eq!(
            extract_readme_heading("# Ödeme ve Fatura Yönetim Servisi\n"),
            Some("Ödeme ve Fatura Yönetim Servisi".into())
        );
        assert_eq!(extract_readme_heading("## Subheading\n"), None);
    }

    #[test]
    fn extract_go_module_name_skips_major_version_suffix() {
        assert_eq!(
            extract_go_module_name("module github.com/acme/payments/v2 // prod\n"),
            Some("payments".into())
        );
        assert_eq!(
            extract_go_module_name("module github.com/acme/api\n"),
            Some("api".into())
        );
    }

    #[test]
    fn monorepo_git_branch_and_pnpm_workspace_and_dotnet_priority() {
        let base = std::env::temp_dir().join(format!("bbstats_manifest_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        // pnpm monorepo + git branch in repo root
        let repo = base.join("acme-mono");
        let app = repo.join("apps").join("web");
        fs::create_dir_all(app.as_path()).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/feat/ui\n").unwrap();
        fs::write(repo.join("pnpm-workspace.yaml"), "packages:\n  - 'apps/*'\n").unwrap();
        fs::write(app.join("package.json"), r#"{"name": "web"}"#).unwrap();

        let root = inspect_directory_manifests(&app).expect("manifest");
        assert_eq!(root.project_name, "acme-mono");
        assert_eq!(root.root_path, repo);
        assert_eq!(root.git_branch.as_deref(), Some("feat/ui"));

        // read_git_branch edge cases
        let tag_dir = base.join("tag-repo");
        fs::create_dir_all(tag_dir.join(".git")).unwrap();
        fs::write(tag_dir.join(".git").join("HEAD"), "ref: refs/tags/v1.0.0\n").unwrap();
        assert_eq!(read_git_branch(&tag_dir).as_deref(), Some("v1.0.0"));
        fs::write(tag_dir.join(".git").join("HEAD"), "a1b2c3d4e5f678901234567890abcdef12345678\n").unwrap();
        assert_eq!(read_git_branch(&tag_dir).as_deref(), Some("a1b2c3d"));

        // .sln takes priority over .csproj deterministically
        let net_dir = base.join("net-app");
        fs::create_dir_all(&net_dir).unwrap();
        fs::write(net_dir.join("Api.csproj"), "<Project/>").unwrap();
        fs::write(net_dir.join("App.sln"), "Microsoft Visual Studio Solution").unwrap();
        let net = inspect_directory_manifests(&net_dir).expect("dotnet");
        assert_eq!(net.project_name, "App");

        let _ = fs::remove_dir_all(&base);
    }
}
