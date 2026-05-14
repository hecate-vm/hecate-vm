use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use anyhow::{Context, anyhow};
use rvsim::{CpuError, CpuState, Interp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{
    HecateClock, HecateMemory, RunStats, SimConfig, handle_syscall, load_elf, syscall_cycles_for,
};

const UI_HTML: &str = include_str!("assets/index.html");

#[derive(Debug, Deserialize)]
struct DapRequest {
    seq: Option<u64>,
    #[serde(rename = "type")]
    request_type: String,
    command: String,
    arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
struct DapResponse {
    seq: u64,
    #[serde(rename = "type")]
    response_type: &'static str,
    request_seq: u64,
    success: bool,
    command: String,
    message: Option<String>,
    body: Value,
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
            registers: self.state.x.to_vec(),
            stats,
        }
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

fn command_from_dap(req: &DapRequest) -> anyhow::Result<VmCommand> {
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
        other => return Err(anyhow!("unsupported DAP command: {other}")),
    };
    Ok(cmd)
}

fn to_dap_response(request_seq: u64, command: String, reply: VmReply) -> DapResponse {
    match reply {
        VmReply::Error { message } => DapResponse {
            seq: request_seq.saturating_add(1),
            response_type: "response",
            request_seq,
            success: false,
            command,
            message: Some(message),
            body: Value::Null,
        },
        other => DapResponse {
            seq: request_seq.saturating_add(1),
            response_type: "response",
            request_seq,
            success: true,
            command,
            message: None,
            body: serde_json::to_value(other).unwrap_or(Value::Null),
        },
    }
}

fn handle_dap_request(mut request: Request, tx: &Sender<Envelope>) {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        respond_json(
            request,
            400,
            &serde_json::json!({ "success": false, "message": "failed to read request body" }),
        );
        return;
    }

    let parsed: DapRequest = match serde_json::from_str(&body) {
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

    if parsed.request_type != "request" {
        respond_json(
            request,
            400,
            &serde_json::json!({ "success": false, "message": "DAP type must be 'request'" }),
        );
        return;
    }

    let request_seq = parsed.seq.unwrap_or(0);
    let command = parsed.command.clone();
    let cmd = match command_from_dap(&parsed) {
        Ok(cmd) => cmd,
        Err(e) => {
            let rsp = DapResponse {
                seq: request_seq.saturating_add(1),
                response_type: "response",
                request_seq,
                success: false,
                command,
                message: Some(e.to_string()),
                body: Value::Null,
            };
            respond_json(
                request,
                200,
                &serde_json::to_value(rsp).unwrap_or(Value::Null),
            );
            return;
        }
    };

    let reply = match send_command(tx, cmd) {
        Ok(reply) => reply,
        Err(e) => VmReply::Error {
            message: e.to_string(),
        },
    };

    let rsp = to_dap_response(request_seq, parsed.command, reply);
    respond_json(
        request,
        200,
        &serde_json::to_value(rsp).unwrap_or(Value::Null),
    );
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
    let server = Server::http(&bind_addr)
        .map_err(|e| anyhow!("Failed to bind debug UI server at {bind_addr}: {e}"))?;

    println!("Debug UI listening on http://{bind_addr}");
    println!("Use /api/dap for DAP-style control messages.");

    for request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();

        match (method, url.as_str()) {
            (Method::Get, "/") => respond_html(request, 200, UI_HTML),
            (Method::Post, "/api/dap") => handle_dap_request(request, &tx),
            _ => respond_json(
                request,
                404,
                &serde_json::json!({
                    "success": false,
                    "message": "not found",
                    "hint": "use GET / or POST /api/dap"
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
