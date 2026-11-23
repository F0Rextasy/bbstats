//! Developer-facing CLI subcommands: `bbstats run`, `bbstats port`, and `bbstats launch`.

use crate::engine::actions::execute_safe_kill_many;
use crate::engine::collector::ProcessCollector;
use crate::engine::project_resolver::{sanitize_cmdline, ProjectResolver};
use crate::engine::win_nt::{current_pid, inspect_process_peb};
use crate::model::{ProcessKey, TransportProto};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliSubcommand {
    Run {
        cwd: Option<PathBuf>,
        program: String,
        args: Vec<String>,
    },
    Port {
        port: u16,
        kill: bool,
    },
    Launch {
        dir: PathBuf,
        list_only: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchJob {
    pub name: String,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

/// Parses `run`, `port`, or `launch` subcommands from argv (`args` excluding argv[0]).
/// Returns `Ok(None)` when the first arg is not one of those subcommands.
pub fn parse_subcommand(args: &[String]) -> Result<Option<CliSubcommand>, String> {
    let Some(first) = args.first().map(|s| s.as_str()) else {
        return Ok(None);
    };
    let rest = &args[1..];
    match first {
        "run" => parse_run_args(rest).map(Some),
        "port" => parse_port_args(rest).map(Some),
        "launch" => parse_launch_args(rest).map(Some),
        _ => Ok(None),
    }
}

fn parse_run_args(rest: &[String]) -> Result<CliSubcommand, String> {
    let mut cwd: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < rest.len() {
        let tok = &rest[i];
        if tok == "--" {
            i += 1;
            break;
        }
        if tok == "--cwd" || tok == "-C" {
            let val = rest
                .get(i + 1)
                .ok_or_else(|| format!("{tok} requires a directory path"))?;
            cwd = Some(PathBuf::from(val));
            i += 2;
            continue;
        }
        if let Some(val) = tok.strip_prefix("--cwd=") {
            if val.is_empty() {
                return Err("--cwd= requires a directory path".to_string());
            }
            cwd = Some(PathBuf::from(val));
            i += 1;
            continue;
        }
        if tok.starts_with('-') {
            return Err(format!("Unknown flag for 'run': {tok}"));
        }
        break;
    }
    let Some(program) = rest.get(i).cloned() else {
        return Err("Usage: bbstats run [--cwd DIR] -- <command> [args...]".to_string());
    };
    let args = rest[i + 1..].to_vec();
    Ok(CliSubcommand::Run { cwd, program, args })
}

fn parse_port_args(rest: &[String]) -> Result<CliSubcommand, String> {
    let mut port: Option<u16> = None;
    let mut kill = false;
    for tok in rest {
        if tok == "--kill" || tok == "-k" {
            kill = true;
        } else if tok.starts_with('-') {
            return Err(format!("Unknown flag for 'port': {tok}"));
        } else if port.is_none() {
            let p: u16 = tok
                .parse()
                .map_err(|_| format!("Invalid port number '{tok}' (expected 1..=65535)"))?;
            if p == 0 {
                return Err("Port must be between 1 and 65535".to_string());
            }
            port = Some(p);
        } else {
            return Err(format!("Unexpected extra argument for 'port': {tok}"));
        }
    }
    let Some(port) = port else {
        return Err("Usage: bbstats port <PORT> [--kill]".to_string());
    };
    Ok(CliSubcommand::Port { port, kill })
}

fn parse_launch_args(rest: &[String]) -> Result<CliSubcommand, String> {
    let mut dir: Option<PathBuf> = None;
    let mut list_only = false;
    for tok in rest {
        if tok == "--list" || tok == "-l" {
            list_only = true;
        } else if tok.starts_with('-') {
            return Err(format!("Unknown flag for 'launch': {tok}"));
        } else if dir.is_none() {
            dir = Some(PathBuf::from(tok));
        } else {
            return Err(format!("Unexpected extra argument for 'launch': {tok}"));
        }
    }
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir().map_err(|e| format!("Failed to read cwd: {e}"))?,
    };
    Ok(CliSubcommand::Launch { dir, list_only })
}

/// Runs a subcommand if `args` matches `run | port | launch`, returning `Some(exit_code)`.
pub fn try_handle_cli(args: &[String]) -> Option<i32> {
    match parse_subcommand(args) {
        Ok(None) => None,
        Ok(Some(cmd)) => Some(execute_subcommand(cmd)),
        Err(msg) => {
            eprintln!("{msg}");
            Some(2)
        }
    }
}

fn execute_subcommand(cmd: CliSubcommand) -> i32 {
    match cmd {
        CliSubcommand::Run { cwd, program, args } => run_direct(cwd.as_deref(), &program, &args),
        CliSubcommand::Port { port, kill } => inspect_port(port, kill),
        CliSubcommand::Launch { dir, list_only } => launch_project(&dir, list_only),
    }
}

fn spawn_inherited_command(program: &str, args: &[String], cwd: Option<&Path>) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    match cmd.spawn() {
        Ok(child) => Ok(child),
        #[cfg(target_os = "windows")]
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Resolve npm.cmd / pnpm.cmd / shell builtins when invoked without extension on Windows.
            let mut sh = Command::new("cmd.exe");
            sh.arg("/D").arg("/C").arg(program).args(args);
            sh.stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            if let Some(dir) = cwd {
                sh.current_dir(dir);
            }
            sh.spawn()
        }
        Err(e) => Err(e),
    }
}

