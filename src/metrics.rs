//! Per-process and native server process-tree resource usage.
//!
//! Host-level numbers say nothing useful when several servers share a box, so
//! the panel reports each JVM or Horizon process tree's CPU and memory.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, MINIMUM_CPU_UPDATE_INTERVAL};

/// How long a directory size is trusted before it is measured again.
///
/// Walking a large world costs real I/O, and the dashboard polls every few
/// seconds; without this the panel would spend more effort measuring the server
/// than running it.
const DISK_TTL: Duration = Duration::from_secs(60);
/// Share one process scan across simultaneous server and dashboard requests.
const TREE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// What the machine as a whole is doing.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct HostMetrics {
    /// Busy percentage across all cores, or unavailable when counters cannot be read.
    pub cpu_percent: Option<f32>,
    /// Physical memory in use, in mebibytes.
    pub memory_used_mb: u64,
    /// Physical memory installed, in mebibytes.
    pub memory_total_mb: u64,
}

/// What a server process or native process tree is currently costing.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProcessMetrics {
    /// Existing sysinfo value: 100 means one fully occupied logical CPU, so a
    /// multi-threaded process can legitimately exceed 100.
    pub cpu_percent: Option<f32>,
    /// Approximate number of logical CPUs consumed.
    pub cpu_cores: Option<f32>,
    /// Process CPU normalized against the whole panel host, in 0..=100.
    pub cpu_host_percent: Option<f32>,
    /// Logical CPUs available to the panel host.
    pub logical_cpu_count: usize,
    /// Resident memory (RSS) in mebibytes; tree sums may count shared pages twice.
    pub memory_mb: u64,
}

/// Samples processes, keeping enough history for CPU deltas to be meaningful.
pub struct Metrics {
    // sysinfo computes CPU usage as a delta against the previous refresh, so
    // the same System instance has to survive between calls. Building a fresh
    // one per request is the classic way to get a number that never moves.
    system: Mutex<System>,
    tree: Mutex<TreeSampler>,
    disk: Mutex<HashMap<PathBuf, (Instant, u64)>>,
    /// Last host CPU reading, and when it was taken.
    host_cpu: Mutex<(Instant, Option<f32>)>,
    /// The host capacity against which per-process CPU is normalized.
    logical_cpu_count: usize,
}

struct TreeSampler {
    system: System,
    refreshed_at: Option<Instant>,
    cpu_ready: HashSet<Pid>,
    #[cfg(target_os = "android")]
    android_cpu: HashMap<Pid, AndroidTreeCpu>,
}

#[cfg(target_os = "android")]
struct AndroidTreeCpu {
    at: Instant,
    counters: HashMap<Pid, ProcCpuCounters>,
    value: Option<f32>,
}

#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcCpuCounters {
    start_ticks: u64,
    cpu_ticks: u64,
}

impl Default for Metrics {
    fn default() -> Self {
        // A tree scan indexes every visible process. Do not let sysinfo retain
        // one /proc descriptor per process and consume the panel's file budget.
        let _ = sysinfo::set_open_files_limit(0);
        let mut system = System::new();

        // Establish the baseline the first real reading is measured against.
        system.refresh_cpu_usage();
        let logical_cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or_else(|_| system.cpus().len().max(1));
        let mut tree_system = System::new();
        tree_system.refresh_cpu_usage();

        Metrics {
            system: Mutex::new(system),
            tree: Mutex::new(TreeSampler {
                system: tree_system,
                refreshed_at: None,
                cpu_ready: HashSet::new(),
                #[cfg(target_os = "android")]
                android_cpu: HashMap::new(),
            }),
            disk: Mutex::new(HashMap::new()),
            host_cpu: Mutex::new((Instant::now(), None)),
            logical_cpu_count,
        }
    }
}

