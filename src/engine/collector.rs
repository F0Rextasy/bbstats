//! Low-overhead process, CPU/RAM/disk, and listening-port collector.

use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::ffi::c_void;
use std::time::Instant;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
#[cfg(target_os = "windows")]
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID,
    MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_LISTEN, MIB_UDP6ROW_OWNER_PID,
    MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID, MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    UDP_TABLE_OWNER_PID,
};
#[cfg(target_os = "windows")]
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use crate::engine::win_nt::{
    current_system_time_100ns, enumerate_processes, logical_core_count, query_system_memory,
    query_system_times_100ns, RawProcessEntry,
};
use crate::model::{BindScope, PortBinding, ProcessKey, TransportProto};

#[derive(Debug, Clone, Copy)]
struct PrevSample {
    cpu_time_100ns: i64,
    seen_tick: u64,
}

#[derive(Debug, Default, Clone)]
pub struct PidNetworkInfo {
    pub ports: Vec<PortBinding>,
}

/// Snapshot of a single process's raw fields + computed dynamic metrics for one tick.
#[derive(Debug, Clone)]
pub struct CollectedProcess {
    pub key: ProcessKey,
    pub parent_pid: u32,
    pub session_id: u32,
    pub image_name: String,
    pub cpu_percent: f32,
    pub private_ram_bytes: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CollectedTick {
    pub system_cpu_percent: f32,
    pub system_ram_used_bytes: u64,
    pub system_ram_total_bytes: u64,
    pub processes: Vec<CollectedProcess>,
}

pub struct ProcessCollector {
    scratch_buf: Vec<u64>,
    raw_entries: Vec<RawProcessEntry>,
    #[cfg(target_os = "windows")]
    net_buf: Vec<u64>,
    prev_samples: HashMap<ProcessKey, PrevSample>,
    tick_id: u64,
    last_tick: Option<Instant>,
    prev_sys_times: Option<(u64, u64, u64)>,
    logical_cores: u32,
}

impl ProcessCollector {
    pub fn new() -> Self {
        Self {
            scratch_buf: Vec::with_capacity((768 * 1024) / 8),
            raw_entries: Vec::with_capacity(512),
            #[cfg(target_os = "windows")]
            net_buf: vec![0u64; 8192],
            prev_samples: HashMap::with_capacity(512),
            tick_id: 0,
            last_tick: None,
            prev_sys_times: query_system_times_100ns(),
            logical_cores: logical_core_count(),
        }
    }

    /// Collects all running processes and computes CPU% and private RAM.
    pub fn collect_processes(&mut self) -> CollectedTick {
        let now = Instant::now();
        let wall_100ns = self
            .last_tick
            .map(|prev| (now.duration_since(prev).as_secs_f64() * 10_000_000.0).max(1.0))
            .unwrap_or(1.0);
        let has_prev_tick = self.last_tick.is_some();
        self.last_tick = Some(now);
        self.tick_id = self.tick_id.wrapping_add(1);
        let tick_id = self.tick_id;

        enumerate_processes(&mut self.scratch_buf, &mut self.raw_entries);

        let now_filetime_100ns = current_system_time_100ns();
        let norm_denom = wall_100ns * (self.logical_cores as f64);

        let mut processes = Vec::with_capacity(self.raw_entries.len());

        for raw in &self.raw_entries {
            let key = ProcessKey::new(raw.pid, raw.create_time_100ns);
            let sample = PrevSample {
                cpu_time_100ns: raw.cpu_time_100ns,
                seen_tick: tick_id,
            };
            let cpu_percent = match self.prev_samples.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let prev = *e.get();
                    e.insert(sample);
                    if has_prev_tick {
                        let d_cpu = raw
                            .cpu_time_100ns
                            .saturating_sub(prev.cpu_time_100ns)
                            .max(0) as f64;
                        ((d_cpu / norm_denom) * 100.0).clamp(0.0, 100.0) as f32
                    } else {
                        0.0
                    }
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(sample);
                    0.0
                }
            };

            let uptime_secs = if raw.create_time_100ns > 0 && now_filetime_100ns > raw.create_time_100ns {
                ((now_filetime_100ns - raw.create_time_100ns) / 10_000_000) as u64
            } else {
                0
            };