fn run_direct(cwd: Option<&Path>, program: &str, args: &[String]) -> i32 {
    if let Some(dir) = cwd {
        if !dir.is_dir() {
            eprintln!("Working directory does not exist: {}", dir.display());
            return 2;
        }
    }
    match spawn_inherited_command(program, args, cwd) {
        Ok(mut child) => match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("Failed while waiting for '{program}': {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("Failed to start '{program}': {e}");
            1
        }
    }
}

fn inspect_port(port: u16, kill: bool) -> i32 {
    let mut collector = ProcessCollector::new();
    let tick = collector.collect_processes();
    let ports_map = collector.collect_ports();
    let mut resolver = ProjectResolver::new(current_pid());
    resolver.resolve_tick(&tick.processes, false, &|pid| {
        ports_map
            .get(&pid)
            .is_some_and(|info| info.ports.iter().any(|p| p.is_listening))
    });

    let mut matches: Vec<(ProcessKey, TransportProto, &str, String, String, String)> = Vec::new();
    for proc_info in &tick.processes {
        let Some(net) = ports_map.get(&proc_info.key.pid) else {
            continue;
        };
        let Some(binding) = net.ports.iter().find(|b| b.port == port && b.is_listening) else {
            continue;
        };
        let meta = resolver.cached_meta(&proc_info.key);
        let project = meta
            .map(|m| m.project_display_name.clone())
            .unwrap_or_else(|| "-".to_string());
        let job = meta
            .map(|m| m.job_title.clone())
            .unwrap_or_else(|| proc_info.image_name.clone());
        let cmdline = meta
            .and_then(|m| m.sanitized_cmdline.clone())
            .or_else(|| {
                inspect_process_peb(proc_info.key.pid)
                    .cmdline
                    .as_deref()
                    .map(sanitize_cmdline)
            })
            .unwrap_or_else(|| proc_info.image_name.clone());
        matches.push((
            proc_info.key,
            binding.protocol,
            &proc_info.image_name,
            project,
            job,
            cmdline,
        ));
    }

    if matches.is_empty() {
        println!("No listening process found on port {port}.");
        return 0;
    }

    for (key, proto, exe, project, job, cmdline) in &matches {
        let p_str = match proto {
            TransportProto::Tcp => "TCP",
            TransportProto::Udp => "UDP",
        };
        println!(
            "Port {port} ({p_str}) · PID {} ({exe}) · Project: {project} · Job: {job}\n  cmd: {cmdline}",
            key.pid
        );
    }

    if !kill {
        return 0;
    }

    let roots: Vec<ProcessKey> = matches.iter().map(|m| m.0).collect();
    let mut user_kills = HashMap::new();
    let killed = execute_safe_kill_many(&roots, &tick.processes, false, &mut user_kills);
    if killed.is_empty() {
        eprintln!("Refused or failed to terminate processes on port {port} (protected or access denied).");
        return 1;
    }
    let pids: Vec<String> = killed.iter().map(|k| k.pid.to_string()).collect();
    println!("Terminated PID(s): {}", pids.join(", "));
    0
}