impl Metrics {
    /// Sample `pid`, or `None` if the process has gone.
    ///
    /// The first sample for a process always reports 0% CPU: there is no
    /// previous measurement to compare against, and inventing one would lie.
    pub fn of(&self, pid: u32) -> Option<ProcessMetrics> {
        let pid = Pid::from_u32(pid);
        let mut system = self.system.lock().ok()?;

        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );

        let process = system.process(pid)?;
        let cpu_percent = process.cpu_usage();
        let (cpu_cores, cpu_host_percent) =
            normalize_process_cpu(cpu_percent, self.logical_cpu_count);
        Some(ProcessMetrics {
            cpu_percent: Some(cpu_percent),
            cpu_cores: Some(cpu_cores),
            cpu_host_percent: Some(cpu_host_percent),
            logical_cpu_count: self.logical_cpu_count,
            memory_mb: process.memory() / 1024 / 1024,
        })
    }

    /// Sample the Horizon supervisor, engine, and their descendants.
    ///
    /// A full process scan is cached briefly. Only descendants of the panel's
    /// own live child are considered, and no value is returned until an engine
    /// is visible. RSS is summed, so shared pages can be counted more than once.
    pub fn of_horizon_tree(&self, pid: u32) -> Option<ProcessMetrics> {
        let mut sampler = self.tree.lock().ok()?;
        if sampler
            .refreshed_at
            .is_none_or(|at| at.elapsed() >= TREE_REFRESH_INTERVAL)
        {
            let previous: HashMap<_, _> = sampler
                .system
                .processes()
                .iter()
                .map(|(pid, process)| (*pid, process.start_time()))
                .collect();
            sampler.system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_cpu().with_memory(),
            );
            sampler.cpu_ready = sampler
                .system
                .processes()
                .iter()
                .filter_map(|(pid, process)| {
                    (previous.get(pid) == Some(&process.start_time())).then_some(*pid)
                })
                .collect();
            #[cfg(target_os = "android")]
            {
                let live: HashSet<_> = sampler.system.processes().keys().copied().collect();
                sampler.android_cpu.retain(|pid, _| live.contains(pid));
            }
            sampler.refreshed_at = Some(Instant::now());
        }
        let root = Pid::from_u32(pid);
        if sampler.system.process(root)?.parent() != Some(Pid::from_u32(std::process::id())) {
            return None;
        }
        let nodes = descendant_pids(&sampler.system, root);
        let engine = nodes
            .iter()
            .filter(|pid| **pid != root)
            .filter_map(|pid| sampler.system.process(*pid))
            .any(|process| {
                let name = process.name().to_string_lossy();
                [
                    "horizon-engine",
                    "horizon-client",
                    "horizon-engine.exe",
                    "horizon-client.exe",
                ]
                .iter()
                .any(|expected| name.eq_ignore_ascii_case(expected))
            });
        if !engine {
            return None;
        }
        let mut memory_bytes = 0_u64;
        let mut cpu = 0_f32;
        let mut cpu_valid = true;
        for process in nodes.iter().filter_map(|pid| sampler.system.process(*pid)) {
            // An unreadable RSS must not be presented as an accurate total.
            if process.memory() == 0 {
                return None;
            }
            memory_bytes = memory_bytes.saturating_add(process.memory());
            let usage = process.cpu_usage();
            if !usage.is_finite() || usage < 0.0 {
                cpu_valid = false;
            } else {
                cpu += usage;
            }
        }
        let cpu_available = cpu_valid
            && cpu.is_finite()
            && !sampler.system.cpus().is_empty()
            && nodes.iter().all(|pid| sampler.cpu_ready.contains(pid));
        #[cfg(target_os = "android")]
        let cpu_value = sampler
            .android_cpu_value(root, &nodes, self.logical_cpu_count)
            .or_else(|| {
                (cpu_available && std::fs::File::open("/proc/stat").is_ok()).then_some(cpu)
            });
        #[cfg(not(target_os = "android"))]
        let cpu_value = cpu_available.then_some(cpu);
        let (cores, host) = normalize_process_cpu(cpu_value.unwrap_or(0.0), self.logical_cpu_count);
        Some(ProcessMetrics {
            cpu_percent: cpu_value,
            cpu_cores: cpu_value.map(|_| cores),
            cpu_host_percent: cpu_value.map(|_| host),
            logical_cpu_count: self.logical_cpu_count,
            memory_mb: memory_bytes / 1024 / 1024,
        })
    }

    /// Whole-machine CPU and memory.
    ///
    /// CPU is a delta against the previous sample, so two refreshes closer
    /// together than sysinfo's minimum interval would report nonsense. Inside
    /// that window the last real reading is repeated rather than recomputed.
    pub fn host(&self) -> HostMetrics {
        let mut system = match self.system.lock() {
            Ok(system) => system,
            Err(poisoned) => poisoned.into_inner(),
        };

        system.refresh_memory();

        #[cfg(target_os = "android")]
        let counters_available = std::fs::File::open("/proc/stat").is_ok();
        #[cfg(not(target_os = "android"))]
        let counters_available = true;
        let read_cpu = |system: &System| {
            (counters_available && !system.cpus().is_empty()).then(|| system.global_cpu_usage())
        };
        let cpu_percent = match self.host_cpu.lock() {
            Ok(mut last) => {
                if last.0.elapsed() >= MINIMUM_CPU_UPDATE_INTERVAL {
                    system.refresh_cpu_usage();
                    *last = (Instant::now(), read_cpu(&system));
                }
                last.1
            }
            Err(_) => read_cpu(&system),
        };

        HostMetrics {
            cpu_percent,
            memory_used_mb: system.used_memory() / 1024 / 1024,
            memory_total_mb: system.total_memory() / 1024 / 1024,
        }
    }

    /// Total size of everything under `dir`, cached for [`DISK_TTL`].
    ///
    /// Returns the cached value immediately when it is fresh; otherwise the walk
    /// runs on the blocking pool, because a world with tens of thousands of
    /// region files is not something to traverse on the async runtime.
    pub async fn disk_usage(&self, dir: &Path) -> u64 {
        if let Ok(cache) = self.disk.lock() {
            if let Some((measured, bytes)) = cache.get(dir) {
                if measured.elapsed() < DISK_TTL {
                    return *bytes;
                }
            }
        }

        let target = dir.to_path_buf();
        let bytes = tokio::task::spawn_blocking(move || directory_size(&target))
            .await
            .unwrap_or(0);

        if let Ok(mut cache) = self.disk.lock() {
            cache.insert(dir.to_path_buf(), (Instant::now(), bytes));
        }

        bytes
    }
}

