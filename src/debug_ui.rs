use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use anyhow::{Context, anyhow};
use rvsim::{CpuError, CpuState, Interp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tungstenite::{Message, accept};

use crate::{
    HecateClock, HecateMemory, RunStats, SimConfig, handle_syscall, load_elf, syscall_cycles_for,
};

const UI_HTML: &str = include_str!("assets/index.html");
const EXAMPLES_JSON: &str = include_str!("assets/examples.json");
const WASM_SHIM_JS: &str = include_str!("assets/wasm/hecate_vm_wasm.js");

#[derive(Debug, Deserialize)]
struct ControlRequest {
    #[serde(default, alias = "seq")]
    id: Option<u64>,
    #[serde(rename = "type")]
    request_type: Option<String>,
    command: String,
    arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ControlResponse {
    id: u64,
    seq: u64,
    success: bool,
    command: String,
    message: Option<String>,
    body: Value,
}

#[derive(Debug, Serialize, Clone)]
struct ExampleEntry {
    name: String,
    path: String,
}

impl ControlRequest {
    fn request_id(&self) -> u64 {
        self.id.unwrap_or(0)
    }
}

#[derive(Debug, Serialize)]
struct DebugSnapshot {
    running: bool,
    halted: bool,
    pc: u32,
    pc_hex: String,
    entry: u32,
    entry_hex: String,
    loaded_path: Option<String>,
    last_stop_reason: String,
    current_instruction: String,
    current_instruction_hex: Option<String>,
    current_instruction_size: Option<u32>,
    current_instruction_bytes: Option<String>,
    registers: Vec<u32>,
    stats: RunStats,
}

#[derive(Debug)]
enum VmCommand {
    Initialize,
    Launch { path: String },
    Continue,
    Pause,
    Next { count: u64 },
    Restart,
    ReadMemory { addr: u32, len: u32 },
    State,
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum VmReply {
    Ack,
    Loaded { entry: u32, entry_hex: String },
    State { state: DebugSnapshot },
    Memory { addr: u32, len: u32, bytes: Vec<u8> },
    Error { message: String },
}

struct Envelope {
    cmd: VmCommand,
    tx: Sender<VmReply>,
}

#[derive(Debug)]
struct DebugMachine {
    cache_line_size: u32,
    l1_size: u32,
    l2_size: u32,
    l3_size: u32,
    max_instructions: Option<u64>,
    config: SimConfig,

    shared_stats: Rc<RefCell<RunStats>>,
    memory: HecateMemory,
    state: CpuState,
    clock: HecateClock,

    loaded_path: Option<PathBuf>,
    entry: u32,
    halted: bool,
    running: bool,
    last_stop_reason: String,
}

impl DebugMachine {
    fn new(
        initial_path: Option<PathBuf>,
        cache_line_size: u32,
        l1_size: u32,
        l2_size: u32,
        l3_size: u32,
        max_instructions: Option<u64>,
        config: SimConfig,
    ) -> anyhow::Result<Self> {
        let shared_stats = Rc::new(RefCell::new(RunStats::default()));
        let memory = HecateMemory::new(
            Rc::clone(&shared_stats),
            cache_line_size,
            l1_size,
            l2_size,
            l3_size,
            &config,
        );

        let mut machine = Self {
            cache_line_size,
            l1_size,
            l2_size,
            l3_size,
            max_instructions,
            config,
            shared_stats: Rc::clone(&shared_stats),
            memory,
            state: CpuState::new(0),
            clock: HecateClock::new(Rc::clone(&shared_stats), max_instructions),
            loaded_path: None,
            entry: 0,
            halted: true,
            running: false,
            last_stop_reason: "No program loaded".to_string(),
        };

        if let Some(path) = initial_path {
            machine.load_program(path)?;
        }

        Ok(machine)
    }

    fn reset_memory_and_clock(&mut self) {
        self.memory = HecateMemory::new(
            Rc::clone(&self.shared_stats),
            self.cache_line_size,
            self.l1_size,
            self.l2_size,
            self.l3_size,
            &self.config,
        );
        self.clock = HecateClock::new(Rc::clone(&self.shared_stats), self.max_instructions);
    }

    fn clear_stats(&mut self) {
        *self.shared_stats.borrow_mut() = RunStats::default();
    }

    fn load_program(&mut self, path: PathBuf) -> anyhow::Result<()> {
        self.clear_stats();
        self.reset_memory_and_clock();

        let entry = load_elf(&path, &mut self.memory)
            .with_context(|| format!("Failed to load program: {}", path.display()))?;
        self.entry = entry;
        self.state = CpuState::new(entry);
        self.loaded_path = Some(path);
        self.halted = false;
        self.running = false;
        self.last_stop_reason = "Program loaded".to_string();
        Ok(())
    }

    fn reset_to_beginning(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.loaded_path.clone() else {
            return Err(anyhow!("No loaded program to reset"));
        };
        self.load_program(path)
    }

    fn effective_quota(&self, local_quota: Option<u64>) -> Option<u64> {
        match (self.max_instructions, local_quota) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn run_until(&mut self, local_quota: Option<u64>) {
        if self.halted {
            return;
        }

        let cap = self.effective_quota(local_quota);
        self.clock.set_max_instructions(cap);

        loop {
            let (error, _last_op) = {
                let mut interp = Interp::new(&mut self.state, &mut self.memory, &mut self.clock);
                interp.run()
            };

            if error != CpuError::Ecall {
                let reached_cap = cap
                    .map(|limit| self.shared_stats.borrow().instret >= limit)
                    .unwrap_or(false);

                if reached_cap {
                    self.last_stop_reason = "QuotaReached".to_string();
                } else {
                    self.halted = true;
                    self.running = false;
                    self.last_stop_reason = format!("{:?}", error);
                }
                break;
            }

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
                *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) += syscall_cycles;
            }

            let (should_continue, extra_cycles, io_bytes_written) =
                handle_syscall(&mut self.state, &self.memory, &self.config, syscall_code);

            if extra_cycles != 0 || io_bytes_written != 0 {
                let mut stats = self.shared_stats.borrow_mut();
                stats.io_cycles = stats.io_cycles.wrapping_add(extra_cycles);
                stats.io_bytes_written = stats.io_bytes_written.wrapping_add(io_bytes_written);
                stats.syscall_cycles = stats.syscall_cycles.wrapping_add(extra_cycles);
                stats.cycles = stats.cycles.wrapping_add(extra_cycles);
                *stats.syscall_cycle_totals.entry(syscall_code).or_insert(0) += extra_cycles;
            }

            if !should_continue {
                self.halted = true;
                self.running = false;
                self.last_stop_reason = "Program exited".to_string();
                break;
            }

            self.state.pc = self.state.pc.wrapping_add(4);
        }
    }

    fn step(&mut self, count: u64) -> anyhow::Result<()> {
        if self.halted {
            return Err(anyhow!("Program is halted or not loaded"));
        }
        let current = self.shared_stats.borrow().instret;
        let target = current.saturating_add(count.max(1));
        self.running = false;
        self.run_until(Some(target));
        if !self.halted {
            self.last_stop_reason = "Step complete".to_string();
        }
        Ok(())
    }

    fn continue_run_quantum(&mut self, quantum: u64) {
        if self.halted {
            self.running = false;
            return;
        }
        let current = self.shared_stats.borrow().instret;
        let target = current.saturating_add(quantum.max(1));
        self.run_until(Some(target));
        if !self.halted && self.running {
            self.last_stop_reason = "Running".to_string();
        }
    }

    fn read_memory(&self, addr: u32, len: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            out.push(
                self.memory
                    .bytes
                    .get(&addr.wrapping_add(i))
                    .copied()
                    .unwrap_or(0),
            );
        }
        out
    }

    fn snapshot(&self) -> DebugSnapshot {
        let (instr_text, instr_hex, instr_size, instr_bytes) = self.current_instruction();
        let stats = self.shared_stats.borrow().clone();
        DebugSnapshot {
            running: self.running,
            halted: self.halted,
            pc: self.state.pc,
            pc_hex: format!("0x{:08x}", self.state.pc),
            entry: self.entry,
            entry_hex: format!("0x{:08x}", self.entry),
            loaded_path: self.loaded_path.as_ref().map(|p| p.display().to_string()),
            last_stop_reason: self.last_stop_reason.clone(),
            current_instruction: instr_text,
            current_instruction_hex: instr_hex,
            current_instruction_size: instr_size,
            current_instruction_bytes: instr_bytes,
            registers: self.state.x.to_vec(),
            stats,
        }
    }

    fn current_instruction(&self) -> (String, Option<String>, Option<u32>, Option<String>) {
        let pc = self.state.pc;

        let b0 = self.memory.bytes.get(&pc).copied();
        let b1 = self.memory.bytes.get(&pc.wrapping_add(1)).copied();
        let (Some(b0), Some(b1)) = (b0, b1) else {
            return (
                "Unavailable (memory unmapped at PC)".to_string(),
                None,
                None,
                None,
            );
        };

        let halfword = (b0 as u16) | ((b1 as u16) << 8);
        if (halfword & 0b11) != 0b11 {
            let raw = halfword as u32;
            let bytes = format!("{:02x} {:02x}", b0, b1);
            let mnemonic = decode_rvc_mnemonic(halfword);
            return (
                format!("{} ({})", mnemonic, bytes),
                Some(format!("0x{:04x}", raw as u16)),
                Some(2),
                Some(bytes),
            );
        }

        let b2 = self.memory.bytes.get(&pc.wrapping_add(2)).copied();
        let b3 = self.memory.bytes.get(&pc.wrapping_add(3)).copied();
        let (Some(b2), Some(b3)) = (b2, b3) else {
            let bytes = format!("{:02x} {:02x}", b0, b1);
            return (
                "Unavailable (incomplete 32-bit instruction at PC)".to_string(),
                None,
                None,
                Some(bytes),
            );
        };

        let raw = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16) | ((b3 as u32) << 24);
        let bytes = format!("{:02x} {:02x} {:02x} {:02x}", b0, b1, b2, b3);
        let mnemonic = decode_rv32_mnemonic(raw, pc);
        (
            format!("{} ({})", mnemonic, bytes),
            Some(format!("0x{:08x}", raw)),
            Some(4),
            Some(bytes),
        )
    }
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

fn parse_u32_value(v: Option<&Value>, key: &str, default: u32) -> anyhow::Result<u32> {
    let Some(args) = v else {
        return Ok(default);
    };
    let Some(raw) = args.get(key) else {
        return Ok(default);
    };

    if let Some(n) = raw.as_u64() {
        return Ok(n as u32);
    }
    if let Some(s) = raw.as_str() {
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return u32::from_str_radix(hex, 16)
                .map_err(|e| anyhow!("invalid hex value for {key}: {e}"));
        }
        return s
            .parse::<u32>()
            .map_err(|e| anyhow!("invalid decimal value for {key}: {e}"));
    }

    Err(anyhow!("invalid value for {key}"))
}

