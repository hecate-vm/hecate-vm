use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::size_of;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use rvsim::elf::{Elf32, ELF_PROGRAM_TYPE_LOADABLE};
use rvsim::{Clock, CpuError, CpuState, Interp, Memory, MemoryAccess, Op};

const L1_LATENCY: u64 = 3;
const L2_LATENCY: u64 = 11;
const L3_LATENCY: u64 = 50;
const MEMORY_LATENCY: u64 = 125;
const STORE_LATENCY: u64 = 1;

#[derive(Debug, Clone, Default)]
struct CacheHits {
    l1i: u64,
    l1d: u64,
    l2: u64,
    l3: u64,
}

#[derive(Debug, Clone, Default)]
struct RunStats {
    cycles: u64,
    instret: u64,
    memory_access_count: u64,
    instruction_fetches: u64,
    data_loads: u64,
    data_stores: u64,
    cache_hits: CacheHits,
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
}

impl CacheHierarchy {
    fn new(line_size: u32, l1_bytes: u32, l2_bytes: u32, l3_bytes: u32) -> Self {
        let lines = |bytes: u32| -> usize { (bytes / line_size).max(1) as usize };
        Self {
            line_size,
            l1i: CacheLevel::new(lines(l1_bytes)),
            l1d: CacheLevel::new(lines(l1_bytes)),
            l2: CacheLevel::new(lines(l2_bytes)),
            l3: CacheLevel::new(lines(l3_bytes)),
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
            return L1_LATENCY;
        }

        if self.l2.touch(line) {
            stats.cache_hits.l2 += 1;
            l1.insert(line);
            return L2_LATENCY;
        }

        if self.l3.touch(line) {
            stats.cache_hits.l3 += 1;
            self.l2.insert(line);
            l1.insert(line);
            return L3_LATENCY;
        }

        self.l3.insert(line);
        self.l2.insert(line);
        l1.insert(line);
        MEMORY_LATENCY
    }

    fn store_cost(&mut self, addr: u32) -> u64 {
        let line = self.line(addr);
        self.l1d.insert(line);
        self.l2.insert(line);
        self.l3.insert(line);
        STORE_LATENCY
    }
}

#[derive(Debug)]
struct HecateMemory {
    bytes: HashMap<u32, u8>,
    stats: Rc<RefCell<RunStats>>,
    caches: CacheHierarchy,
}

impl HecateMemory {
    fn new(stats: Rc<RefCell<RunStats>>, line_size: u32, l1: u32, l2: u32, l3: u32) -> Self {
        Self {
            bytes: HashMap::new(),
            stats,
            caches: CacheHierarchy::new(line_size, l1, l2, l3),
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
        path: PathBuf,
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
    },
}

fn read_u32_field(packed_field: *const u32) -> u32 {
    u32::from_le(unsafe { packed_field.read_unaligned() })
}

fn load_elf(path: &PathBuf, memory: &mut HecateMemory) -> anyhow::Result<u32> {
    let data =
        std::fs::read(path).with_context(|| format!("Failed to read ELF: {}", path.display()))?;

    let elf = Elf32::parse(&data).map_err(|e| anyhow!("ELF parse failed: {e}"))?;

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

fn report_result(error: CpuError, state: &CpuState, stats: &RunStats, dump_registers: bool) {
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
    println!("Cache hits L1I: {}", stats.cache_hits.l1i);
    println!("Cache hits L1D: {}", stats.cache_hits.l1d);
    println!("Cache hits L2: {}", stats.cache_hits.l2);
    println!("Cache hits L3: {}", stats.cache_hits.l3);

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
) -> anyhow::Result<()> {
    let shared_stats = Rc::new(RefCell::new(RunStats::default()));
    let mut memory = HecateMemory::new(
        Rc::clone(&shared_stats),
        cache_line_size,
        l1_size,
        l2_size,
        l3_size,
    );

    let entry = load_elf(&path, &mut memory)?;

    let mut state = CpuState::new(entry);
    let mut clock = HecateClock::new(Rc::clone(&shared_stats), max_instructions);

    let (error, _last_op) = {
        let mut interp = Interp::new(&mut state, &mut memory, &mut clock);
        interp.run()
    };

    let stats = shared_stats.borrow().clone();
    report_result(error, &state, &stats, dump_registers);

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
        } => run_elf(
            path,
            cache_line_size,
            l1_size,
            l2_size,
            l3_size,
            max_instructions,
            dump_registers,
        )?,
    }

    Ok(())
}