/// Linux sysinfo also lists individual threads. Exclude them to avoid counting
/// a multithreaded engine's shared RSS and CPU repeatedly.
fn descendant_pids(system: &System, root: Pid) -> Vec<Pid> {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if process.thread_kind().is_some() {
            continue;
        }
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([root]);
    while let Some(pid) = pending.pop_front() {
        if !visited.insert(pid) {
            continue;
        }
        result.push(pid);
        if let Some(next) = children.get(&pid) {
            pending.extend(next.iter().copied());
        }
    }
    result
}

#[cfg(any(target_os = "android", test))]
fn parse_proc_cpu_stat(text: &str) -> Option<ProcCpuCounters> {
    // The process name is parenthesized and may itself contain spaces or ')'.
    // The first token after the final ')' is field 3 (state).
    let (_, fields) = text.rsplit_once(')')?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some(ProcCpuCounters {
        start_ticks: fields.get(19)?.parse().ok()?,
        cpu_ticks: user.checked_add(system)?,
    })
}

#[cfg(any(target_os = "android", test))]
fn proc_tree_cpu_percent(
    previous: &HashMap<Pid, ProcCpuCounters>,
    current: &HashMap<Pid, ProcCpuCounters>,
    elapsed: Duration,
    ticks_per_second: u64,
    logical_cpu_count: usize,
) -> Option<f32> {
    if ticks_per_second == 0
        || elapsed.is_zero()
        || previous.len() != current.len()
        || current.is_empty()
    {
        return None;
    }
    let ticks = current.iter().try_fold(0_u64, |sum, (pid, value)| {
        let old = previous.get(pid)?;
        if old.start_ticks != value.start_ticks {
            return None;
        }
        sum.checked_add(value.cpu_ticks.checked_sub(old.cpu_ticks)?)
    })?;
    let percent = ticks as f64 / ticks_per_second as f64 / elapsed.as_secs_f64() * 100.0;
    if !percent.is_finite() {
        return None;
    }
    Some((percent as f32).clamp(0.0, logical_cpu_count.max(1) as f32 * 100.0))
}