fn parse_u64_value(v: Option<&Value>, key: &str, default: u64) -> anyhow::Result<u64> {
    let Some(args) = v else {
        return Ok(default);
    };
    let Some(raw) = args.get(key) else {
        return Ok(default);
    };

    if let Some(n) = raw.as_u64() {
        return Ok(n);
    }
    if let Some(s) = raw.as_str() {
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return u64::from_str_radix(hex, 16)
                .map_err(|e| anyhow!("invalid hex value for {key}: {e}"));
        }
        return s
            .parse::<u64>()
            .map_err(|e| anyhow!("invalid decimal value for {key}: {e}"));
    }

    Err(anyhow!("invalid value for {key}"))
}

fn parse_string_value(v: Option<&Value>, key: &str) -> anyhow::Result<String> {
    let Some(args) = v else {
        return Err(anyhow!("missing arguments"));
    };
    let Some(raw) = args.get(key) else {
        return Err(anyhow!("missing argument: {key}"));
    };
    let Some(s) = raw.as_str() else {
        return Err(anyhow!("argument {key} must be a string"));
    };
    Ok(s.to_string())
}

fn worker_loop(
    rx: Receiver<Envelope>,
    initial_path: Option<PathBuf>,
    cache_line_size: u32,
    l1_size: u32,
    l2_size: u32,
    l3_size: u32,
    max_instructions: Option<u64>,
    config: SimConfig,
) -> anyhow::Result<()> {
    let mut vm = DebugMachine::new(
        initial_path,
        cache_line_size,
        l1_size,
        l2_size,
        l3_size,
        max_instructions,
        config,
    )?;

    loop {
        if vm.running && !vm.halted {
            vm.continue_run_quantum(5_000);
            while let Ok(env) = rx.try_recv() {
                if handle_command(&mut vm, env)? {
                    return Ok(());
                }
            }
            continue;
        }

        let env = match rx.recv() {
            Ok(env) => env,
            Err(_) => break,
        };
        if handle_command(&mut vm, env)? {
            break;
        }
    }

    Ok(())
}

