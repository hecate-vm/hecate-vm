use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use base64::Engine;
use hecate_vm::{
    HecateClock, HecateMemory, RunStats, SimConfig, decode_rv32_mnemonic, decode_rvc_mnemonic,
    handle_syscall_silent, load_elf_bytes, syscall_cycles_for,
};
use rvsim::{CpuError, CpuState, Interp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasm_bindgen::prelude::*;

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

#[derive(Debug, Clone)]
struct LoadedProgram {
    name: String,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
struct ControlRequest {
    #[serde(default, alias = "seq")]
    id: Option<u64>,
    command: String,
    arguments: Option<Value>,
}

#[derive(Serialize)]
struct ControlResponse {
    id: u64,
    seq: u64,
    success: bool,
    command: String,
    message: Option<String>,
    body: Value,
}

#[wasm_bindgen]
pub struct HecateVmWasm {
    shared_stats: Rc<RefCell<RunStats>>,
    memory: HecateMemory,
    state: CpuState,
    clock: HecateClock,
    config: SimConfig,

    loaded: Option<LoadedProgram>,
    loaded_path: Option<String>,
    entry: u32,
    halted: bool,
    running: bool,
    last_stop_reason: String,
}

#[wasm_bindgen]
impl HecateVmWasm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> HecateVmWasm {
        let shared_stats = Rc::new(RefCell::new(RunStats::default()));
        let config = SimConfig::default();
        let memory = HecateMemory::new(
            Rc::clone(&shared_stats),
            64,
            32 * 1024,
            256 * 1024,
            8 * 1024 * 1024,
            &config,
        );
        let state = CpuState::new(0);
        let clock = HecateClock::new(Rc::clone(&shared_stats), None);

        HecateVmWasm {
            shared_stats,
            memory,
            state,
            clock,
            config,
            loaded: None,
            loaded_path: None,
            entry: 0,
            halted: true,
            running: false,
            last_stop_reason: "No program loaded".to_string(),
        }
    }

    #[wasm_bindgen(js_name = loadBytes)]
    pub fn load_bytes(
        &mut self,
        bytes: js_sys::Uint8Array,
        name: String,
    ) -> Result<JsValue, JsValue> {
        let mut buf = vec![0_u8; bytes.length() as usize];
        bytes.copy_to(&mut buf);

        self.load_program_bytes(name.clone(), buf).map_err(js_err)?;

        to_js(&serde_json::json!({
            "entry": self.entry,
            "entry_hex": format!("0x{:08x}", self.entry),
            "loaded_path": name,
        }))
    }

    pub fn command(&mut self, command: String, args: JsValue) -> Result<JsValue, JsValue> {
        let args_value = if args.is_undefined() || args.is_null() {
            Value::Null
        } else {
            serde_wasm_bindgen::from_value(args).map_err(js_err)?
        };

        let body = self
            .execute(
                &command,
                if args_value.is_null() {
                    None
                } else {
                    Some(args_value)
                },
            )
            .map_err(js_err)?;
        to_js(&body)
    }

    pub fn control(&mut self, request: JsValue) -> Result<JsValue, JsValue> {
        let req: ControlRequest = serde_wasm_bindgen::from_value(request).map_err(js_err)?;
        let request_id = req.id.unwrap_or(0);
        let command = req.command.clone();

        let response = match self.execute(&req.command, req.arguments) {
            Ok(body) => ControlResponse {
                id: request_id,
                seq: request_id,
                success: true,
                command,
                message: None,
                body,
            },
            Err(error) => ControlResponse {
                id: request_id,
                seq: request_id,
                success: false,
                command,
                message: Some(error.to_string()),
                body: Value::Null,
            },
        };

        to_js(&response)
    }
}

#[wasm_bindgen(js_name = createHecateVm)]
pub fn create_hecate_vm() -> HecateVmWasm {
    HecateVmWasm::new()
}

impl HecateVmWasm {
    fn reset_memory_and_clock(&mut self) {
        self.memory = HecateMemory::new(
            Rc::clone(&self.shared_stats),
            64,
            32 * 1024,
            256 * 1024,
            8 * 1024 * 1024,
            &self.config,
        );
        self.clock = HecateClock::new(Rc::clone(&self.shared_stats), None);
    }

    fn clear_stats(&mut self) {
        *self.shared_stats.borrow_mut() = RunStats::default();
    }

    fn load_program_bytes(&mut self, name: String, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.clear_stats();
        self.reset_memory_and_clock();

        let entry = load_elf_bytes(&name, &bytes, &mut self.memory)?;

        self.entry = entry;
        self.state = CpuState::new(entry);
        self.loaded_path = Some(name.clone());
        self.loaded = Some(LoadedProgram { name, bytes });
        self.halted = false;
        self.running = false;
        self.last_stop_reason = "Program loaded".to_string();
        Ok(())
    }

    fn reset_to_beginning(&mut self) -> anyhow::Result<()> {
        let Some(loaded) = self.loaded.clone() else {
            return Err(anyhow!("No loaded program to reset"));
        };
        self.load_program_bytes(loaded.name, loaded.bytes)
    }

    fn run_until(&mut self, local_quota: Option<u64>) {
        if self.halted {
            return;
        }

        self.clock.set_max_instructions(local_quota);

        loop {
            let (error, _last_op) = {
                let mut interp = Interp::new(&mut self.state, &mut self.memory, &mut self.clock);
                interp.run()
            };

            if error != CpuError::Ecall {
                let reached_cap = local_quota
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
                handle_syscall_silent(&mut self.state, &self.memory, &self.config, syscall_code);

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

    fn snapshot(&mut self) -> DebugSnapshot {
        if self.running && !self.halted {
            self.continue_run_quantum(5_000);
        }
        let (instr_text, instr_hex, instr_size, instr_bytes) = self.current_instruction();
        let stats = self.shared_stats.borrow().clone();
        DebugSnapshot {
            running: self.running,
            halted: self.halted,
            pc: self.state.pc,
            pc_hex: format!("0x{:08x}", self.state.pc),
            entry: self.entry,
            entry_hex: format!("0x{:08x}", self.entry),
            loaded_path: self.loaded_path.clone(),
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

    fn execute(&mut self, command: &str, args: Option<Value>) -> anyhow::Result<Value> {
        match command {
            "initialize" => Ok(serde_json::json!({"kind": "ack"})),
            "examples" => Ok(serde_json::json!({"examples": []})),
            "launchBytes" | "upload" => {
                let (name, bytes) = parse_launch_bytes_args(args)?;
                self.load_program_bytes(name, bytes)?;
                Ok(serde_json::json!({
                    "entry": self.entry,
                    "entry_hex": format!("0x{:08x}", self.entry),
                }))
            }
            "continue" => {
                if self.halted {
                    Err(anyhow!("Program is halted or not loaded"))
                } else {
                    self.running = true;
                    self.last_stop_reason = "Running".to_string();
                    Ok(serde_json::json!({"kind": "ack"}))
                }
            }
            "pause" => {
                self.running = false;
                self.last_stop_reason = "Paused by client".to_string();
                Ok(serde_json::json!({"state": self.snapshot()}))
            }
            "next" | "step" => {
                let count = parse_u64_value(args.as_ref(), "count", 1)?;
                self.step(count)?;
                Ok(serde_json::json!({"state": self.snapshot()}))
            }
            "restart" | "reset" => {
                self.reset_to_beginning()?;
                Ok(serde_json::json!({"state": self.snapshot()}))
            }
            "readMemory" => {
                let addr = parse_u32_value(args.as_ref(), "addr", 0)?;
                let len = parse_u32_value(args.as_ref(), "len", 64)?;
                Ok(serde_json::json!({
                    "addr": addr,
                    "len": len,
                    "bytes": self.read_memory(addr, len),
                }))
            }
            "state" | "registers" => Ok(serde_json::json!({"state": self.snapshot()})),
            "disconnect" | "shutdown" => {
                self.running = false;
                Ok(serde_json::json!({"kind": "ack"}))
            }
            other => Err(anyhow!("unsupported control command: {other}")),
        }
    }
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(js_err)
}

fn js_err<E: std::fmt::Display>(error: E) -> JsValue {
    JsValue::from_str(&error.to_string())
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

fn parse_launch_bytes_args(v: Option<Value>) -> anyhow::Result<(String, Vec<u8>)> {
    let Some(args) = v else {
        return Err(anyhow!("missing arguments"));
    };
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing argument: name"))?
        .to_string();

    if let Some(bytes) = args.get("bytes") {
        let parsed: Vec<u8> = serde_json::from_value(bytes.clone())
            .map_err(|e| anyhow!("invalid bytes array: {e}"))?;
        return Ok((name, parsed));
    }

    if let Some(encoded) = args.get("bytesBase64").and_then(|v| v.as_str()) {
        let parsed = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| anyhow!("invalid bytesBase64 payload: {e}"))?;
        return Ok((name, parsed));
    }

    Err(anyhow!(
        "missing launch bytes payload (bytes or bytesBase64)"
    ))
}