#[cfg(target_os = "android")]
impl TreeSampler {
    /// Android often denies `/proc/stat` while allowing the panel to read its
    /// own child processes. Derive CPU usage from their tick deltas and elapsed
    /// monotonic time instead of treating denied host counters as zero.
    fn android_cpu_value(
        &mut self,
        root: Pid,
        nodes: &[Pid],
        logical_cpu_count: usize,
    ) -> Option<f32> {
        let now = Instant::now();
        if let Some(sample) = self.android_cpu.get(&root) {
            if now.duration_since(sample.at) < TREE_REFRESH_INTERVAL {
                return sample.value;
            }
        }
        let counters: Option<HashMap<_, _>> = nodes
            .iter()
            .map(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_u32())).ok()?;
                Some((*pid, parse_proc_cpu_stat(&stat)?))
            })
            .collect();
        let Some(counters) = counters else {
            self.android_cpu.remove(&root);
            return None;
        };
        let value = self.android_cpu.get(&root).and_then(|previous| {
            proc_tree_cpu_percent(
                &previous.counters,
                &counters,
                now.duration_since(previous.at),
                rustix::param::clock_ticks_per_second(),
                logical_cpu_count,
            )
        });
        self.android_cpu.insert(
            root,
            AndroidTreeCpu {
                at: now,
                counters,
                value,
            },
        );
        value
    }
}

/// Convert sysinfo's one-logical-core percentage to both useful display forms.
///
/// Keeping this pure makes the semantics independently testable without
/// depending on a process's scheduling or the host's current load.
fn normalize_process_cpu(cpu_percent: f32, logical_cpu_count: usize) -> (f32, f32) {
    let logical_cpu_count = logical_cpu_count.max(1) as f32;
    let non_negative = if cpu_percent.is_finite() {
        cpu_percent.max(0.0)
    } else {
        0.0
    };
    let cpu_cores = non_negative / 100.0;
    let cpu_host_percent = (non_negative / logical_cpu_count).clamp(0.0, 100.0);
    (cpu_cores, cpu_host_percent)
}