pub fn discover_launch_jobs(dir: &Path) -> Result<(PathBuf, Vec<LaunchJob>), String> {
    if !dir.is_dir() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let toml_path = dir.join("bbstats.toml");
    if toml_path.is_file() {
        let content = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("Failed to read {}: {e}", toml_path.display()))?;
        let jobs = parse_bbstats_toml(&content, dir)?;
        if !jobs.is_empty() {
            return Ok((toml_path, jobs));
        }
    }

    let procfile_path = dir.join("Procfile");
    if procfile_path.is_file() {
        let content = std::fs::read_to_string(&procfile_path)
            .map_err(|e| format!("Failed to read {}: {e}", procfile_path.display()))?;
        let jobs = parse_procfile(&content, dir);
        if !jobs.is_empty() {
            return Ok((procfile_path, jobs));
        }
    }

    let pkg_path = dir.join("package.json");
    if pkg_path.is_file() {
        let content = std::fs::read_to_string(&pkg_path)
            .map_err(|e| format!("Failed to read {}: {e}", pkg_path.display()))?;
        let jobs = parse_package_json_scripts(&content, dir)?;
        if !jobs.is_empty() {
            return Ok((pkg_path, jobs));
        }
    }

    Err(format!(
        "No bbstats.toml, Procfile, or package.json scripts found in {}",
        dir.display()
    ))
}

fn resolve_job_cwd(base: &Path, raw: Option<&str>) -> PathBuf {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(rel) => {
            let p = PathBuf::from(rel);
            if p.is_absolute() {
                p
            } else {
                base.join(p)
            }
        }
        None => base.to_path_buf(),
    }
}

pub fn parse_bbstats_toml(content: &str, base_dir: &Path) -> Result<Vec<LaunchJob>, String> {
    let val: toml::Value =
        toml::from_str(content).map_err(|e| format!("Invalid bbstats.toml: {e}"))?;
    let mut out = Vec::new();
    let Some(jobs_val) = val.get("jobs") else {
        return Ok(out);
    };

    if let Some(tbl) = jobs_val.as_table() {
        for (name, entry) in tbl {
            if let Some(cmd_str) = entry.as_str() {
                let trimmed = cmd_str.trim();
                if !trimmed.is_empty() {
                    out.push(LaunchJob {
                        name: name.clone(),
                        command: trimmed.to_string(),
                        cwd: Some(base_dir.to_path_buf()),
                    });
                }
            } else if let Some(sub) = entry.as_table() {
                let cmd_str = sub
                    .get("command")
                    .or_else(|| sub.get("cmd"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .unwrap_or("");
                if !cmd_str.is_empty() {
                    let cwd = resolve_job_cwd(
                        base_dir,
                        sub.get("cwd")
                            .or_else(|| sub.get("working_dir"))
                            .and_then(|v| v.as_str()),
                    );
                    out.push(LaunchJob {
                        name: name.clone(),
                        command: cmd_str.to_string(),
                        cwd: Some(cwd),
                    });
                }
            }
        }
    } else if let Some(arr) = jobs_val.as_array() {
        for entry in arr {
            let Some(sub) = entry.as_table() else {
                continue;
            };
            let name = sub
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let cmd_str = sub
                .get("command")
                .or_else(|| sub.get("cmd"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if !name.is_empty() && !cmd_str.is_empty() {
                let cwd = resolve_job_cwd(
                    base_dir,
                    sub.get("cwd")
                        .or_else(|| sub.get("working_dir"))
                        .and_then(|v| v.as_str()),
                );
                out.push(LaunchJob {
                    name: name.to_string(),
                    command: cmd_str.to_string(),
                    cwd: Some(cwd),
                });
            }
        }
    }
    Ok(out)
}

pub fn parse_procfile(content: &str, base_dir: &Path) -> Vec<LaunchJob> {
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, cmd)) = trimmed.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let cmd = cmd.trim();
        if !name.is_empty() && !cmd.is_empty() {
            out.push(LaunchJob {
                name: name.to_string(),
                command: cmd.to_string(),
                cwd: Some(base_dir.to_path_buf()),
            });
        }
    }
    out
}

pub fn parse_package_json_scripts(
    content: &str,
    base_dir: &Path,
) -> Result<Vec<LaunchJob>, String> {
    let v: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("Invalid package.json: {e}"))?;
    let mut out = Vec::new();
    let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) else {
        return Ok(out);
    };
    let runner = if base_dir.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if base_dir.join("yarn.lock").is_file() {
        "yarn"
    } else if base_dir.join("bun.lockb").is_file() || base_dir.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    };
    for (name, _) in scripts {
        if !name.trim().is_empty() {
            out.push(LaunchJob {
                name: name.clone(),
                command: format!("{runner} run {name}"),
                cwd: Some(base_dir.to_path_buf()),
            });
        }
    }
    Ok(out)
}

