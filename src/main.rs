use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::mem::size_of;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use rvsim::elf::{ELF_PROGRAM_TYPE_LOADABLE, Elf32};
use rvsim::{Clock, CpuError, CpuState, Interp, Memory, MemoryAccess, Op};
use serde::{Deserialize, Serialize};

mod debug_ui;

mod bundled_examples {
    include!(concat!(env!("OUT_DIR"), "/bundled_examples.rs"));
}

const DEFAULT_CONFIG: &str = include_str!("default.toml");

#[derive(Debug, Clone, Default, Serialize)]
struct CacheHits {
    l1i: u64,
    l1d: u64,
    l2: u64,
    l3: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct RunStats {
    cycles: u64,
    instret: u64,
    memory_access_count: u64,
    instruction_fetches: u64,
    data_loads: u64,
    data_stores: u64,
    syscall_count: u64,
    syscall_cycles: u64,
    syscall_hits: HashMap<u32, u64>,
    syscall_cycle_totals: HashMap<u32, u64>,
    cache_hits: CacheHits,
    io_bytes_written: u64,
    io_cycles: u64,
}

// ---------------------------------------------------------------------------
// Simulation config (loaded from TOML)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct LatencyConfigRaw {
    l1: Option<u64>,
    l2: Option<u64>,
    l3: Option<u64>,
    memory: Option<u64>,
    store: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SimConfigRaw {
    default_syscall_cycles: Option<u64>,
    io_cycles_per_byte: Option<u64>,
    #[serde(default)]
    latency: LatencyConfigRaw,
    #[serde(default)]
    syscall_cycles: HashMap<String, u64>,
}

impl SimConfigRaw {
    /// Overlay `user` on top of `self` (user values win; syscall_cycles are merged).
    fn merge_with(self, user: SimConfigRaw) -> SimConfigRaw {
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

    fn resolve(self) -> anyhow::Result<SimConfig> {
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

#[derive(Debug, Clone)]
struct SimConfig {
    default_syscall_cycles: u64,
    io_cycles_per_byte: u64,
    l1_latency: u64,
    l2_latency: u64,
    l3_latency: u64,
    memory_latency: u64,
    store_latency: u64,
    syscall_cycles: HashMap<u32, u64>,
}

fn load_config(path: &std::path::Path) -> anyhow::Result<SimConfigRaw> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("Failed to parse config: {}", path.display()))
}

fn parse_u32_auto(raw: &str) -> Result<u32, String> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| format!("invalid u32 hex '{raw}': {e}"))
    } else {
        raw.parse::<u32>()
            .map_err(|e| format!("invalid u32 '{raw}': {e}"))
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
struct CacheHierarchy {
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
    fn new(
        line_size: u32,
        l1_bytes: u32,
        l2_bytes: u32,
        l3_bytes: u32,
        l1_latency: u64,
        l2_latency: u64,
        l3_latency: u64,
        memory_latency: u64,
        store_latency: u64,
    ) -> Self {
        let lines = |bytes: u32| -> usize { (bytes / line_size).max(1) as usize };
        Self {
            line_size,
            l1i: CacheLevel::new(lines(l1_bytes)),
            l1d: CacheLevel::new(lines(l1_bytes)),
            l2: CacheLevel::new(lines(l2_bytes)),
            l3: CacheLevel::new(lines(l3_bytes)),
            l1_latency,
            l2_latency,
            l3_latency,
            memory_latency,
            store_latency,
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
struct HecateMemory {
    bytes: HashMap<u32, u8>,
    stats: Rc<RefCell<RunStats>>,
    caches: CacheHierarchy,
}

impl HecateMemory {
    fn new(
        stats: Rc<RefCell<RunStats>>,
        line_size: u32,
        l1: u32,
        l2: u32,
        l3: u32,
        config: &SimConfig,
    ) -> Self {
        Self {
            bytes: HashMap::new(),
            stats,
            caches: CacheHierarchy::new(
                line_size,
                l1,
                l2,
                l3,
                config.l1_latency,
                config.l2_latency,
                config.l3_latency,
                config.memory_latency,
                config.store_latency,
            ),
        }
    }

    fn write_bytes(&mut self, addr: u32, data: &[u8]) {
        for (offset, byte) in data.iter().enumerate() {
            self.bytes.insert(addr.wrapping_add(offset as u32), *byte);
        }
    }

    fn zero_fill(&mut self, addr: u32, len: u32) {
        for i in 0..len {
            self.bytes.insert(addr.wrapping_add(i), 0);
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

                let src = (&value as *const T).cast::<u8>();
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

#[derive(Parser)]
#[command(name = "hecate-vm")]
#[command(about = "Hecate RISC-V MVP runner")]
struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Run {
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 64)]
        cache_line_size: u32,
        #[arg(long, default_value_t = 32 * 1024)]
        l1_size: u32,
        #[arg(long, default_value_t = 256 * 1024)]
        l2_size: u32,
        #[arg(long, default_value_t = 8 * 1024 * 1024)]
        l3_size: u32,
        #[arg(long)]
        max_instructions: Option<u64>,
        #[arg(long)]
        dump_registers: bool,
        /// Path to a TOML config file (merged with the built-in defaults).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Enable the browser-based debug UI and remote control API.
        #[arg(long, default_value_t = false)]
        debug_ui: bool,
        /// TCP port for the debug UI and API (localhost only).
        #[arg(long, default_value_t = 8581)]
        debug_port: u16,
    },
}

fn read_u32_field(packed_field: *const u32) -> u32 {
    u32::from_le(unsafe { packed_field.read_unaligned() })
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
        memory.write_bytes(vaddr, &segment[..file_len]);

        if memsz > filesz {
            memory.zero_fill(vaddr.wrapping_add(filesz), memsz - filesz);
        }
    }

    let entry = read_u32_field(std::ptr::addr_of!(elf.header.entry));
    Ok(entry)
}

fn load_elf(path: &PathBuf, memory: &mut HecateMemory) -> anyhow::Result<u32> {
    let data =
        std::fs::read(path).with_context(|| format!("Failed to read ELF: {}", path.display()))?;

    load_elf_bytes(&path.display().to_string(), &data, memory)
}

fn syscall_cycles_for(code: u32, default_cycles: u64, syscall_cycles: &HashMap<u32, u64>) -> u64 {
    syscall_cycles.get(&code).copied().unwrap_or(default_cycles)
}

fn read_program_bytes(memory: &HecateMemory, addr: u32, len: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len as usize);
    for i in 0..len {
        let b = memory
            .bytes
            .get(&addr.wrapping_add(i))
            .copied()
            .unwrap_or(0);
        bytes.push(b);
    }
    bytes
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
                let bytes = read_program_bytes(memory, buf_addr, len);
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

fn report_result(
    error: CpuError,
    state: &CpuState,
    stats: &RunStats,
    dump_registers: bool,
    config: &SimConfig,
) {
    println!();
    println!("========== RESULT/STATS ==========");
    println!();

    println!("Stop reason: {error:?}");
    if error == CpuError::Ecall {
        println!("ECALL a7(code)={} a0(value)={}", state.x[17], state.x[10]);
    }
    println!("PC: {:#010x}", state.pc);
    println!("Score (cycles): {}", stats.cycles);
    println!("Instructions retired: {}", stats.instret);
    println!("Memory accesses: {}", stats.memory_access_count);
    println!("Instruction fetches: {}", stats.instruction_fetches);
    println!("Data loads: {}", stats.data_loads);
    println!("Data stores: {}", stats.data_stores);
    println!("Syscalls: {}", stats.syscall_count);
    println!("Syscall cycles contribution: {}", stats.syscall_cycles);
    println!("I/O cycles contribution: {}", stats.io_cycles);
    println!("IO Bytes Written: {}", stats.io_bytes_written);
    println!("Cache hits L1I: {}", stats.cache_hits.l1i);
    println!("Cache hits L1D: {}", stats.cache_hits.l1d);
    println!("Cache hits L2: {}", stats.cache_hits.l2);
    println!("Cache hits L3: {}", stats.cache_hits.l3);

    if !stats.syscall_hits.is_empty() {
        println!();
        println!("Syscall breakdown:");
        let mut calls: Vec<(u32, u64)> = stats
            .syscall_hits
            .iter()
            .map(|(code, count)| (*code, *count))
            .collect();
        calls.sort_by_key(|(code, _)| *code);

        for (code, count) in calls {
            let base_cycles =
                syscall_cycles_for(code, config.default_syscall_cycles, &config.syscall_cycles);
            let subtotal = stats.syscall_cycle_totals.get(&code).copied().unwrap_or(0);
            let variable_cycles = subtotal.saturating_sub(base_cycles.saturating_mul(count));
            println!(
                "  syscall {}: count={} base_cycles_each={} variable_cycles={} subtotal={}",
                code, count, base_cycles, variable_cycles, subtotal
            );
        }
    }

    if dump_registers {
        println!();
        println!("Registers:");
        for (idx, reg) in state.x.iter().enumerate() {
            println!("x{idx:02}: {:#010x} ({})", reg, *reg as i32);
        }
    }
}

fn run_elf(
    path: PathBuf,
    cache_line_size: u32,
    l1_size: u32,
    l2_size: u32,
    l3_size: u32,
    max_instructions: Option<u64>,
    dump_registers: bool,
    config: SimConfig,
) -> anyhow::Result<()> {
    let shared_stats = Rc::new(RefCell::new(RunStats::default()));
    let mut memory = HecateMemory::new(
        Rc::clone(&shared_stats),
        cache_line_size,
        l1_size,
        l2_size,
        l3_size,
        &config,
    );

    let entry = load_elf(&path, &mut memory)?;

    let mut state = CpuState::new(entry);
    let mut clock = HecateClock::new(Rc::clone(&shared_stats), max_instructions);

    let error = loop {
        let (error, _last_op) = {
            let mut interp = Interp::new(&mut state, &mut memory, &mut clock);
            interp.run()
        };

        if error != CpuError::Ecall {
            break error;
        }

        let syscall_code = state.x[17];
        let syscall_cycles = syscall_cycles_for(
            syscall_code,
            config.default_syscall_cycles,
            &config.syscall_cycles,
        );
        {
            let mut stats = shared_stats.borrow_mut();
            stats.instret = stats.instret.wrapping_add(1);
            stats.syscall_count = stats.syscall_count.wrapping_add(1);
            stats.syscall_cycles = stats.syscall_cycles.wrapping_add(syscall_cycles);
            stats.cycles = stats.cycles.wrapping_add(syscall_cycles);
            *stats.syscall_hits.entry(syscall_code).or_insert(0) += 1;
            *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) += syscall_cycles;
        }

        let (should_continue, extra_cycles, io_bytes_written) =
            handle_syscall(&mut state, &memory, &config, syscall_code);
        if extra_cycles != 0 || io_bytes_written != 0 {
            let mut stats = shared_stats.borrow_mut();
            stats.io_cycles = stats.io_cycles.wrapping_add(extra_cycles);
            stats.io_bytes_written = stats.io_bytes_written.wrapping_add(io_bytes_written);
            stats.syscall_cycles = stats.syscall_cycles.wrapping_add(extra_cycles);
            stats.cycles = stats.cycles.wrapping_add(extra_cycles);
            *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) += extra_cycles;
        }

        if !should_continue {
            break error;
        }

        state.pc = state.pc.wrapping_add(4);
    };

    let stats = shared_stats.borrow().clone();
    report_result(error, &state, &stats, dump_registers, &config);

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let Args { action } = Args::parse();

    match action {
        Action::Run {
            path,
            cache_line_size,
            l1_size,
            l2_size,
            l3_size,
            max_instructions,
            dump_registers,
            config: config_path,
            debug_ui,
            debug_port,
        } => {
            let default_raw: SimConfigRaw = toml::from_str(DEFAULT_CONFIG)
                .context("Failed to parse built-in default config")?;
            let config = match config_path {
                Some(ref path) => {
                    let user_raw = load_config(path)?;
                    default_raw.merge_with(user_raw)
                }
                None => default_raw,
            }
            .resolve()?;

            if debug_ui {
                debug_ui::serve(
                    path,
                    cache_line_size,
                    l1_size,
                    l2_size,
                    l3_size,
                    max_instructions,
                    config,
                    debug_port,
                )?;
            } else {
                let Some(path) = path else {
                    return Err(anyhow!(
                        "A binary path is required unless --debug-ui is enabled."
                    ));
                };

                run_elf(
                    path,
                    cache_line_size,
                    l1_size,
                    l2_size,
                    l3_size,
                    max_instructions,
                    dump_registers,
                    config,
                )?;
            }
        }
    }

    Ok(())
}