            processes.push(CollectedProcess {
                key,
                parent_pid: raw.parent_pid,
                session_id: raw.session_id,
                image_name: raw.image_name.clone(),
                cpu_percent,
                private_ram_bytes: raw.private_ram_bytes,
                uptime_secs,
            });
        }

        self.prev_samples.retain(|_, v| v.seen_tick == tick_id);

        let system_cpu_percent = self.compute_system_cpu(&processes);
        let (system_ram_used_bytes, system_ram_total_bytes) = query_system_memory();

        CollectedTick {
            system_cpu_percent,
            system_ram_used_bytes,
            system_ram_total_bytes,
            processes,
        }
    }

    fn compute_system_cpu(&mut self, processes: &[CollectedProcess]) -> f32 {
        let Some(cur) = query_system_times_100ns() else {
            return processes
                .iter()
                .map(|p| p.cpu_percent)
                .sum::<f32>()
                .clamp(0.0, 100.0);
        };
        let cpu = if let Some(prev) = self.prev_sys_times {
            let d_idle = cur.0.saturating_sub(prev.0) as f64;
            let d_kernel = cur.1.saturating_sub(prev.1) as f64;
            let d_user = cur.2.saturating_sub(prev.2) as f64;
            let total = d_kernel + d_user;
            if total > 0.0 {
                let busy = (total - d_idle).max(0.0);
                ((busy / total) * 100.0).clamp(0.0, 100.0) as f32
            } else {
                0.0
            }
        } else {
            0.0
        };
        self.prev_sys_times = Some(cur);
        cpu
    }

    /// Collects listening TCP/UDP ports and established TCP counts per PID.
    pub fn collect_ports(&mut self) -> HashMap<u32, PidNetworkInfo> {
        #[cfg(target_os = "windows")]
        {
            let mut by_pid: HashMap<u32, PidNetworkInfo> = HashMap::with_capacity(64);
            self.collect_tcp_v4(&mut by_pid);
            self.collect_tcp_v6(&mut by_pid);
            self.collect_udp_v4(&mut by_pid);
            self.collect_udp_v6(&mut by_pid);

            for info in by_pid.values_mut() {
                info.ports.sort_by_key(|p| (p.port, matches!(p.protocol, TransportProto::Udp)));
            }
            by_pid
        }
        #[cfg(not(target_os = "windows"))]
        {
            let Ok(output) = std::process::Command::new("lsof")
                .args(["-iTCP", "-iUDP", "-P", "-n", "-F", "pPnT"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
            else {
                return HashMap::new();
            };
            if !output.status.success() {
                return HashMap::new();
            }
            parse_lsof_ports(&String::from_utf8_lossy(&output.stdout))
        }
    }

    #[cfg(target_os = "windows")]
    fn collect_tcp_v4(&mut self, by_pid: &mut HashMap<u32, PidNetworkInfo>) {
        let Some(rows) = query_tcp_v4_rows(&mut self.net_buf) else {
            return;
        };
        for row in rows {
            let pid = row.dwOwningPid;
            if pid == 0 {
                continue;
            }
            if row.dwState == MIB_TCP_STATE_LISTEN.0 as u32 {
                let port = u16::from_be(row.dwLocalPort as u16);
                if port == 0 {
                    continue;
                }
                let scope = classify_ipv4_scope(row.dwLocalAddr);
                push_port_unique(
                    by_pid.entry(pid).or_default(),
                    PortBinding {
                        port,
                        protocol: TransportProto::Tcp,
                        scope,
                        is_listening: true,
                        suggested_url: suggest_url_for_port(port),
                    },
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn collect_tcp_v6(&mut self, by_pid: &mut HashMap<u32, PidNetworkInfo>) {
        let Some(rows) = query_tcp_v6_rows(&mut self.net_buf) else {
            return;
        };
        for row in rows {
            let pid = row.dwOwningPid;
            if pid == 0 {
                continue;
            }
            if row.dwState == MIB_TCP_STATE_LISTEN.0 as u32 {
                let port = u16::from_be(row.dwLocalPort as u16);
                if port == 0 {
                    continue;
                }
                let scope = classify_ipv6_scope(&row.ucLocalAddr);
                push_port_unique(
                    by_pid.entry(pid).or_default(),
                    PortBinding {
                        port,
                        protocol: TransportProto::Tcp,
                        scope,
                        is_listening: true,
                        suggested_url: suggest_url_for_port(port),
                    },
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn collect_udp_v4(&mut self, by_pid: &mut HashMap<u32, PidNetworkInfo>) {
        if !query_ip_table(&mut self.net_buf, |ptr, sz| unsafe {
            GetExtendedUdpTable(
                Some(ptr),
                sz,
                false,
                AF_INET.0 as u32,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }) {
            return;
        }

        let byte_len = self.net_buf.len() * std::mem::size_of::<u64>();
        if byte_len < std::mem::size_of::<MIB_UDPTABLE_OWNER_PID>() {
            return;
        }
        let table = unsafe { &*(self.net_buf.as_ptr() as *const MIB_UDPTABLE_OWNER_PID) };
        let count = table.dwNumEntries as usize;
        let rows_offset = std::mem::offset_of!(MIB_UDPTABLE_OWNER_PID, table);
        let Some(rows_bytes) = count.checked_mul(std::mem::size_of::<MIB_UDPROW_OWNER_PID>()) else {
            return;
        };
        if rows_offset.saturating_add(rows_bytes) > byte_len {
            return;
        }
        let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), count) };

        for row in rows {
            let pid = row.dwOwningPid;
            let port = u16::from_be(row.dwLocalPort as u16);
            if pid == 0 || port == 0 || port >= 49152 {
                continue;
            }
            push_port_unique(
                by_pid.entry(pid).or_default(),
                PortBinding {
                    port,
                    protocol: TransportProto::Udp,
                    scope: classify_ipv4_scope(row.dwLocalAddr),
                    is_listening: true,
                    suggested_url: None,
                },
            );
        }
    }

    #[cfg(target_os = "windows")]
    fn collect_udp_v6(&mut self, by_pid: &mut HashMap<u32, PidNetworkInfo>) {
        if !query_ip_table(&mut self.net_buf, |ptr, sz| unsafe {
            GetExtendedUdpTable(
                Some(ptr),
                sz,
                false,
                AF_INET6.0 as u32,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }) {
            return;
        }

        let byte_len = self.net_buf.len() * std::mem::size_of::<u64>();
        if byte_len < std::mem::size_of::<MIB_UDP6TABLE_OWNER_PID>() {
            return;
        }
        let table = unsafe { &*(self.net_buf.as_ptr() as *const MIB_UDP6TABLE_OWNER_PID) };
        let count = table.dwNumEntries as usize;
        let rows_offset = std::mem::offset_of!(MIB_UDP6TABLE_OWNER_PID, table);
        let Some(rows_bytes) = count.checked_mul(std::mem::size_of::<MIB_UDP6ROW_OWNER_PID>()) else {
            return;
        };
        if rows_offset.saturating_add(rows_bytes) > byte_len {
            return;
        }
        let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), count) };

        for row in rows {
            let pid = row.dwOwningPid;
            let port = u16::from_be(row.dwLocalPort as u16);
            if pid == 0 || port == 0 || port >= 49152 {
                continue;
            }
            push_port_unique(
                by_pid.entry(pid).or_default(),
                PortBinding {
                    port,
                    protocol: TransportProto::Udp,
                    scope: classify_ipv6_scope(&row.ucLocalAddr),
                    is_listening: true,
                    suggested_url: None,
                },
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn query_tcp_v4_rows(buf: &mut Vec<u64>) -> Option<&[MIB_TCPROW_OWNER_PID]> {
    if !query_ip_table(buf, |ptr, sz| unsafe {
        GetExtendedTcpTable(Some(ptr), sz, false, AF_INET.0 as u32, TCP_TABLE_OWNER_PID_ALL, 0)
    }) {
        return None;
    }
    let byte_len = buf.len() * std::mem::size_of::<u64>();
    if byte_len < std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>() {
        return None;
    }
    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
    let count = table.dwNumEntries as usize;
    let rows_offset = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
    let rows_bytes = count.checked_mul(std::mem::size_of::<MIB_TCPROW_OWNER_PID>())?;
    if rows_offset.saturating_add(rows_bytes) > byte_len {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(table.table.as_ptr(), count) })
}

#[cfg(target_os = "windows")]
fn query_tcp_v6_rows(buf: &mut Vec<u64>) -> Option<&[MIB_TCP6ROW_OWNER_PID]> {
    if !query_ip_table(buf, |ptr, sz| unsafe {
        GetExtendedTcpTable(Some(ptr), sz, false, AF_INET6.0 as u32, TCP_TABLE_OWNER_PID_ALL, 0)
    }) {
        return None;
    }
    let byte_len = buf.len() * std::mem::size_of::<u64>();
    if byte_len < std::mem::size_of::<MIB_TCP6TABLE_OWNER_PID>() {
        return None;
    }
    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID) };
    let count = table.dwNumEntries as usize;
    let rows_offset = std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table);
    let rows_bytes = count.checked_mul(std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>())?;
    if rows_offset.saturating_add(rows_bytes) > byte_len {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(table.table.as_ptr(), count) })
}

/// Lightweight check for whether any port in `ports` is currently listening on IPv4/IPv6 TCP.
pub fn are_tcp_ports_listening(ports: &[u16]) -> bool {
    if ports.is_empty() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        let mut buf = vec![0u64; 8192];
        if let Some(rows) = query_tcp_v4_rows(&mut buf) {
            if rows.iter().any(|r| {
                r.dwState == MIB_TCP_STATE_LISTEN.0 as u32
                    && ports.contains(&u16::from_be(r.dwLocalPort as u16))
            }) {
                return true;
            }
        }
        if let Some(rows) = query_tcp_v6_rows(&mut buf) {
            if rows.iter().any(|r| {
                r.dwState == MIB_TCP_STATE_LISTEN.0 as u32
                    && ports.contains(&u16::from_be(r.dwLocalPort as u16))
            }) {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Ok(output) = std::process::Command::new("lsof")
            .args(["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-F", "pPnT"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let map = parse_lsof_ports(&String::from_utf8_lossy(&output.stdout));
        map.values()
            .any(|info| info.ports.iter().any(|b| ports.contains(&b.port)))
    }
}

#[cfg(target_os = "windows")]
fn query_ip_table<F>(buf: &mut Vec<u64>, mut call: F) -> bool
where
    F: FnMut(*mut c_void, *mut u32) -> u32,
{
    if buf.is_empty() {
        buf.resize(8192, 0);
    }
    for _ in 0..3 {
        let mut size = (buf.len() * std::mem::size_of::<u64>()) as u32;
        let rc = call(buf.as_mut_ptr() as *mut c_void, &mut size);
        if rc == NO_ERROR.0 {
            return true;
        }
        if rc != ERROR_INSUFFICIENT_BUFFER.0 {
            return false;
        }
        let needed_bytes = (size as usize).max(buf.len() * 8) + 8192;
        buf.resize(needed_bytes.div_ceil(std::mem::size_of::<u64>()), 0);
    }
    false
}

#[cfg(target_os = "windows")]
fn classify_ipv4_scope(addr_be: u32) -> BindScope {
    let ip = u32::from_be(addr_be);
    if ip == 0 {
        BindScope::AllInterfaces
    } else if ip == 0x7F00_0001 {
        BindScope::Localhost
    } else {
        BindScope::SpecificIp
    }
}

#[cfg(target_os = "windows")]
fn classify_ipv6_scope(addr: &[u8; 16]) -> BindScope {
    if addr.iter().all(|&b| b == 0) {
        BindScope::AllInterfaces
    } else if addr[..15].iter().all(|&b| b == 0) && addr[15] == 1 {
        BindScope::Localhost
    } else {
        BindScope::SpecificIp
    }
}

#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn parse_lsof_ports(text: &str) -> HashMap<u32, PidNetworkInfo> {
    let mut by_pid: HashMap<u32, PidNetworkInfo> = HashMap::with_capacity(64);
    let mut cur_pid = 0u32;
    let mut cur_proto: Option<TransportProto> = None;
    let mut cur_endpoint: Option<(BindScope, u16)> = None;
    let mut is_listen = false;
    let mut is_estab = false;

    let flush_socket = |by_pid: &mut HashMap<u32, PidNetworkInfo>,
                        pid: u32,
                        proto: Option<TransportProto>,
                        endpoint: Option<(BindScope, u16)>,
                        listen: bool,
                        estab: bool| {
        if pid == 0 {
            return;
        }
        let Some(p) = proto else { return };
        if matches!(p, TransportProto::Tcp) && estab {
            return;
        }
        let Some((scope, port)) = endpoint else { return };
        if port == 0 {
            return;
        }
        if matches!(p, TransportProto::Tcp) && !listen {
            return;
        }
        if matches!(p, TransportProto::Udp) && port >= 49152 {
            return;
        }
        push_port_unique(
            by_pid.entry(pid).or_default(),
            PortBinding {
                port,
                protocol: p,
                scope,
                is_listening: true,
                suggested_url: if matches!(p, TransportProto::Tcp) {
                    suggest_url_for_port(port)
                } else {
                    None
                },
            },
        );
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            flush_socket(&mut by_pid, cur_pid, cur_proto, cur_endpoint, is_listen, is_estab);
            cur_proto = None;
            cur_endpoint = None;
            is_listen = false;
            is_estab = false;
            cur_pid = rest.trim().parse::<u32>().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix('P') {
            flush_socket(&mut by_pid, cur_pid, cur_proto, cur_endpoint, is_listen, is_estab);
            cur_endpoint = None;
            is_listen = false;
            is_estab = false;
            cur_proto = if rest.eq_ignore_ascii_case("TCP") {
                Some(TransportProto::Tcp)
            } else if rest.eq_ignore_ascii_case("UDP") {
                Some(TransportProto::Udp)
            } else {
                None
            };
        } else if let Some(rest) = line.strip_prefix('n') {
            let local = rest.split("->").next().unwrap_or(rest).trim();
            if let Some((host, port_s)) = local.rsplit_once(':') {
                if let Ok(port) = port_s.parse::<u16>() {
                    let h = host.trim_matches(|c| c == '[' || c == ']');
                    let scope = if h == "*" || h == "0.0.0.0" || h == "::" {
                        BindScope::AllInterfaces
                    } else if h == "127.0.0.1" || h == "::1" || h.eq_ignore_ascii_case("localhost") {
                        BindScope::Localhost
                    } else {
                        BindScope::SpecificIp
                    };
                    cur_endpoint = Some((scope, port));
                }
            }
        } else if let Some(rest) = line.strip_prefix("TST=") {
            if rest.eq_ignore_ascii_case("LISTEN") {
                is_listen = true;
            } else if rest.eq_ignore_ascii_case("ESTABLISHED") {
                is_estab = true;
            }
        }
    }
    flush_socket(&mut by_pid, cur_pid, cur_proto, cur_endpoint, is_listen, is_estab);

    for info in by_pid.values_mut() {
        info.ports
            .sort_by_key(|p| (p.port, matches!(p.protocol, TransportProto::Udp)));
    }
    by_pid
}

fn push_port_unique(info: &mut PidNetworkInfo, binding: PortBinding) {
    if let Some(existing) = info
        .ports
        .iter_mut()
        .find(|p| p.port == binding.port && p.protocol == binding.protocol)
    {
        if matches!(binding.scope, BindScope::AllInterfaces) {
            existing.scope = BindScope::AllInterfaces;
        }
        return;
    }
    info.ports.push(binding);
}

/// Suggests a clickable HTTP/HTTPS URL for common developer server ports.
pub fn suggest_url_for_port(port: u16) -> Option<String> {
    match port {
        443 | 8443 => Some(format!("https://localhost:{port}")),
        80 => Some("http://localhost".to_string()),
        3000..=3010
        | 4000
        | 4200
        | 4321
        | 5000
        | 5173..=5179
        | 5500
        | 7000
        | 8000..=8010
        | 8080..=8089
        | 8888
        | 9000
        | 9090 => Some(format!("http://localhost:{port}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_tcp_and_udp_listening_ports_across_pids() {
        let sample = "\
p4210
f5
PTCP
n127.0.0.1:3000
TST=LISTEN
f6
PTCP
n[::1]:5432->[::1]:51234
TST=ESTABLISHED
p5500
f8
PTCP
n*:8080
TST=LISTEN
";
        let map = parse_lsof_ports(sample);
        let p1 = map.get(&4210).unwrap();
        assert_eq!(p1.ports.len(), 1);
        assert_eq!(p1.ports[0].port, 3000);
        assert_eq!(p1.ports[0].scope, BindScope::Localhost);
        assert_eq!(p1.ports[0].suggested_url.as_deref(), Some("http://localhost:3000"));

        let p2 = map.get(&5500).unwrap();
        assert_eq!(p2.ports[0].port, 8080);
        assert_eq!(p2.ports[0].scope, BindScope::AllInterfaces);
    }
}
