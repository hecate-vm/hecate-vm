use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use anyhow::{Context, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tungstenite::{Message, accept};

use crate::vm::{
    HecateVm, IoMode, MemoryReadResult, ResetMemoryPolicy, SimConfig, VmDump, VmRuntimeOptions,
};

const UI_HTML: &str = include_str!("assets/index.html");
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

impl ControlRequest {
    fn request_id(&self) -> u64 {
        self.id.unwrap_or(0)
    }
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
}

#[derive(Debug)]
enum VmCommand {
    Initialize,
    Unload,
    Examples,
    LoadBlob { name: String, bytes: Vec<u8> },
    LoadExample { name: String },
    Run,
    Pause,
    Step,
    StepCount { count: u64 },
    StepOver,
    StepOut,
    Reset { policy: ResetMemoryPolicy },
    Read { addr: u32, len: u32 },
    Write { addr: u32, bytes: Vec<u8> },
    State,
    Dump,
    Restore { dump: VmDump },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum VmReply {
    Ack,
    Loaded {
        entry: u32,
        entry_hex: String,
        state: Value,
    },
    Examples {
        examples: Vec<ExampleEntry>,
    },
    State {
        state: Value,
    },
    Memory {
        result: MemoryReadResult,
    },
    Dump {
        dump: VmDump,
    },
    Error {
        message: String,
    },
}

struct Envelope {
    cmd: VmCommand,
    tx: Sender<VmReply>,
}

fn builtin_examples() -> Vec<ExampleEntry> {
    let mut examples = crate::bundled_examples::EXAMPLES
        .iter()
        .map(|example| ExampleEntry {
            name: example.name.to_string(),
        })
        .collect::<Vec<_>>();
    examples.sort_by(|a, b| a.name.cmp(&b.name));
    examples
}

fn builtin_example_by_name(name: &str) -> Option<&'static [u8]> {
    crate::bundled_examples::EXAMPLES
        .iter()
        .find(|example| example.name == name)
        .map(|example| example.bytes)
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

fn parse_base64_bytes(v: Option<&Value>, key: &str) -> anyhow::Result<Vec<u8>> {
    let encoded = parse_string_value(v, key)?;
    BASE64_STANDARD
        .decode(encoded)
        .map_err(|e| anyhow!("invalid base64 value for {key}: {e}"))
}

fn parse_write_bytes(v: Option<&Value>) -> anyhow::Result<Vec<u8>> {
    let Some(args) = v else {
        return Err(anyhow!("missing arguments"));
    };

    if let Some(value) = args.get("bytesBase64") {
        let Some(encoded) = value.as_str() else {
            return Err(anyhow!("argument bytesBase64 must be a string"));
        };
        return BASE64_STANDARD
            .decode(encoded)
            .map_err(|e| anyhow!("invalid base64 value for bytesBase64: {e}"));
    }

    let Some(value) = args.get("bytes") else {
        return Err(anyhow!("missing argument: bytes or bytesBase64"));
    };
    let Some(items) = value.as_array() else {
        return Err(anyhow!("argument bytes must be an array of integers"));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(byte) = item.as_u64() else {
            return Err(anyhow!("argument bytes must be an array of integers"));
        };
        if byte > u8::MAX as u64 {
            return Err(anyhow!("byte value {byte} is outside the range 0..=255"));
        }
        out.push(byte as u8);
    }
    Ok(out)
}

fn parse_reset_policy(v: Option<&Value>) -> anyhow::Result<ResetMemoryPolicy> {
    let Some(args) = v else {
        return Ok(ResetMemoryPolicy::Ignore);
    };
    let Some(raw) = args.get("policy") else {
        return Ok(ResetMemoryPolicy::Ignore);
    };
    let Some(value) = raw.as_str() else {
        return Err(anyhow!("argument policy must be a string"));
    };
    match value {
        "ignore" => Ok(ResetMemoryPolicy::Ignore),
        "zero" => Ok(ResetMemoryPolicy::Zero),
        "random" | "randomize" => Ok(ResetMemoryPolicy::Randomize),
        _ => Err(anyhow!("unsupported reset policy: {value}")),
    }
}

fn parse_dump_value(v: Option<&Value>) -> anyhow::Result<VmDump> {
    let Some(args) = v else {
        return Err(anyhow!("missing arguments"));
    };
    let Some(raw) = args.get("dump") else {
        return Err(anyhow!("missing argument: dump"));
    };
    serde_json::from_value(raw.clone()).map_err(|e| anyhow!("invalid dump payload: {e}"))
}

fn load_builtin_example(vm: &mut HecateVm, name: String) -> anyhow::Result<Value> {
    let Some(bytes) = builtin_example_by_name(&name) else {
        return Err(anyhow!("Unknown example: {name}"));
    };
    let state = vm.load(name, bytes)?;
    serde_json::to_value(state).map_err(|e| anyhow!("failed to serialize VM state: {e}"))
}

fn state_value(vm: &HecateVm) -> VmReply {
    match serde_json::to_value(vm.state()) {
        Ok(state) => VmReply::State { state },
        Err(e) => VmReply::Error {
            message: format!("failed to serialize VM state: {e}"),
        },
    }
}

fn state_reply_from_snapshot(state: crate::vm::VmState) -> anyhow::Result<VmReply> {
    Ok(VmReply::State {
        state: serde_json::to_value(state)
            .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
    })
}

fn worker_loop(
    rx: Receiver<Envelope>,
    initial_path: Option<PathBuf>,
    options: VmRuntimeOptions,
    config: SimConfig,
) -> anyhow::Result<()> {
    let mut vm = HecateVm::new(options, config, IoMode::Buffer);
    if let Some(path) = initial_path {
        vm.load_file(&path)?;
    }

    loop {
        if vm.is_running() && !vm.is_halted() {
            vm.tick_running(5_000);
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

fn handle_command(vm: &mut HecateVm, env: Envelope) -> anyhow::Result<bool> {
    let reply = match env.cmd {
        VmCommand::Initialize => VmReply::Ack,
        VmCommand::Unload => {
            *vm = HecateVm::new(vm.options().clone(), vm.config().clone(), vm.io_mode());
            VmReply::Ack
        }
        VmCommand::Examples => VmReply::Examples {
            examples: builtin_examples(),
        },
        VmCommand::LoadBlob { name, bytes } => match vm.load(name, &bytes) {
            Ok(state) => VmReply::Loaded {
                entry: state.entry_point,
                entry_hex: format!("0x{:08x}", state.entry_point),
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::LoadExample { name } => match load_builtin_example(vm, name) {
            Ok(state) => VmReply::Loaded {
                entry: vm.entry_point(),
                entry_hex: format!("0x{:08x}", vm.entry_point()),
                state,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Run => match vm.run() {
            Ok(()) => VmReply::Ack,
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Pause => state_reply_from_snapshot(vm.pause())?,
        VmCommand::Step => match vm.step() {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::StepCount { count } => match vm.step_count(count) {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::StepOver => match vm.step_over() {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::StepOut => match vm.step_out() {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Reset { policy } => match vm.reset(policy) {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Read { addr, len } => match vm.read(addr, len) {
            Ok(result) => VmReply::Memory { result },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::Write { addr, bytes } => match vm.write(addr, &bytes) {
            Ok(()) => state_value(vm),
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
        },
        VmCommand::State => state_value(vm),
        VmCommand::Dump => VmReply::Dump { dump: vm.dump() },
        VmCommand::Restore { dump } => match vm.restore(dump) {
            Ok(state) => VmReply::State {
                state: serde_json::to_value(state)
                    .map_err(|e| anyhow!("failed to serialize VM state: {e}"))?,
            },
            Err(e) => VmReply::Error {
                message: e.to_string(),
            },
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

fn command_from_request(req: &ControlRequest) -> anyhow::Result<VmCommand> {
    let args = req.arguments.as_ref();
    let cmd = match req.command.as_str() {
        "initialize" => VmCommand::Initialize,
        "unload" => VmCommand::Unload,
        "examples" => VmCommand::Examples,
        "load" => {
            if args.and_then(|value| value.get("bytesBase64")).is_some() {
                VmCommand::LoadBlob {
                    name: parse_string_value(args, "name")?,
                    bytes: parse_base64_bytes(args, "bytesBase64")?,
                }
            } else {
                VmCommand::LoadExample {
                    name: parse_string_value(args, "name")?,
                }
            }
        }
        "run" => VmCommand::Run,
        "pause" => VmCommand::Pause,
        "step" => {
            let count = parse_u64_value(args, "count", 1)?;
            if count <= 1 {
                VmCommand::Step
            } else {
                VmCommand::StepCount { count }
            }
        }
        "step_over" => VmCommand::StepOver,
        "step_out" => VmCommand::StepOut,
        "reset" => VmCommand::Reset {
            policy: parse_reset_policy(args)?,
        },
        "read" => VmCommand::Read {
            addr: parse_u32_value(args, "addr", 0)?,
            len: parse_u32_value(args, "len", 64)?,
        },
        "write" => VmCommand::Write {
            addr: parse_u32_value(args, "addr", 0)?,
            bytes: parse_write_bytes(args)?,
        },
        "state" => VmCommand::State,
        "dump" => VmCommand::Dump,
        "restore" => VmCommand::Restore {
            dump: parse_dump_value(args)?,
        },
        "shutdown" => VmCommand::Shutdown,
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

#[cfg(feature = "http_control_api")]
fn handle_examples_request(request: Request) {
    respond_json(
        request,
        200,
        &serde_json::json!({ "examples": builtin_examples() }),
    );
}

#[cfg(feature = "http_control_api")]
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
    options: VmRuntimeOptions,
    config: SimConfig,
    port: u16,
) -> anyhow::Result<()> {
    let (tx, rx) = channel::<Envelope>();

    let worker = thread::spawn(move || worker_loop(rx, initial_path, options, config));

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

    println!("Remote Control: ws://127.0.0.1:{ws_port}");
    println!("Debug Console : http://{bind_addr}");

    for request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();

        match (method, url.as_str()) {
            (Method::Get, "/") => respond_html(request, 200, UI_HTML),
            (Method::Get, "/assets/wasm/hecate_vm_wasm.js") => {
                respond_text(request, 200, WASM_SHIM_JS, "text/javascript; charset=utf-8")
            }
            #[cfg(feature = "http_control_api")]
            (method, endpoint) if endpoint.starts_with("/api/v1/") => {
                match (method, endpoint.strip_prefix("/api/v1/").unwrap()) {
                    (Method::Get, "examples") => handle_examples_request(request),
                    (Method::Post, "control") => handle_control_request(request, &tx),
                    _ => respond_json(
                        request,
                        404,
                        &serde_json::json!({
                            "success": false,
                            "message": "not found",
                        }),
                    ),
                }
            }
            _ => respond_json(
                request,
                404,
                &serde_json::json!({
                    "success": false,
                    "message": "not found",
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