fn spawn_shell_job(job: &LaunchJob) -> std::io::Result<Child> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd.exe");
        c.arg("/D").arg("/C").arg(&job.command);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&job.command);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(ref cwd) = job.cwd {
        cmd.current_dir(cwd);
    }
    cmd.spawn()
}

fn launch_project(dir: &Path, list_only: bool) -> i32 {
    let (source, jobs) = match discover_launch_jobs(dir) {
        Ok(res) => res,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    println!("Discovered {} job(s) from {}", jobs.len(), source.display());
    for job in &jobs {
        println!("  [{}] {}", job.name, job.command);
    }
    if list_only {
        return 0;
    }

    let mut running: Vec<(String, Child)> = Vec::new();
    for job in jobs {
        match spawn_shell_job(&job) {
            Ok(child) => {
                println!("[{}] started (PID {})", job.name, child.id());
                running.push((job.name, child));
            }
            Err(e) => {
                eprintln!("[{}] failed to start: {e}", job.name);
                for (_, mut child) in running {
                    let _ = child.kill();
                }
                return 1;
            }
        }
    }

    let mut exit_code = 0;
    for (name, mut child) in running {
        match child.wait() {
            Ok(status) => {
                let code = status.code().unwrap_or(1);
                println!("[{name}] exited with code {code}");
                if code != 0 && exit_code == 0 {
                    exit_code = code;
                }
            }
            Err(e) => {
                eprintln!("[{name}] wait error: {e}");
                if exit_code == 0 {
                    exit_code = 1;
                }
            }
        }
    }
    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bbstats_cli_{name}_{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_subcommand_handles_run_port_and_launch() {
        let a = |xs: &[&str]| xs.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();

        assert_eq!(parse_subcommand(&a(&["--help"])), Ok(None));

        let run = parse_subcommand(&a(&["run", "--cwd", "api", "--", "npm", "run", "dev"])).unwrap();
        assert_eq!(
            run,
            Some(CliSubcommand::Run {
                cwd: Some(PathBuf::from("api")),
                program: "npm".into(),
                args: a(&["run", "dev"]),
            })
        );

        let port = parse_subcommand(&a(&["port", "3000", "--kill"])).unwrap();
        assert_eq!(
            port,
            Some(CliSubcommand::Port {
                port: 3000,
                kill: true,
            })
        );
        assert!(parse_subcommand(&a(&["port", "0"])).is_err());
        assert!(parse_subcommand(&a(&["run", "--bogus"])).is_err());

        let launch = parse_subcommand(&a(&["launch", ".", "--list"])).unwrap();
        assert_eq!(
            launch,
            Some(CliSubcommand::Launch {
                dir: PathBuf::from("."),
                list_only: true,
            })
        );
    }

    #[test]
    fn discover_launch_jobs_parses_bbstats_toml_procfile_and_package_json() {
        let dir = temp_dir("launch");

        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"demo","scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let (src, pkg_jobs) = discover_launch_jobs(&dir).unwrap();
        assert!(src.ends_with("package.json"));
        assert!(pkg_jobs.iter().any(|j| j.name == "dev" && j.command == "pnpm run dev"));

        std::fs::write(
            dir.join("Procfile"),
            "# comment\nweb: node server.js\nworker: python worker.py\n",
        )
        .unwrap();
        let (src2, proc_jobs) = discover_launch_jobs(&dir).unwrap();
        assert!(src2.ends_with("Procfile"));
        assert_eq!(proc_jobs.len(), 2);
        assert_eq!(proc_jobs[0].name, "web");
        assert_eq!(proc_jobs[1].command, "python worker.py");

        std::fs::write(
            dir.join("bbstats.toml"),
            "[jobs]\napi = \"cargo run\"\n[jobs.worker]\ncmd = \"python bot.py\"\ncwd = \"sub\"\n",
        )
        .unwrap();
        let (src3, toml_jobs) = discover_launch_jobs(&dir).unwrap();
        assert!(src3.ends_with("bbstats.toml"));
        assert_eq!(toml_jobs.len(), 2);
        let worker = toml_jobs.iter().find(|j| j.name == "worker").unwrap();
        assert_eq!(worker.command, "python bot.py");
        assert_eq!(worker.cwd, Some(dir.join("sub")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
