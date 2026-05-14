use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::Write;
use std::mem::size_of;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use rvsim::elf::{ELF_PROGRAM_TYPE_LOADABLE, Elf32};
use rvsim::{Clock, CpuError, CpuState, Interp, Memory, MemoryAccess, Op};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(feature = "rv32fd")]
use rvsim::softfloat::Sf64;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheHits {
    pub l1i: u64,
    pub l1d: u64,
    pub l2: u64,
    pub l3: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunStats {
    pub cycles: u64,
    pub instret: u64,
    pub memory_access_count: u64,
    pub instruction_fetches: u64,
    pub data_loads: u64,
    pub data_stores: u64,
    pub syscall_count: u64,
    pub syscall_cycles: u64,
    pub syscall_hits: HashMap<u32, u64>,
    pub syscall_cycle_totals: HashMap<u32, u64>,
    pub cache_hits: CacheHits,
    pub io_bytes_written: u64,
    pub io_cycles: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LatencyConfigRaw {
    pub l1: Option<u64>,
    pub l2: Option<u64>,
    pub l3: Option<u64>,
    pub memory: Option<u64>,
    pub store: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SimConfigRaw {
    pub default_syscall_cycles: Option<u64>,
    pub io_cycles_per_byte: Option<u64>,
    #[serde(default)]
    pub latency: LatencyConfigRaw,
    #[serde(default)]
    pub syscall_cycles: HashMap<String, u64>,
}

impl SimConfigRaw {
    pub fn merge_with(self, user: SimConfigRaw) -> SimConfigRaw {
        SimConfigRaw {
            default_syscall_cycles: user.default_syscall_cycles.or(self.default_syscall_cycles),
            io_cycles_per_byte: user.io_cycles_per_byte.or(self.io_cycles_per_byte),
            latency: LatencyConfigRaw {
                l1: user.latency.l1.or(self.latency.l1),
                l2: user.latency.l2.or(self.latency.l2),
                l3: user.latency.l3.or(self.latency.l3),
                memory: user.latency.memory.or(self.latency.memory),
                store: user.latency.store.or(self.latency.store),
            },
            syscall_cycles: {
                let mut cycles = self.syscall_cycles;
                cycles.extend(user.syscall_cycles);
                cycles
            },
        }
    }

    pub fn resolve(self) -> anyhow::Result<SimConfig> {
        let syscall_cycles = self
            .syscall_cycles
            .into_iter()
            .map(|(k, v)| {
                let code = parse_u32_auto(&k)
                    .map_err(|e| anyhow!("invalid syscall key in config: {e}"))?;
                Ok((code, v))
            })
            .collect::<anyhow::Result<HashMap<u32, u64>>>()?;

        Ok(SimConfig {
            default_syscall_cycles: self.default_syscall_cycles.unwrap_or(500),
            io_cycles_per_byte: self.io_cycles_per_byte.unwrap_or(20),
            l1_latency: self.latency.l1.unwrap_or(3),
            l2_latency: self.latency.l2.unwrap_or(11),
            l3_latency: self.latency.l3.unwrap_or(50),
            memory_latency: self.latency.memory.unwrap_or(125),
            store_latency: self.latency.store.unwrap_or(1),
            syscall_cycles,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimConfig {
    pub default_syscall_cycles: u64,
    pub io_cycles_per_byte: u64,
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub memory_latency: u64,
    pub store_latency: u64,
    pub syscall_cycles: HashMap<u32, u64>,
}

pub fn load_config(path: &Path) -> anyhow::Result<SimConfigRaw> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("Failed to parse config: {}", path.display()))
}

pub fn parse_u32_auto(raw: &str) -> Result<u32, String> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| format!("invalid u32 hex '{raw}': {e}"))
    } else {
        raw.parse::<u32>()
            .map_err(|e| format!("invalid u32 '{raw}': {e}"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmRuntimeOptions {
    pub cache_line_size: u32,
    pub l1_size: u32,
    pub l2_size: u32,
    pub l3_size: u32,
    pub max_instructions: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBounds {
    pub start: u32,
    pub end: u32,
}

impl MemoryBounds {
    fn contains(&self, addr: u32) -> bool {
        self.start <= addr && addr <= self.end
    }

    fn available_from(&self, addr: u32) -> u32 {
        if !self.contains(addr) {
            0
        } else {
            self.end.saturating_sub(addr).saturating_add(1)
        }
    }

    fn expand_to_include(&mut self, addr: u32, len: u32) {
        self.start = self.start.min(addr);
        let end = if len == 0 {
            addr
        } else {
            addr.saturating_add(len.saturating_sub(1))
        };
        self.end = self.end.max(end);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAccessError {
    pub addr: u32,
    pub len: u32,
    pub bounds: Option<MemoryBounds>,
    pub message: String,
}

impl fmt::Display for MemoryAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for MemoryAccessError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MemoryReadResult {
    Full {
        addr: u32,
        len: u32,
        bytes: Vec<u8>,
    },
    Partial {
        addr: u32,
        requested_len: u32,
        available_len: u32,
        bytes: Vec<u8>,
        bounds: MemoryBounds,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetMemoryPolicy {
    Ignore,
    Zero,
    Randomize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum VmStopReason {
    NoProgramLoaded,
    ProgramLoaded,
    Running,
    Paused,
    StepComplete,
    StepOverComplete,
    StepOutComplete,
    QuotaReached,
    ProgramExited,
    Breakpoint { pc: u32 },
    CpuError { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionSnapshot {
    pub text: String,
    pub hex: Option<String>,
    pub size: Option<u32>,
    pub bytes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedBinary {
    pub name: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmState {
    pub running: bool,
    pub halted: bool,
    pub pc: u32,
    pub entry_point: u32,
    pub loaded_binary_name: Option<String>,
    pub loaded_binary_hash: Option<String>,
    pub stop_reason: VmStopReason,
    pub current_instruction: InstructionSnapshot,
    pub registers: [u32; 32],
    #[cfg(feature = "rv32fd")]
    pub floating_registers: [u64; 32],
    pub fcsr: u32,
    pub reservation: Option<u32>,
    pub stats: RunStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuSnapshot {
    pub x: [u32; 32],
    #[cfg(feature = "rv32fd")]
    pub f: [u64; 32],
    pub pc: u32,
    pub fcsr: u32,
    pub reservation: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmMemoryDump {
    pub bytes: HashMap<u32, u8>,
    pub bounds: Option<MemoryBounds>,
    pub caches: CacheHierarchy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmDump {
    pub options: VmRuntimeOptions,
    pub config: SimConfig,
    pub loaded_binary: Option<LoadedBinary>,
    pub entry_point: u32,
    pub running: bool,
    pub halted: bool,
    pub stop_reason: VmStopReason,
    pub cpu: CpuSnapshot,
    pub stats: RunStats,
    pub memory: VmMemoryDump,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheLevel {
    capacity_lines: usize,
    lines: VecDeque<u32>,
    line_set: HashSet<u32>,
}

impl CacheLevel {
    fn new(capacity_lines: usize) -> Self {
        Self {
            capacity_lines,
            lines: VecDeque::with_capacity(capacity_lines),
            line_set: HashSet::with_capacity(capacity_lines),
        }
    }

    fn touch(&mut self, line: u32) -> bool {
        if self.line_set.contains(&line) {
            if let Some(pos) = self.lines.iter().position(|l| *l == line) {
                self.lines.remove(pos);
            }
            self.lines.push_front(line);
            return true;
        }
        false
    }

    fn insert(&mut self, line: u32) {
        if self.line_set.contains(&line) {
            self.touch(line);
            return;
        }

        if self.lines.len() >= self.capacity_lines {
            if let Some(evicted) = self.lines.pop_back() {
                self.line_set.remove(&evicted);
            }
        }
        self.lines.push_front(line);
        self.line_set.insert(line);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheHierarchy {
    line_size: u32,
    l1i: CacheLevel,
    l1d: CacheLevel,
    l2: CacheLevel,
    l3: CacheLevel,
    l1_latency: u64,
    l2_latency: u64,
    l3_latency: u64,
    memory_latency: u64,
    store_latency: u64,
}

impl CacheHierarchy {
    fn new(options: VmRuntimeOptions, config: &SimConfig) -> Self {
        let lines = |bytes: u32| -> usize { (bytes / options.cache_line_size).max(1) as usize };
        Self {
            line_size: options.cache_line_size,
            l1i: CacheLevel::new(lines(options.l1_size)),
            l1d: CacheLevel::new(lines(options.l1_size)),
            l2: CacheLevel::new(lines(options.l2_size)),
            l3: CacheLevel::new(lines(options.l3_size)),
            l1_latency: config.l1_latency,
            l2_latency: config.l2_latency,
            l3_latency: config.l3_latency,
            memory_latency: config.memory_latency,
            store_latency: config.store_latency,
        }
    }

    fn line(&self, addr: u32) -> u32 {
        addr / self.line_size
    }

    fn read_cost(&mut self, addr: u32, is_instruction: bool, stats: &mut RunStats) -> u64 {
        let line = self.line(addr);
        let l1 = if is_instruction {
            &mut self.l1i
        } else {
            &mut self.l1d
        };

        if l1.touch(line) {
            if is_instruction {
                stats.cache_hits.l1i += 1;
            } else {
                stats.cache_hits.l1d += 1;
            }
            return self.l1_latency;
        }

        if self.l2.touch(line) {
            stats.cache_hits.l2 += 1;
            l1.insert(line);
            return self.l2_latency;
        }

        if self.l3.touch(line) {
            stats.cache_hits.l3 += 1;
            self.l2.insert(line);
            l1.insert(line);
            return self.l3_latency;
        }

        self.l3.insert(line);
        self.l2.insert(line);
        l1.insert(line);
        self.memory_latency
    }

    fn store_cost(&mut self, addr: u32) -> u64 {
        let line = self.line(addr);
        self.l1d.insert(line);
        self.l2.insert(line);
        self.l3.insert(line);
        self.store_latency
    }
}

#[derive(Debug)]
pub(crate) struct HecateMemory {
    bytes: HashMap<u32, u8>,
    stats: Rc<RefCell<RunStats>>,
    caches: CacheHierarchy,
    bounds: Option<MemoryBounds>,
}

impl HecateMemory {
    fn new(stats: Rc<RefCell<RunStats>>, options: VmRuntimeOptions, config: &SimConfig) -> Self {
        Self {
            bytes: HashMap::new(),
            stats,
            caches: CacheHierarchy::new(options, config),
            bounds: None,
        }
    }

    fn from_dump(
        stats: Rc<RefCell<RunStats>>,
        options: VmRuntimeOptions,
        memory: VmMemoryDump,
        config: &SimConfig,
    ) -> Self {
        let mut caches = memory.caches;
        caches.line_size = options.cache_line_size;
        caches.l1_latency = config.l1_latency;
        caches.l2_latency = config.l2_latency;
        caches.l3_latency = config.l3_latency;
        caches.memory_latency = config.memory_latency;
        caches.store_latency = config.store_latency;

        Self {
            bytes: memory.bytes,
            stats,
            caches,
            bounds: memory.bounds,
        }
    }

    fn reset_caches(&mut self, options: VmRuntimeOptions, config: &SimConfig) {
        self.caches = CacheHierarchy::new(options, config);
    }

    fn clear_bytes(&mut self) {
        self.bytes.clear();
        self.bounds = None;
    }

    fn update_bounds(&mut self, addr: u32, len: u32) {
        if len == 0 {
            return;
        }

        match &mut self.bounds {
            Some(bounds) => bounds.expand_to_include(addr, len),
            None => {
                self.bounds = Some(MemoryBounds {
                    start: addr,
                    end: addr.saturating_add(len.saturating_sub(1)),
                })
            }
        }
    }

    fn write_bytes_raw(&mut self, addr: u32, data: &[u8]) {
        self.update_bounds(addr, data.len() as u32);
        for (offset, byte) in data.iter().enumerate() {
            self.bytes.insert(addr.wrapping_add(offset as u32), *byte);
        }
    }

    fn zero_fill(&mut self, addr: u32, len: u32) {
        self.update_bounds(addr, len);
        for i in 0..len {
            self.bytes.insert(addr.wrapping_add(i), 0);
        }
    }

    fn zero_current_bounds(&mut self) {
        let Some(bounds) = self.bounds else {
            return;
        };

        for addr in bounds.start..=bounds.end {
            self.bytes.insert(addr, 0);
        }
    }

    fn randomize_current_bounds(&mut self) {
        let Some(bounds) = self.bounds else {
            return;
        };

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        let mut state = seed ^ ((bounds.start as u64) << 32) ^ bounds.end as u64;

        for addr in bounds.start..=bounds.end {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            self.bytes.insert(addr, state as u8);
        }
    }

    fn read_program_bytes(&self, addr: u32, len: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(len as usize);
        for i in 0..len {
            bytes.push(self.bytes.get(&addr.wrapping_add(i)).copied().unwrap_or(0));
        }
        bytes
    }

    fn read_external(&self, addr: u32, len: u32) -> Result<MemoryReadResult, MemoryAccessError> {
        let Some(bounds) = self.bounds else {
            return Err(MemoryAccessError {
                addr,
                len,
                bounds: None,
                message: "memory is not initialized".to_string(),
            });
        };

        if !bounds.contains(addr) {
            return Err(MemoryAccessError {
                addr,
                len,
                bounds: Some(bounds),
                message: format!(
                    "address 0x{addr:08x} is out of bounds (valid range 0x{:08x}..=0x{:08x})",
                    bounds.start, bounds.end
                ),
            });
        }

        let available_len = if len == 0 {
            0
        } else {
            bounds.available_from(addr).min(len)
        };
        let bytes = self.read_program_bytes(addr, available_len);

        if available_len == len {
            Ok(MemoryReadResult::Full { addr, len, bytes })
        } else {
            Ok(MemoryReadResult::Partial {
                addr,
                requested_len: len,
                available_len,
                bytes,
                bounds,
            })
        }
    }

    fn write_external(&mut self, addr: u32, data: &[u8]) -> Result<(), MemoryAccessError> {
        let len = data.len() as u32;
        let Some(bounds) = self.bounds else {
            return Err(MemoryAccessError {
                addr,
                len,
                bounds: None,
                message: "memory is not initialized".to_string(),
            });
        };

        if !bounds.contains(addr) {
            return Err(MemoryAccessError {
                addr,
                len,
                bounds: Some(bounds),
                message: format!(
                    "address 0x{addr:08x} is out of bounds (valid range 0x{:08x}..=0x{:08x})",
                    bounds.start, bounds.end
                ),
            });
        }

        let end = addr
            .checked_add(len.saturating_sub(1))
            .ok_or_else(|| MemoryAccessError {
                addr,
                len,
                bounds: Some(bounds),
                message: "address range overflows the 32-bit address space".to_string(),
            })?;

        if end > bounds.end {
            return Err(MemoryAccessError {
                addr,
                len,
                bounds: Some(bounds),
                message: format!(
                    "write range 0x{addr:08x}..=0x{end:08x} exceeds bounds 0x{:08x}..=0x{:08x}",
                    bounds.start, bounds.end
                ),
            });
        }

        self.write_bytes_raw(addr, data);
        Ok(())
    }

    fn dump(&self) -> VmMemoryDump {
        VmMemoryDump {
            bytes: self.bytes.clone(),
            bounds: self.bounds,
            caches: self.caches.clone(),
        }
    }
}

impl Memory for HecateMemory {
    fn access<T: Copy>(&mut self, addr: u32, access: MemoryAccess<T>) -> bool {
        let size = size_of::<T>() as u32;

        let mut stats = self.stats.borrow_mut();
        stats.memory_access_count += 1;

        match access {
            MemoryAccess::Exec(dest) => {
                stats.instruction_fetches += 1;
                let cost = self.caches.read_cost(addr, true, &mut stats);
                stats.cycles = stats.cycles.wrapping_add(cost);

                let mut raw = vec![0_u8; size as usize];
                for i in 0..size {
                    let Some(byte) = self.bytes.get(&addr.wrapping_add(i)).copied() else {
                        return false;
                    };
                    raw[i as usize] = byte;
                }

                unsafe {
                    std::ptr::copy_nonoverlapping(
                        raw.as_ptr(),
                        (dest as *mut T).cast::<u8>(),
                        raw.len(),
                    );
                }
                true
            }
            MemoryAccess::Load(dest) => {
                stats.data_loads += 1;
                let cost = self.caches.read_cost(addr, false, &mut stats);
                stats.cycles = stats.cycles.wrapping_add(cost);

                let mut raw = vec![0_u8; size as usize];
                for i in 0..size {
                    let Some(byte) = self.bytes.get(&addr.wrapping_add(i)).copied() else {
                        return false;
                    };
                    raw[i as usize] = byte;
                }

                unsafe {
                    std::ptr::copy_nonoverlapping(
                        raw.as_ptr(),
                        (dest as *mut T).cast::<u8>(),
                        raw.len(),
                    );
                }
                true
            }
            MemoryAccess::Store(value) => {
                stats.data_stores += 1;
                let cost = self.caches.store_cost(addr);
                stats.cycles = stats.cycles.wrapping_add(cost);
                drop(stats);

                let src = (&value as *const T).cast::<u8>();
                self.update_bounds(addr, size);
                for i in 0..size {
                    let byte = unsafe { *src.add(i as usize) };
                    self.bytes.insert(addr.wrapping_add(i), byte);
                }
                true
            }
        }
    }
}

#[derive(Debug)]
struct HecateClock {
    stats: Rc<RefCell<RunStats>>,
    max_instructions: Option<u64>,
}

impl HecateClock {
    fn new(stats: Rc<RefCell<RunStats>>, max_instructions: Option<u64>) -> Self {
        Self {
            stats,
            max_instructions,
        }
    }

    fn set_max_instructions(&mut self, max_instructions: Option<u64>) {
        self.max_instructions = max_instructions;
    }
}

impl Clock for HecateClock {
    fn read_cycle(&self) -> u64 {
        self.stats.borrow().cycles
    }

    fn read_time(&self) -> u64 {
        self.stats.borrow().cycles
    }

    fn read_instret(&self) -> u64 {
        self.stats.borrow().instret
    }

    fn progress(&mut self, _op: &Op) {
        let mut stats = self.stats.borrow_mut();
        stats.instret = stats.instret.wrapping_add(1);
        stats.cycles = stats.cycles.wrapping_add(1);
    }

    fn check_quota(&self) -> bool {
        if let Some(max) = self.max_instructions {
            self.stats.borrow().instret < max
        } else {
            true
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstructionFlow {
    Linear,
    Call { return_pc: u32 },
}

#[derive(Debug, Clone)]
struct DecodedInstruction {
    snapshot: InstructionSnapshot,
    flow: InstructionFlow,
}

fn read_u32_field(packed_field: *const u32) -> u32 {
    u32::from_le(unsafe { packed_field.read_unaligned() })
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn syscall_cycles_for(
    code: u32,
    default_cycles: u64,
    syscall_cycles: &HashMap<u32, u64>,
) -> u64 {
    syscall_cycles.get(&code).copied().unwrap_or(default_cycles)
}

fn handle_syscall(
    state: &mut CpuState,
    memory: &HecateMemory,
    config: &SimConfig,
    code: u32,
) -> (bool, u64, u64) {
    match code {
        64 => {
            let fd = state.x[10];
            let buf_addr = state.x[11];
            let len = state.x[12];

            if fd == 1 || fd == 2 {
                let bytes = memory.read_program_bytes(buf_addr, len);
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(&bytes);
                let _ = out.flush();
                state.x[10] = len;
                let io_bytes = len as u64;
                let extra_cycles = io_bytes.wrapping_mul(config.io_cycles_per_byte);
                (true, extra_cycles, io_bytes)
            } else {
                state.x[10] = 0;
                (true, 0, 0)
            }
        }
        93 => (false, 0, 0),
        _ => (true, 0, 0),
    }
}

pub struct HecateVm {
    options: VmRuntimeOptions,
    config: SimConfig,
    shared_stats: Rc<RefCell<RunStats>>,
    memory: HecateMemory,
    state: CpuState,
    clock: HecateClock,
    loaded_binary: Option<LoadedBinary>,
    entry_point: u32,
    halted: bool,
    running: bool,
    stop_reason: VmStopReason,
    breakpoints: HashSet<u32>,
}

impl HecateVm {
    pub fn new(options: VmRuntimeOptions, config: SimConfig) -> Self {
        let shared_stats = Rc::new(RefCell::new(RunStats::default()));
        let memory = HecateMemory::new(Rc::clone(&shared_stats), options, &config);
        let clock = HecateClock::new(Rc::clone(&shared_stats), options.max_instructions);

        Self {
            options,
            config,
            shared_stats,
            memory,
            state: CpuState::new(0),
            clock,
            loaded_binary: None,
            entry_point: 0,
            halted: true,
            running: false,
            stop_reason: VmStopReason::NoProgramLoaded,
            breakpoints: HashSet::new(),
        }
    }

    pub fn options(&self) -> VmRuntimeOptions {
        self.options
    }

    pub fn config(&self) -> &SimConfig {
        &self.config
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    pub fn entry_point(&self) -> u32 {
        self.entry_point
    }

    pub fn loaded_binary(&self) -> Option<&LoadedBinary> {
        self.loaded_binary.as_ref()
    }

    pub fn set_breakpoints<I>(&mut self, breakpoints: I)
    where
        I: IntoIterator<Item = u32>,
    {
        self.breakpoints.clear();
        self.breakpoints.extend(breakpoints);
    }

    fn clear_stats(&mut self) {
        *self.shared_stats.borrow_mut() = RunStats::default();
    }

    fn reset_clock_and_caches(&mut self) {
        self.clock = HecateClock::new(Rc::clone(&self.shared_stats), self.options.max_instructions);
        self.memory.reset_caches(self.options, &self.config);
    }

    pub fn load(&mut self, name: impl Into<String>, blob: &[u8]) -> anyhow::Result<VmState> {
        self.clear_stats();
        self.reset_clock_and_caches();
        self.memory.clear_bytes();

        let name = name.into();
        let entry = load_elf_bytes(&name, blob, &mut self.memory)?;
        self.loaded_binary = Some(LoadedBinary {
            name,
            sha256: hash_bytes(blob),
            bytes: blob.to_vec(),
        });
        self.entry_point = entry;
        self.state = CpuState::new(entry);
        self.halted = false;
        self.running = false;
        self.stop_reason = VmStopReason::ProgramLoaded;
        Ok(self.state())
    }

    pub fn load_file(&mut self, path: &Path) -> anyhow::Result<VmState> {
        let data = std::fs::read(path)
            .with_context(|| format!("Failed to read ELF: {}", path.display()))?;
        self.load(path.display().to_string(), &data)
    }

    pub fn step(&mut self) -> anyhow::Result<VmState> {
        self.step_count(1)
    }

    pub fn step_count(&mut self, count: u64) -> anyhow::Result<VmState> {
        if self.halted {
            return Err(anyhow!("Program is halted or not loaded"));
        }

        self.running = false;
        let total = count.max(1);
        for _ in 0..total {
            let current = self.shared_stats.borrow().instret;
            let target = current.saturating_add(1);
            self.execute_until(Some(target));
            if self.halted || matches!(self.stop_reason, VmStopReason::Breakpoint { .. }) {
                return Ok(self.state());
            }
        }
        self.stop_reason = VmStopReason::StepComplete;
        Ok(self.state())
    }

    pub fn step_over(&mut self) -> anyhow::Result<VmState> {
        if self.halted {
            return Err(anyhow!("Program is halted or not loaded"));
        }

        let decoded = self.decode_current_instruction();
        let InstructionFlow::Call { return_pc } = decoded.flow else {
            return self.step();
        };

        self.step()?;
        while !self.halted && self.state.pc != return_pc {
            self.step()?;
            if matches!(self.stop_reason, VmStopReason::Breakpoint { .. }) {
                return Ok(self.state());
            }
        }

        if !self.halted {
            self.stop_reason = VmStopReason::StepOverComplete;
        }
        Ok(self.state())
    }

    pub fn step_out(&mut self) -> anyhow::Result<VmState> {
        if self.halted {
            return Err(anyhow!("Program is halted or not loaded"));
        }

        let initial_sp = self.state.x[2];
        let return_pc = self.state.x[1];
        if return_pc == 0 {
            return self.step();
        }

        loop {
            self.step()?;
            if self.halted || matches!(self.stop_reason, VmStopReason::Breakpoint { .. }) {
                break;
            }
            if self.state.pc == return_pc && self.state.x[2] >= initial_sp {
                self.stop_reason = VmStopReason::StepOutComplete;
                break;
            }
        }

        Ok(self.state())
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        if self.halted {
            return Err(anyhow!("Program is halted or not loaded"));
        }
        self.running = true;
        self.stop_reason = VmStopReason::Running;
        Ok(())
    }

    pub fn pause(&mut self) -> VmState {
        self.running = false;
        if !self.halted {
            self.stop_reason = VmStopReason::Paused;
        }
        self.state()
    }

    pub fn tick_running(&mut self, quantum: u64) {
        if !self.running || self.halted {
            self.running = false;
            return;
        }

        let current = self.shared_stats.borrow().instret;
        let target = current.saturating_add(quantum.max(1));
        self.execute_until(Some(target));

        if self.running && !self.halted {
            self.stop_reason = VmStopReason::Running;
        }
    }

    pub fn read(&self, addr: u32, len: u32) -> Result<MemoryReadResult, MemoryAccessError> {
        self.memory.read_external(addr, len)
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemoryAccessError> {
        self.memory.write_external(addr, data)
    }

    pub fn state(&self) -> VmState {
        VmState {
            running: self.running,
            halted: self.halted,
            pc: self.state.pc,
            entry_point: self.entry_point,
            loaded_binary_name: self
                .loaded_binary
                .as_ref()
                .map(|binary| binary.name.clone()),
            loaded_binary_hash: self
                .loaded_binary
                .as_ref()
                .map(|binary| binary.sha256.clone()),
            stop_reason: self.stop_reason.clone(),
            current_instruction: self.decode_current_instruction().snapshot,
            registers: self.state.x,
            #[cfg(feature = "rv32fd")]
            floating_registers: self.state.f.map(|value| value.0),
            fcsr: self.state.fcsr,
            reservation: self.state.reservation,
            stats: self.shared_stats.borrow().clone(),
        }
    }

    pub fn reset(&mut self, policy: ResetMemoryPolicy) -> anyhow::Result<VmState> {
        self.clear_stats();
        self.reset_clock_and_caches();
        self.running = false;

        match policy {
            ResetMemoryPolicy::Ignore => {}
            ResetMemoryPolicy::Zero => self.memory.zero_current_bounds(),
            ResetMemoryPolicy::Randomize => self.memory.randomize_current_bounds(),
        }

        if let Some(binary) = self.loaded_binary.clone() {
            let entry = load_elf_bytes(&binary.name, &binary.bytes, &mut self.memory)?;
            self.entry_point = entry;
            self.state = CpuState::new(entry);
            self.halted = false;
            self.stop_reason = VmStopReason::ProgramLoaded;
        } else {
            self.entry_point = 0;
            self.state = CpuState::new(0);
            self.halted = true;
            self.stop_reason = VmStopReason::NoProgramLoaded;
        }

        Ok(self.state())
    }

    pub fn dump(&self) -> VmDump {
        VmDump {
            options: self.options,
            config: self.config.clone(),
            loaded_binary: self.loaded_binary.clone(),
            entry_point: self.entry_point,
            running: self.running,
            halted: self.halted,
            stop_reason: self.stop_reason.clone(),
            cpu: CpuSnapshot::from_cpu_state(&self.state),
            stats: self.shared_stats.borrow().clone(),
            memory: self.memory.dump(),
        }
    }

    pub fn restore(&mut self, dump: VmDump) -> anyhow::Result<VmState> {
        self.options = dump.options;
        self.config = dump.config;
        self.loaded_binary = dump.loaded_binary;
        self.entry_point = dump.entry_point;
        self.running = dump.running;
        self.halted = dump.halted;
        self.stop_reason = dump.stop_reason;
        self.shared_stats = Rc::new(RefCell::new(dump.stats));
        self.state = dump.cpu.into_cpu_state();
        self.memory = HecateMemory::from_dump(
            Rc::clone(&self.shared_stats),
            self.options,
            dump.memory,
            &self.config,
        );
        self.clock = HecateClock::new(Rc::clone(&self.shared_stats), self.options.max_instructions);
        Ok(self.state())
    }

    fn effective_quota(&self, local_quota: Option<u64>) -> Option<u64> {
        match (self.options.max_instructions, local_quota) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn hit_breakpoint(&self) -> bool {
        self.breakpoints.contains(&self.state.pc)
    }

    fn execute_until(&mut self, local_quota: Option<u64>) {
        let cap = self.effective_quota(local_quota);
        self.clock.set_max_instructions(cap);

        loop {
            let (error, _last_op) = {
                let mut interp = Interp::new(&mut self.state, &mut self.memory, &mut self.clock);
                interp.run()
            };

            match error {
                CpuError::Ecall => {
                    let syscall_code = self.state.x[17];
                    let syscall_cycles = syscall_cycles_for(
                        syscall_code,
                        self.config.default_syscall_cycles,
                        &self.config.syscall_cycles,
                    );
                    {
                        let mut stats = self.shared_stats.borrow_mut();
                        stats.instret = stats.instret.wrapping_add(1);
                        stats.syscall_count = stats.syscall_count.wrapping_add(1);
                        stats.syscall_cycles = stats.syscall_cycles.wrapping_add(syscall_cycles);
                        stats.cycles = stats.cycles.wrapping_add(syscall_cycles);
                        *stats.syscall_hits.entry(syscall_code).or_insert(0) += 1;
                        *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) +=
                            syscall_cycles;
                    }

                    let (should_continue, extra_cycles, io_bytes_written) =
                        handle_syscall(&mut self.state, &self.memory, &self.config, syscall_code);

                    if extra_cycles != 0 || io_bytes_written != 0 {
                        let mut stats = self.shared_stats.borrow_mut();
                        stats.io_cycles = stats.io_cycles.wrapping_add(extra_cycles);
                        stats.io_bytes_written =
                            stats.io_bytes_written.wrapping_add(io_bytes_written);
                        stats.syscall_cycles = stats.syscall_cycles.wrapping_add(extra_cycles);
                        stats.cycles = stats.cycles.wrapping_add(extra_cycles);
                        *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) +=
                            extra_cycles;
                    }

                    if !should_continue {
                        self.halted = true;
                        self.running = false;
                        self.stop_reason = VmStopReason::ProgramExited;
                        break;
                    }

                    self.state.pc = self.state.pc.wrapping_add(4);
                    if self.hit_breakpoint() {
                        self.running = false;
                        self.stop_reason = VmStopReason::Breakpoint { pc: self.state.pc };
                        break;
                    }
                }
                CpuError::Ebreak => {
                    self.running = false;
                    self.stop_reason = VmStopReason::Breakpoint { pc: self.state.pc };
                    break;
                }
                CpuError::QuotaExceeded => {
                    self.stop_reason = VmStopReason::QuotaReached;
                    break;
                }
                other => {
                    self.halted = true;
                    self.running = false;
                    self.stop_reason = VmStopReason::CpuError {
                        error: format!("{other:?}"),
                    };
                    break;
                }
            }
        }
    }

    fn decode_current_instruction(&self) -> DecodedInstruction {
        decode_instruction(&self.memory, self.state.pc)
    }
}

impl CpuSnapshot {
    fn from_cpu_state(state: &CpuState) -> Self {
        Self {
            x: state.x,
            #[cfg(feature = "rv32fd")]
            f: state.f.map(|value| value.0),
            pc: state.pc,
            fcsr: state.fcsr,
            reservation: state.reservation,
        }
    }

    fn into_cpu_state(self) -> CpuState {
        let mut state = CpuState::new(self.pc);
        state.x = self.x;
        #[cfg(feature = "rv32fd")]
        {
            state.f = self.f.map(Sf64);
        }
        state.pc = self.pc;
        state.fcsr = self.fcsr;
        state.reservation = self.reservation;
        state
    }
}

pub(crate) fn load_elf_bytes(
    label: &str,
    data: &[u8],
    memory: &mut HecateMemory,
) -> anyhow::Result<u32> {
    let elf = Elf32::parse(data).map_err(|e| anyhow!("ELF parse failed for {label}: {e}"))?;

    for (ph, segment) in elf.ph.iter().zip(elf.p.iter()) {
        let typ = read_u32_field(std::ptr::addr_of!(ph.typ));
        if typ != ELF_PROGRAM_TYPE_LOADABLE {
            continue;
        }

        let vaddr = read_u32_field(std::ptr::addr_of!(ph.vaddr));
        let filesz = read_u32_field(std::ptr::addr_of!(ph.filesz));
        let memsz = read_u32_field(std::ptr::addr_of!(ph.memsz));

        let file_len = filesz.min(segment.len() as u32) as usize;
        memory.write_bytes_raw(vaddr, &segment[..file_len]);

        if memsz > filesz {
            memory.zero_fill(vaddr.wrapping_add(filesz), memsz - filesz);
        }
    }

    let entry = read_u32_field(std::ptr::addr_of!(elf.header.entry));
    Ok(entry)
}

fn xreg(reg: u32) -> String {
    format!("x{}", reg)
}

fn sign_extend(value: u32, width: u32) -> i32 {
    let shift = 32 - width;
    ((value << shift) as i32) >> shift
}

fn decode_rvc_mnemonic(inst: u16) -> String {
    let funct3 = (inst >> 13) & 0x7;
    let quadrant = inst & 0x3;

    match (quadrant, funct3) {
        (0b01, 0b000) => "c.addi".to_string(),
        (0b01, 0b001) => "c.jal".to_string(),
        (0b01, 0b010) => "c.li".to_string(),
        (0b01, 0b011) => "c.lui/addi16sp".to_string(),
        (0b01, 0b100) => "c.misc-alu".to_string(),
        (0b01, 0b101) => "c.j".to_string(),
        (0b01, 0b110) => "c.beqz".to_string(),
        (0b01, 0b111) => "c.bnez".to_string(),
        (0b10, 0b100) => "c.jr/mv/ebreak/jalr/add".to_string(),
        _ => "c.unknown".to_string(),
    }
}

fn decode_rv32_mnemonic(inst: u32, pc: u32) -> String {
    let opcode = inst & 0x7f;
    let rd = (inst >> 7) & 0x1f;
    let funct3 = (inst >> 12) & 0x7;
    let rs1 = (inst >> 15) & 0x1f;
    let rs2 = (inst >> 20) & 0x1f;
    let funct7 = (inst >> 25) & 0x7f;

    let imm_i = sign_extend(inst >> 20, 12);
    let imm_s = sign_extend(((inst >> 25) << 5) | ((inst >> 7) & 0x1f), 12);
    let imm_b = sign_extend(
        ((inst >> 31) << 12)
            | (((inst >> 7) & 0x1) << 11)
            | (((inst >> 25) & 0x3f) << 5)
            | (((inst >> 8) & 0xf) << 1),
        13,
    );
    let imm_u = inst & 0xffff_f000;
    let imm_j = sign_extend(
        ((inst >> 31) << 20)
            | (((inst >> 12) & 0xff) << 12)
            | (((inst >> 20) & 0x1) << 11)
            | (((inst >> 21) & 0x3ff) << 1),
        21,
    );

    match opcode {
        0x37 => format!("lui {}, 0x{:x}", xreg(rd), imm_u),
        0x17 => format!("auipc {}, 0x{:x}", xreg(rd), imm_u),
        0x6f => format!(
            "jal {}, {} -> 0x{:08x}",
            xreg(rd),
            imm_j,
            pc.wrapping_add(imm_j as u32)
        ),
        0x67 => format!("jalr {}, {}({})", xreg(rd), imm_i, xreg(rs1)),
        0x63 => {
            let mnem = match funct3 {
                0b000 => "beq",
                0b001 => "bne",
                0b100 => "blt",
                0b101 => "bge",
                0b110 => "bltu",
                0b111 => "bgeu",
                _ => "b?",
            };
            format!(
                "{} {}, {}, {} -> 0x{:08x}",
                mnem,
                xreg(rs1),
                xreg(rs2),
                imm_b,
                pc.wrapping_add(imm_b as u32)
            )
        }
        0x03 => {
            let mnem = match funct3 {
                0b000 => "lb",
                0b001 => "lh",
                0b010 => "lw",
                0b100 => "lbu",
                0b101 => "lhu",
                _ => "l?",
            };
            format!("{} {}, {}({})", mnem, xreg(rd), imm_i, xreg(rs1))
        }
        0x23 => {
            let mnem = match funct3 {
                0b000 => "sb",
                0b001 => "sh",
                0b010 => "sw",
                _ => "s?",
            };
            format!("{} {}, {}({})", mnem, xreg(rs2), imm_s, xreg(rs1))
        }
        0x13 => match funct3 {
            0b000 => format!("addi {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b010 => format!("slti {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b011 => format!("sltiu {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b100 => format!("xori {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b110 => format!("ori {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b111 => format!("andi {}, {}, {}", xreg(rd), xreg(rs1), imm_i),
            0b001 => {
                let shamt = (inst >> 20) & 0x1f;
                format!("slli {}, {}, {}", xreg(rd), xreg(rs1), shamt)
            }
            0b101 => {
                let shamt = (inst >> 20) & 0x1f;
                if funct7 == 0x20 {
                    format!("srai {}, {}, {}", xreg(rd), xreg(rs1), shamt)
                } else {
                    format!("srli {}, {}, {}", xreg(rd), xreg(rs1), shamt)
                }
            }
            _ => "i-unknown".to_string(),
        },
        0x33 => {
            let mnem = match (funct7, funct3) {
                (0x00, 0b000) => "add",
                (0x20, 0b000) => "sub",
                (0x00, 0b001) => "sll",
                (0x00, 0b010) => "slt",
                (0x00, 0b011) => "sltu",
                (0x00, 0b100) => "xor",
                (0x00, 0b101) => "srl",
                (0x20, 0b101) => "sra",
                (0x00, 0b110) => "or",
                (0x00, 0b111) => "and",
                (0x01, 0b000) => "mul",
                (0x01, 0b001) => "mulh",
                (0x01, 0b010) => "mulhsu",
                (0x01, 0b011) => "mulhu",
                (0x01, 0b100) => "div",
                (0x01, 0b101) => "divu",
                (0x01, 0b110) => "rem",
                (0x01, 0b111) => "remu",
                _ => "r-unknown",
            };
            format!("{} {}, {}, {}", mnem, xreg(rd), xreg(rs1), xreg(rs2))
        }
        0x73 => {
            if inst == 0x0000_0073 {
                "ecall".to_string()
            } else if inst == 0x0010_0073 {
                "ebreak".to_string()
            } else {
                "system".to_string()
            }
        }
        0x0f => "fence".to_string(),
        _ => format!("unknown(opcode=0x{:02x})", opcode),
    }
}

fn decode_instruction(memory: &HecateMemory, pc: u32) -> DecodedInstruction {
    let b0 = memory.bytes.get(&pc).copied();
    let b1 = memory.bytes.get(&pc.wrapping_add(1)).copied();
    let (Some(b0), Some(b1)) = (b0, b1) else {
        return DecodedInstruction {
            snapshot: InstructionSnapshot {
                text: "Unavailable (memory unmapped at PC)".to_string(),
                hex: None,
                size: None,
                bytes: None,
            },
            flow: InstructionFlow::Linear,
        };
    };

    let halfword = (b0 as u16) | ((b1 as u16) << 8);
    if (halfword & 0b11) != 0b11 {
        let bytes = format!("{:02x} {:02x}", b0, b1);
        let quadrant = halfword & 0x3;
        let funct3 = (halfword >> 13) & 0x7;
        let rs1 = ((halfword >> 7) & 0x1f) as u32;
        let rs2 = ((halfword >> 2) & 0x1f) as u32;
        let bit12 = ((halfword >> 12) & 0x1) != 0;
        let flow = if quadrant == 0b01 && funct3 == 0b001 {
            InstructionFlow::Call {
                return_pc: pc.wrapping_add(2),
            }
        } else if quadrant == 0b10 && funct3 == 0b100 && bit12 && rs2 == 0 && rs1 != 0 {
            InstructionFlow::Call {
                return_pc: pc.wrapping_add(2),
            }
        } else {
            InstructionFlow::Linear
        };

        return DecodedInstruction {
            snapshot: InstructionSnapshot {
                text: format!("{} ({})", decode_rvc_mnemonic(halfword), bytes),
                hex: Some(format!("0x{:04x}", halfword)),
                size: Some(2),
                bytes: Some(bytes),
            },
            flow,
        };
    }

    let b2 = memory.bytes.get(&pc.wrapping_add(2)).copied();
    let b3 = memory.bytes.get(&pc.wrapping_add(3)).copied();
    let (Some(b2), Some(b3)) = (b2, b3) else {
        let bytes = format!("{:02x} {:02x}", b0, b1);
        return DecodedInstruction {
            snapshot: InstructionSnapshot {
                text: "Unavailable (incomplete 32-bit instruction at PC)".to_string(),
                hex: None,
                size: None,
                bytes: Some(bytes),
            },
            flow: InstructionFlow::Linear,
        };
    };

    let raw = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16) | ((b3 as u32) << 24);
    let bytes = format!("{:02x} {:02x} {:02x} {:02x}", b0, b1, b2, b3);
    let opcode = raw & 0x7f;
    let rd = (raw >> 7) & 0x1f;
    let funct3 = (raw >> 12) & 0x7;
    let rs1 = (raw >> 15) & 0x1f;
    let flow = if opcode == 0x6f && (rd == 1 || rd == 5) {
        InstructionFlow::Call {
            return_pc: pc.wrapping_add(4),
        }
    } else if opcode == 0x67 && funct3 == 0 && (rd == 1 || rd == 5) {
        InstructionFlow::Call {
            return_pc: pc.wrapping_add(4),
        }
    } else if opcode == 0x67 && funct3 == 0 && rd == 0 && rs1 == 1 {
        InstructionFlow::Linear
    } else {
        InstructionFlow::Linear
    };

    DecodedInstruction {
        snapshot: InstructionSnapshot {
            text: format!("{} ({})", decode_rv32_mnemonic(raw, pc), bytes),
            hex: Some(format!("0x{:08x}", raw)),
            size: Some(4),
            bytes: Some(bytes),
        },
        flow,
    }
}