/// Sum every regular file under `root`.
fn directory_size(root: &Path) -> u64 {
    guardian::ScopedFs::open(root)
        .and_then(|fs| fs.directory_size("."))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_cpu_parser_handles_names_with_spaces_and_parentheses() {
        let mut fields = vec!["0"; 20];
        fields[0] = "S";
        fields[11] = "25";
        fields[12] = "7";
        fields[19] = "987";
        let line = format!("123 (name ) with spaces) {}", fields.join(" "));
        assert_eq!(
            parse_proc_cpu_stat(&line),
            Some(ProcCpuCounters {
                start_ticks: 987,
                cpu_ticks: 32
            })
        );
        assert_eq!(parse_proc_cpu_stat("123 (truncated) S"), None);
    }

    #[test]
    fn proc_cpu_delta_rejects_reused_or_changed_processes() {
        let a = Pid::from_u32(1);
        let b = Pid::from_u32(2);
        let previous = HashMap::from([
            (
                a,
                ProcCpuCounters {
                    start_ticks: 5,
                    cpu_ticks: 100,
                },
            ),
            (
                b,
                ProcCpuCounters {
                    start_ticks: 9,
                    cpu_ticks: 200,
                },
            ),
        ]);
        let mut current = HashMap::from([
            (
                a,
                ProcCpuCounters {
                    start_ticks: 5,
                    cpu_ticks: 150,
                },
            ),
            (
                b,
                ProcCpuCounters {
                    start_ticks: 9,
                    cpu_ticks: 225,
                },
            ),
        ]);
        assert_eq!(
            proc_tree_cpu_percent(&previous, &current, Duration::from_secs(2), 100, 8),
            Some(37.5)
        );
        current.get_mut(&b).unwrap().start_ticks = 10;
        assert_eq!(
            proc_tree_cpu_percent(&previous, &current, Duration::from_secs(2), 100, 8),
            None
        );
        current.get_mut(&b).unwrap().start_ticks = 9;
        current.get_mut(&a).unwrap().cpu_ticks = 99;
        assert_eq!(
            proc_tree_cpu_percent(&previous, &current, Duration::from_secs(2), 100, 8),
            None
        );
        current.remove(&b);
        assert_eq!(
            proc_tree_cpu_percent(&previous, &current, Duration::from_secs(2), 100, 8),
            None
        );
    }

    #[test]
    fn unavailable_cpu_serializes_as_null_without_hiding_rss() {
        let value = serde_json::to_value(ProcessMetrics {
            cpu_percent: None,
            cpu_cores: None,
            cpu_host_percent: None,
            logical_cpu_count: 8,
            memory_mb: 321,
        })
        .unwrap();
        assert!(value["cpu_percent"].is_null());
        assert!(value["cpu_host_percent"].is_null());
        assert_eq!(value["memory_mb"], 321);
    }

    #[cfg(unix)]
    #[test]
    fn horizon_metrics_require_a_visible_engine_in_the_owned_tree() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("sh"))
            .find(|path| path.is_file())
            .expect("a shell is required for the process-tree test");
        let engine = root.path().join("horizon-engine");
        std::fs::copy(&shell, &engine).unwrap();
        let mut supervisor = Command::new(&shell)
            .args([
                "-c",
                "\"$1\" -c 'while :; do :; done' & engine=$!; IFS= read -r line; kill \"$engine\"; wait \"$engine\" || :",
                "supervisor",
            ])
            .arg(&engine)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let metrics = Metrics::default();
        // The panel process itself is not its own child, even if it has an
        // engine somewhere below it.
        let own_pid_rejected = metrics.of_horizon_tree(std::process::id()).is_none();
        let deadline = Instant::now() + Duration::from_secs(12);
        let result = loop {
            if let Some(sample) = metrics.of_horizon_tree(supervisor.id()) {
                break Some(sample);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        #[cfg(target_os = "android")]
        let cpu_sample = {
            std::thread::sleep(TREE_REFRESH_INTERVAL + Duration::from_millis(100));
            metrics.of_horizon_tree(supervisor.id())
        };
        let diagnostic = {
            let sampler = metrics.tree.lock().unwrap();
            let root = Pid::from_u32(supervisor.id());
            let owner = Pid::from_u32(std::process::id());
            sampler
                .system
                .processes()
                .iter()
                .filter(|(pid, p)| {
                    **pid == root || p.parent() == Some(root) || p.parent() == Some(owner)
                })
                .map(|(pid, p)| {
                    format!(
                        "pid={pid} parent={:?} name={:?} thread={:?} rss={}",
                        p.parent(),
                        p.name(),
                        p.thread_kind(),
                        p.memory()
                    )
                })
                .collect::<Vec<_>>()
        };
        if let Some(mut stdin) = supervisor.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
        }
        let status = supervisor.wait().unwrap();
        assert!(own_pid_rejected);
        assert!(status.success());
        let sample = result.unwrap_or_else(|| {
            panic!("the owned supervisor and engine should be visible: {diagnostic:?}")
        });
        assert!(sample.memory_mb > 0, "engine RSS should be included");
        #[cfg(target_os = "android")]
        assert!(
            cpu_sample
                .and_then(|sample| sample.cpu_percent)
                .is_some_and(|value| value > 0.0),
            "owned process CPU counters should produce a nonzero delta"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires HORIZON_SMOKE_SERVER, HORIZON_SMOKE_ENGINE and HORIZON_SMOKE_RESOURCES"]
    async fn real_horizon_engine_is_included_in_process_metrics() {
        use guardian::{Guardian, GuardianConfig, ServerConfig, ServerStatus};
        let root = tempfile::tempdir().unwrap();
        let installation = root.path().join("horizon");
        let directory = root.path().join("servers/metrics");
        std::fs::create_dir_all(&installation).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        for (variable, name) in [
            ("HORIZON_SMOKE_SERVER", "horizon-server"),
            ("HORIZON_SMOKE_ENGINE", "horizon-engine"),
        ] {
            let source = std::env::var_os(variable).expect(variable);
            let destination = installation.join(name);
            std::fs::hard_link(&source, &destination)
                .or_else(|_| std::fs::copy(&source, &destination).map(|_| ()))
                .unwrap();
        }
        std::os::unix::fs::symlink(
            std::env::var_os("HORIZON_SMOKE_RESOURCES").unwrap(),
            directory.join("resources"),
        )
        .unwrap();
        std::fs::write(directory.join("server.toml"), "[server]\nbind = '127.0.0.1'\nonline_mode = false\nwhitelist = false\nview_distance = 2\nsimulation_distance = 2\n[engine]\nlow_memory = true\n[backups]\nevery = '0'\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut config = ServerConfig::paper(&directory, "26.3");
        config.core = "horizon".into();
        config.port = port;
        config.eula_accepted = true;
        let guardian = Guardian::new_with_sandbox_policy(
            config,
            GuardianConfig {
                stop_timeout_secs: 120,
                ..Default::default()
            },
            root.path(),
            guardian::sandbox::SandboxPolicy::new(true),
        );
        let mut events = guardian.subscribe();
        guardian.start().await.unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                match events.recv().await.unwrap() {
                    guardian::ServerEvent::Status {
                        status: ServerStatus::Online,
                    } => return true,
                    guardian::ServerEvent::Status {
                        status: ServerStatus::Offline | ServerStatus::Crashed,
                    } => return false,
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        let metrics = Metrics::default();
        let pid = guardian.snapshot().await.pid;
        let sample = pid.and_then(|pid| metrics.of_horizon_tree(pid));
        let supervisor_only = pid.and_then(|pid| metrics.of(pid));
        tokio::time::sleep(TREE_REFRESH_INTERVAL + Duration::from_millis(100)).await;
        let second_sample = pid.and_then(|pid| metrics.of_horizon_tree(pid));
        if guardian.status().await.is_running()
            || guardian.status().await == ServerStatus::Preparing
        {
            guardian.stop().await.unwrap();
        }
        let lines = guardian.console().await;
        assert!(ready, "Horizon did not become ready: {lines:?}");
        let sample = sample.expect("the native engine and supervisor should be visible");
        assert!(
            sample.memory_mb > 16,
            "native engine memory is missing: {sample:?}"
        );
        if let Some(supervisor) = supervisor_only {
            assert!(
                sample.memory_mb > supervisor.memory_mb,
                "engine RSS must be included"
            );
        }
        let second = second_sample.expect("the native process tree should remain visible");
        assert!(second.memory_mb > 16);
        #[cfg(target_os = "android")]
        assert!(
            second.cpu_percent.is_some(),
            "per-process CPU counters should be available"
        );
        eprintln!("Horizon process-tree sample: {second:?}");
    }

    /// Occupy every core for `duration`.
    fn saturate(duration: Duration) {
        let threads: Vec<_> = (0..std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2))
            .map(|_| {
                std::thread::spawn(move || {
                    let deadline = Instant::now() + duration;
                    let mut spin: u64 = 0;
                    while Instant::now() < deadline {
                        spin = spin.wrapping_add(1);
                    }
                    std::hint::black_box(spin);
                })
            })
            .collect();

        for thread in threads {
            let _ = thread.join();
        }
    }

    #[test]
    fn host_cpu_responds_to_load() {
        let metrics = Metrics::default();

        // Establish a baseline, then read again after an idle gap.
        metrics.host();
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL * 2);
        let Some(idle) = metrics.host().cpu_percent else {
            #[cfg(target_os = "android")]
            return;
            #[cfg(not(target_os = "android"))]
            panic!("host CPU counters are unavailable");
        };

        saturate(MINIMUM_CPU_UPDATE_INTERVAL * 3);
        let busy = metrics
            .host()
            .cpu_percent
            .expect("host CPU counters disappeared");

        // Asserting `busy > 0` would not catch anything: a freshly built System
        // reports an average since boot, which is non-zero and never moves. What
        // was broken is that the number did not *respond*, so that is the
        // assertion — saturating every core has to show up. The host may already
        // be busy, so allow a smaller delta when idle is already high.
        let delta = if idle > 80.0 { 1.0 } else { 5.0 };
        assert!(
            busy > idle + delta || busy > 90.0,
            "idle {idle}% then saturated {busy}%: the reading is not tracking load"
        );
    }

    #[cfg(target_os = "android")]
    #[test]
    fn restricted_host_cpu_is_reported_as_unavailable() {
        if std::fs::File::open("/proc/stat").is_err() {
            let metrics = Metrics::default();
            std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL * 2);
            assert!(metrics.host().cpu_percent.is_none());
        }
    }

    #[test]
    fn host_memory_is_reported_and_sane() {
        let host = Metrics::default().host();

        assert!(host.memory_total_mb > 0);
        assert!(host.memory_used_mb > 0);
        assert!(host.memory_used_mb <= host.memory_total_mb);
    }

    #[test]
    fn process_cpu_exposes_raw_cores_and_host_normalized_values() {
        let (cores, host_percent) = normalize_process_cpu(278.98, 8);

        assert!((cores - 2.7898).abs() < 0.0001);
        assert!((host_percent - 34.8725).abs() < 0.0001);
    }

    #[test]
    fn process_cpu_normalization_has_a_safe_one_cpu_floor() {
        let (cores, host_percent) = normalize_process_cpu(150.0, 0);

        assert_eq!(cores, 1.5);
        assert_eq!(host_percent, 100.0);
    }

    #[test]
    fn a_directory_size_is_the_sum_of_its_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();
        std::fs::write(tmp.path().join("nested/b"), vec![0u8; 250]).unwrap();

        assert_eq!(directory_size(tmp.path()), 350);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cycle_does_not_loop_forever() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), vec![0u8; 40]).unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();

        // A link back to an ancestor. Following it would recurse without end.
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("nested/loop")).unwrap();

        assert_eq!(directory_size(tmp.path()), 40);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_tree_is_not_counted_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("big"), vec![0u8; 500]).unwrap();

        std::os::unix::fs::symlink(&real, tmp.path().join("alias")).unwrap();

        assert_eq!(directory_size(tmp.path()), 500);
    }

    #[test]
    fn an_absent_directory_measures_zero_rather_than_failing() {
        assert_eq!(directory_size(Path::new("/nonexistent/for/sure")), 0);
    }

    #[tokio::test]
    async fn a_measurement_is_cached() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), vec![0u8; 64]).unwrap();

        let metrics = Metrics::default();
        assert_eq!(metrics.disk_usage(tmp.path()).await, 64);

        // Written after the measurement, so a cached read must not see it.
        std::fs::write(tmp.path().join("b"), vec![0u8; 1000]).unwrap();
        assert_eq!(metrics.disk_usage(tmp.path()).await, 64);
    }
}