fn handle_command(vm: &mut DebugMachine, env: Envelope) -> anyhow::Result<bool> {
    let reply = match env.cmd {
        VmCommand::Initialize => VmReply::Ack,
        VmCommand::Launch { path } => {
            let p = PathBuf::from(path);
            match vm.load_program(p) {
                Ok(()) => VmReply::Loaded {
                    entry: vm.entry,
                    entry_hex: format!("0x{:08x}", vm.entry),
                },
                Err(e) => VmReply::Error {
                    message: e.to_string(),
                },
            }
        }
        VmCommand::Continue => {
            if vm.halted {
                VmReply::Error {
                    message: "Program is halted or not loaded".to_string(),
                }
            } else {
                vm.running = true;
                vm.last_stop_reason = "Running".to_string();
                VmReply::Ack
            }
        }
        VmCommand::Pause => {
            vm.running = false;
            vm.last_stop_reason = "Paused by client".to_string();
            VmReply::State {
                state: vm.snapshot(),
            }
        }
        VmCommand::Next { count } => match vm.step(count) {
            Ok(()) => VmReply::State {
                state: vm.snapshot(),
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Restart => match vm.reset_to_beginning() {
            Ok(()) => VmReply::State {
                state: vm.snapshot(),
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::ReadMemory { addr, len } => VmReply::Memory {
            addr,
            len,
            bytes: vm.read_memory(addr, len),
        },
        VmCommand::State => VmReply::State {
            state: vm.snapshot(),
        },
        VmCommand::Shutdown => {
            let _ = env.tx.send(VmReply::Ack);
            return Ok(true);
        }
    };

    let _ = env.tx.send(reply);
    Ok(false)
}

fn send_command(tx: &Sender<Envelope>, cmd: VmCommand) -> anyhow::Result<VmReply> {
    let (resp_tx, resp_rx) = channel::<VmReply>();
    tx.send(Envelope { cmd, tx: resp_tx })
        .context("failed to send command to VM thread")?;
    resp_rx
        .recv()
        .context("failed to receive response from VM thread")
}

fn process_control_request(parsed: ControlRequest, tx: &Sender<Envelope>) -> ControlResponse {
    let request_id = parsed.request_id();
    let command = parsed.command.clone();

    if let Some(raw_type) = parsed.request_type.as_deref() {
        if raw_type != "request" {
            return ControlResponse {
                id: request_id,
                seq: request_id,
                success: false,
                command,
                message: Some("if provided, type must be 'request'".to_string()),
                body: Value::Null,
            };
        }
    }

    let cmd = match command_from_request(&parsed) {
        Ok(cmd) => cmd,
        Err(e) => {
            return ControlResponse {
                id: request_id,
                seq: request_id,
                success: false,
                command,
                message: Some(e.to_string()),
                body: Value::Null,
            };
        }
    };

    let reply = match send_command(tx, cmd) {
        Ok(reply) => reply,
        Err(e) => VmReply::Error {
            message: e.to_string(),
        },
    };

    to_control_response(request_id, parsed.command, reply)
}

fn discover_examples() -> Vec<ExampleEntry> {
    let mut out = Vec::<ExampleEntry>::new();
    let mut seen = HashSet::<String>::new();

    let normalize = |path: &str| {
        let p = path.trim_start_matches("./").replace("\\", "/");
        format!("./{}", p)
    };

    let mut push_if_exists = |name: &str, path: &str| {
        let normalized = normalize(path);
        if PathBuf::from(&normalized).exists() && seen.insert(normalized.clone()) {
            out.push(ExampleEntry {
                name: name.to_string(),
                path: normalized,
            });
        }
    };

    push_if_exists(
        "hello_world",
        "./target/hecate-rv32-build/examples-build/hello_world/hello_world.elf",
    );
    push_if_exists(
        "linked_list",
        "./target/hecate-rv32-build/examples-build/linked_list/linked_list.elf",
    );
    push_if_exists(
        "vector_contiguous",
        "./target/hecate-rv32-build/examples-build/vector_contiguous/vector_contiguous.elf",
    );
    push_if_exists(
        "rust_hello",
        "./target/hecate-rv32-build/examples-build/rust_hello/rust_hello.elf",
    );
    push_if_exists(
        "rust_hello (cargo target)",
        "./examples/rust_hello/target/riscv32im-hecate-none-elf/release/rust_hello.elf",
    );

    if let Ok(dir) = fs::read_dir("./target/hecate-rv32-build/examples-build") {
        for entry in dir.flatten() {
            let sub = entry.path();
            if !sub.is_dir() {
                continue;
            }
            let name = sub
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("example")
                .to_string();
            let elf = sub.join(format!("{name}.elf"));
            if elf.exists() {
                let path = normalize(&elf.display().to_string());
                if seen.insert(path.clone()) {
                    out.push(ExampleEntry { name, path });
                }
            }
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn content_type_header(value: &str) -> Option<Header> {
    Header::from_bytes(&b"Content-Type"[..], value.as_bytes()).ok()
}

fn respond_json(request: Request, status: u16, value: &Value) {
    let body = value.to_string();
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Some(header) = content_type_header("application/json; charset=utf-8") {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

fn respond_html(request: Request, status: u16, body: &str) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Some(header) = content_type_header("text/html; charset=utf-8") {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

fn respond_text(request: Request, status: u16, body: &str, content_type: &str) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Some(header) = content_type_header(content_type) {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

fn handle_examples_request(request: Request) {
    let examples = discover_examples();
    respond_json(request, 200, &serde_json::json!({ "examples": examples }));
}

fn handle_examples_manifest_request(request: Request) {
    let value: Value = serde_json::from_str(EXAMPLES_JSON)
        .unwrap_or_else(|_| serde_json::json!({ "examples": [] }));
    respond_json(request, 200, &value);
}

fn command_from_request(req: &ControlRequest) -> anyhow::Result<VmCommand> {
    let args = req.arguments.as_ref();
    let cmd = match req.command.as_str() {
        "initialize" => VmCommand::Initialize,
        "launch" | "load" => VmCommand::Launch {
            path: parse_string_value(args, "path")?,
        },
        "continue" => VmCommand::Continue,
        "pause" => VmCommand::Pause,
        "next" | "step" => VmCommand::Next {
            count: parse_u64_value(args, "count", 1)?,
        },
        "restart" | "reset" => VmCommand::Restart,
        "readMemory" => VmCommand::ReadMemory {
            addr: parse_u32_value(args, "addr", 0)?,
            len: parse_u32_value(args, "len", 64)?,
        },
        "state" | "registers" => VmCommand::State,
        "disconnect" | "shutdown" => VmCommand::Shutdown,
        other => return Err(anyhow!("unsupported control command: {other}")),
    };
    Ok(cmd)
}

fn to_control_response(request_id: u64, command: String, reply: VmReply) -> ControlResponse {
    match reply {
        VmReply::Error { message } => ControlResponse {
            id: request_id,
            seq: request_id,
            success: false,
            command,
            message: Some(message),
            body: Value::Null,
        },
        other => ControlResponse {
            id: request_id,
            seq: request_id,
            success: true,
            command,
            message: None,
            body: serde_json::to_value(other).unwrap_or(Value::Null),
        },
    }
}

fn handle_control_request(mut request: Request, tx: &Sender<Envelope>) {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        respond_json(
            request,
            400,
            &serde_json::json!({ "success": false, "message": "failed to read request body" }),
        );
        return;
    }

    let parsed: ControlRequest = match serde_json::from_str(&body) {
        Ok(req) => req,
        Err(e) => {
            respond_json(
                request,
                400,
                &serde_json::json!({ "success": false, "message": format!("invalid JSON: {e}") }),
            );
            return;
        }
    };

    let rsp = process_control_request(parsed, tx);
    respond_json(
        request,
        200,
        &serde_json::to_value(rsp).unwrap_or(Value::Null),
    );
}

fn handle_ws_client(stream: TcpStream, tx: Sender<Envelope>) -> anyhow::Result<()> {
    let mut socket = accept(stream).context("websocket handshake failed")?;

    loop {
        let message = match socket.read() {
            Ok(msg) => msg,
            Err(_) => break,
        };

        if !message.is_text() {
            continue;
        }

        let text = message.into_text().unwrap_or_default();
        let parsed: ControlRequest = match serde_json::from_str(&text) {
            Ok(req) => req,
            Err(e) => {
                let bad = serde_json::json!({
                    "id": 0,
                    "seq": 0,
                    "success": false,
                    "command": "unknown",
                    "message": format!("invalid JSON: {e}"),
                    "body": null
                });
                let _ = socket.send(Message::Text(bad.to_string().into()));
                continue;
            }
        };

        let rsp = process_control_request(parsed, &tx);
        let payload = serde_json::to_string(&rsp).unwrap_or_else(|_| {
            serde_json::json!({
                "id": 0,
                "seq": 0,
                "success": false,
                "command": "unknown",
                "message": "serialization failed",
                "body": null
            })
            .to_string()
        });
        socket.send(Message::Text(payload.into()))?;
    }

    Ok(())
}

fn run_ws_server(bind_addr: String, tx: Sender<Envelope>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(&bind_addr)
        .map_err(|e| anyhow!("Failed to bind websocket server at {bind_addr}: {e}"))?;
    println!("Control websocket listening on ws://{bind_addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let tx_clone = tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_ws_client(stream, tx_clone) {
                        eprintln!("Websocket client error: {err}");
                    }
                });
            }
            Err(err) => {
                eprintln!("Websocket accept error: {err}");
            }
        }
    }

    Ok(())
}

pub fn serve(
    initial_path: Option<PathBuf>,
    cache_line_size: u32,
    l1_size: u32,
    l2_size: u32,
    l3_size: u32,
    max_instructions: Option<u64>,
    config: SimConfig,
    port: u16,
) -> anyhow::Result<()> {
    let (tx, rx) = channel::<Envelope>();

    let worker = thread::spawn(move || {
        worker_loop(
            rx,
            initial_path,
            cache_line_size,
            l1_size,
            l2_size,
            l3_size,
            max_instructions,
            config,
        )
    });

    let bind_addr = format!("127.0.0.1:{port}");
    let ws_port = port.saturating_add(1);
    let ws_bind_addr = format!("127.0.0.1:{ws_port}");

    let server = Server::http(&bind_addr)
        .map_err(|e| anyhow!("Failed to bind debug UI server at {bind_addr}: {e}"))?;

    let ws_tx = tx.clone();
    thread::spawn(move || {
        if let Err(err) = run_ws_server(ws_bind_addr, ws_tx) {
            eprintln!("Websocket server failed: {err}");
        }
    });

    println!("Debug UI listening on http://{bind_addr}");
    println!("Use /api/v1/control for control messages.");
    println!("Use ws://127.0.0.1:{ws_port} for websocket control.");

    for request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();

        match (method, url.as_str()) {
            (Method::Get, "/") => respond_html(request, 200, UI_HTML),
            (Method::Get, "/api/v1/examples") => handle_examples_request(request),
            (Method::Get, "/assets/examples.json") => handle_examples_manifest_request(request),
            (Method::Get, "/assets/wasm/hecate_vm_wasm.js") => {
                respond_text(request, 200, WASM_SHIM_JS, "text/javascript; charset=utf-8")
            }
            (Method::Post, "/api/v1/control") => handle_control_request(request, &tx),
            _ => respond_json(
                request,
                404,
                &serde_json::json!({
                    "success": false,
                    "message": "not found",
                    "hint": "use GET /, GET /api/v1/examples, GET /assets/examples.json, GET /assets/wasm/hecate_vm_wasm.js, or POST /api/v1/control"
                }),
            ),
        }
    }

    let _ = send_command(&tx, VmCommand::Shutdown);
    worker
        .join()
        .map_err(|_| anyhow!("VM worker thread panicked"))??;

    Ok(())
}
