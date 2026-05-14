use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::mem::size_of;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, anyhow};
use rvsim::elf::{ELF_PROGRAM_TYPE_LOADABLE, Elf32};
use rvsim::{Clock, CpuState, Memory, MemoryAccess, Op};
use serde::{Deserialize, Serialize};

mod bundled_examples {
    include!(concat!(env!("OUT_DIR"), "/bundled_examples.rs"));
}

pub use bundled_examples::{BundledExample, EXAMPLES as BUNDLED_EXAMPLES};

pub const DEFAULT_CONFIG: &str = include_str!("default.toml");

#[derive(Debug, Clone, Default, Serialize)]
pub struct CacheHits {
    pub l1i: u64,
    pub l1d: u64,
    pub l2: u64,
    pub l3: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
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

#[derive(Debug, Clone)]
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

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            default_syscall_cycles: 500,
            io_cycles_per_byte: 20,
            l1_latency: 3,
            l2_latency: 11,
            l3_latency: 50,
            memory_latency: 125,
            store_latency: 1,
            syscall_cycles: HashMap::new(),
        }
    }
}

pub fn load_config(path: &std::path::Path) -> anyhow::Result<SimConfigRaw> {
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
pub struct HecateMemory {
    pub bytes: HashMap<u32, u8>,
    stats: Rc<RefCell<RunStats>>,
    caches: CacheHierarchy,
}

impl HecateMemory {
    pub fn new(
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

    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) {
        for (offset, byte) in data.iter().enumerate() {
            self.bytes.insert(addr.wrapping_add(offset as u32), *byte);
        }
    }

    pub fn zero_fill(&mut self, addr: u32, len: u32) {
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
pub struct HecateClock {
    stats: Rc<RefCell<RunStats>>,
    max_instructions: Option<u64>,
}

impl HecateClock {
    pub fn new(stats: Rc<RefCell<RunStats>>, max_instructions: Option<u64>) -> Self {
        Self {
            stats,
            max_instructions,
        }
    }

    pub fn set_max_instructions(&mut self, max_instructions: Option<u64>) {
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

fn read_u32_field(packed_field: *const u32) -> u32 {
    u32::from_le(unsafe { packed_field.read_unaligned() })
}

pub fn load_elf_bytes(label: &str, data: &[u8], memory: &mut HecateMemory) -> anyhow::Result<u32> {
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

    Ok(read_u32_field(std::ptr::addr_of!(elf.header.entry)))
}

pub fn load_elf(path: &PathBuf, memory: &mut HecateMemory) -> anyhow::Result<u32> {
    let data =
        std::fs::read(path).with_context(|| format!("Failed to read ELF: {}", path.display()))?;
    load_elf_bytes(&path.display().to_string(), &data, memory)
}

pub fn syscall_cycles_for(
    code: u32,
    default_cycles: u64,
    syscall_cycles: &HashMap<u32, u64>,
) -> u64 {
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

fn handle_syscall_impl(
    state: &mut CpuState,
    memory: &HecateMemory,
    config: &SimConfig,
    code: u32,
    emit_io: bool,
) -> (bool, u64, u64) {
    match code {
        64 => {
            let fd = state.x[10];
            let buf_addr = state.x[11];
            let len = state.x[12];

            if fd == 1 || fd == 2 {
                let bytes = read_program_bytes(memory, buf_addr, len);
                if emit_io {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
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

pub fn handle_syscall(
    state: &mut CpuState,
    memory: &HecateMemory,
    config: &SimConfig,
    code: u32,
) -> (bool, u64, u64) {
    handle_syscall_impl(state, memory, config, code, true)
}

pub fn handle_syscall_silent(
    state: &mut CpuState,
    memory: &HecateMemory,
    config: &SimConfig,
    code: u32,
) -> (bool, u64, u64) {
    handle_syscall_impl(state, memory, config, code, false)
}

fn xreg(reg: u32) -> String {
    format!("x{}", reg)
}

fn sign_extend(value: u32, width: u32) -> i32 {
    let shift = 32 - width;
    ((value << shift) as i32) >> shift
}

pub fn decode_rvc_mnemonic(inst: u16) -> String {
    let funct3 = (inst >> 13) & 0x7;
    let quadrant = inst & 0x3;

    match (quadrant, funct3) {
        (0b01, 0b000) => "c.addi".to_string(),
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

pub fn decode_rv32_mnemonic(inst: u32, pc: u32) -> String {
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
