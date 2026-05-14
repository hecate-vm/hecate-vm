use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::bundled_examples;
use crate::vm::{
    HecateVm, IoMode, MemoryReadResult, ResetMemoryPolicy, SimConfigRaw, VmDump, VmRuntimeOptions,
};

const DEFAULT_CONFIG: &str = include_str!("default.toml");

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

#[wasm_bindgen]
pub struct HecateVmWasm {
    vm: HecateVm,
}

fn default_vm() -> Result<HecateVm, JsValue> {
    let default_raw: SimConfigRaw = toml::from_str(DEFAULT_CONFIG)
        .map_err(|e| JsValue::from_str(&format!("failed to parse built-in config: {e}")))?;
    let config = default_raw
        .resolve()
        .map_err(|e| JsValue::from_str(&format!("failed to resolve built-in config: {e}")))?;

    Ok(HecateVm::new(
        VmRuntimeOptions {
            cache_line_size: 64,
            l1_size: 32 * 1024,
            l2_size: 256 * 1024,
            l3_size: 8 * 1024 * 1024,
            max_instructions: None,
        },
        config,
        IoMode::Stdout,
    ))
}

fn to_json_value<T: Serialize>(value: &T) -> Result<Value, JsValue> {
    serde_json::to_value(value)
        .map_err(|e| JsValue::from_str(&format!("serialization failed: {e}")))
}

fn parse_u32_value(v: Option<&Value>, key: &str, default: u32) -> Result<u32, JsValue> {
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
                .map_err(|e| JsValue::from_str(&format!("invalid hex value for {key}: {e}")));
        }
        return s
            .parse::<u32>()
            .map_err(|e| JsValue::from_str(&format!("invalid decimal value for {key}: {e}")));
    }

    Err(JsValue::from_str(&format!("invalid value for {key}")))
}

fn parse_u64_value(v: Option<&Value>, key: &str, default: u64) -> Result<u64, JsValue> {
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
                .map_err(|e| JsValue::from_str(&format!("invalid hex value for {key}: {e}")));
        }
        return s
            .parse::<u64>()
            .map_err(|e| JsValue::from_str(&format!("invalid decimal value for {key}: {e}")));
    }

    Err(JsValue::from_str(&format!("invalid value for {key}")))
}

fn parse_string_value(v: Option<&Value>, key: &str) -> Result<String, JsValue> {
    let Some(args) = v else {
        return Err(JsValue::from_str("missing arguments"));
    };
    let Some(raw) = args.get(key) else {
        return Err(JsValue::from_str(&format!("missing argument: {key}")));
    };
    let Some(s) = raw.as_str() else {
        return Err(JsValue::from_str(&format!(
            "argument {key} must be a string"
        )));
    };
    Ok(s.to_string())
}

fn parse_base64_bytes(v: Option<&Value>, key: &str) -> Result<Vec<u8>, JsValue> {
    let encoded = parse_string_value(v, key)?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| JsValue::from_str(&format!("invalid base64 value for {key}: {e}")))
}

fn parse_write_bytes(v: Option<&Value>) -> Result<Vec<u8>, JsValue> {
    let Some(args) = v else {
        return Err(JsValue::from_str("missing arguments"));
    };

    if let Some(value) = args.get("bytesBase64") {
        let Some(encoded) = value.as_str() else {
            return Err(JsValue::from_str("argument bytesBase64 must be a string"));
        };
        return base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| JsValue::from_str(&format!("invalid base64 value for bytesBase64: {e}")));
    }

    let Some(value) = args.get("bytes") else {
        return Err(JsValue::from_str("missing argument: bytes or bytesBase64"));
    };
    let Some(items) = value.as_array() else {
        return Err(JsValue::from_str(
            "argument bytes must be an array of integers",
        ));
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(byte) = item.as_u64() else {
            return Err(JsValue::from_str(
                "argument bytes must be an array of integers",
            ));
        };
        if byte > u8::MAX as u64 {
            return Err(JsValue::from_str(&format!(
                "byte value {byte} is outside the range 0..=255"
            )));
        }
        out.push(byte as u8);
    }
    Ok(out)
}

fn parse_reset_policy(v: Option<&Value>) -> Result<ResetMemoryPolicy, JsValue> {
    let Some(args) = v else {
        return Ok(ResetMemoryPolicy::Ignore);
    };
    let Some(raw) = args.get("policy") else {
        return Ok(ResetMemoryPolicy::Ignore);
    };
    let Some(value) = raw.as_str() else {
        return Err(JsValue::from_str("argument policy must be a string"));
    };
    match value {
        "ignore" => Ok(ResetMemoryPolicy::Ignore),
        "zero" => Ok(ResetMemoryPolicy::Zero),
        "random" | "randomize" => Ok(ResetMemoryPolicy::Randomize),
        _ => Err(JsValue::from_str(&format!(
            "unsupported reset policy: {value}"
        ))),
    }
}

fn parse_dump_value(v: Option<&Value>) -> Result<VmDump, JsValue> {
    let Some(args) = v else {
        return Err(JsValue::from_str("missing arguments"));
    };
    let Some(raw) = args.get("dump") else {
        return Err(JsValue::from_str("missing argument: dump"));
    };
    serde_json::from_value(raw.clone())
        .map_err(|e| JsValue::from_str(&format!("invalid dump payload: {e}")))
}

fn builtin_examples() -> Vec<Value> {
    bundled_examples::EXAMPLES
        .iter()
        .map(|example| serde_json::json!({ "name": example.name }))
        .collect()
}

fn builtin_example_bytes(name: &str) -> Option<(&'static str, &'static [u8])> {
    bundled_examples::EXAMPLES
        .iter()
        .find(|example| example.name == name)
        .map(|example| (example.name, example.bytes))
}

fn response(id: u64, command: String, body: Value) -> ControlResponse {
    ControlResponse {
        id,
        seq: id,
        success: true,
        command,
        message: None,
        body,
    }
}

fn error_response(id: u64, command: String, message: String) -> ControlResponse {
    ControlResponse {
        id,
        seq: id,
        success: false,
        command,
        message: Some(message),
        body: Value::Null,
    }
}

impl HecateVmWasm {
    fn execute(&mut self, req: ControlRequest) -> Result<ControlResponse, JsValue> {
        if let Some(raw_type) = req.request_type.as_deref() {
            if raw_type != "request" {
                return Ok(error_response(
                    req.id.unwrap_or(0),
                    req.command,
                    "if provided, type must be 'request'".to_string(),
                ));
            }
        }

        let id = req.id.unwrap_or(0);
        let command = req.command.clone();
        let args = req.arguments.as_ref();

        let reply = match command.as_str() {
            "initialize" => response(id, command, Value::Null),
            "examples" => response(
                id,
                command,
                serde_json::json!({ "examples": builtin_examples() }),
            ),
            "load" => {
                let name = parse_string_value(args, "name")?;
                let (name, bytes) = builtin_example_bytes(&name)
                    .ok_or_else(|| JsValue::from_str(&format!("Unknown example: {name}")))?;
                let state = self
                    .vm
                    .load(name, bytes)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(
                    id,
                    command,
                    serde_json::json!({
                        "entry": state.entry_point,
                        "entryHex": format!("0x{:08x}", state.entry_point),
                        "state": to_json_value(&state)?
                    }),
                )
            }
            "run" => {
                self.vm
                    .run()
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(id, command, Value::Null)
            }
            "tick" => {
                let quantum = parse_u64_value(args, "quantum", 5_000)?;
                self.vm.tick_running(quantum);
                response(
                    id,
                    command,
                    serde_json::json!({ "state": to_json_value(&self.vm.state())? }),
                )
            }
            "pause" => response(
                id,
                command,
                serde_json::json!({ "state": to_json_value(&self.vm.pause())? }),
            ),
            "step" => {
                let count = parse_u64_value(args, "count", 1)?;
                let state = if count <= 1 {
                    self.vm.step()
                } else {
                    self.vm.step_count(count)
                }
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(
                    id,
                    command,
                    serde_json::json!({ "state": to_json_value(&state)? }),
                )
            }
            "step_over" => response(
                id,
                command,
                serde_json::json!({
                    "state": to_json_value(
                        &self
                            .vm
                            .step_over()
                            .map_err(|e| JsValue::from_str(&e.to_string()))?,
                    )?
                }),
            ),
            "step_out" => response(
                id,
                command,
                serde_json::json!({
                    "state": to_json_value(
                        &self
                            .vm
                            .step_out()
                            .map_err(|e| JsValue::from_str(&e.to_string()))?,
                    )?
                }),
            ),
            "reset" => {
                let state = self
                    .vm
                    .reset(parse_reset_policy(args)?)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(
                    id,
                    command,
                    serde_json::json!({ "state": to_json_value(&state)? }),
                )
            }
            "read" => {
                let addr = parse_u32_value(args, "addr", 0)?;
                let len = parse_u32_value(args, "len", 64)?;
                let result: MemoryReadResult = self
                    .vm
                    .read(addr, len)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(id, command, serde_json::json!({ "result": result }))
            }
            "write" => {
                let addr = parse_u32_value(args, "addr", 0)?;
                let bytes = parse_write_bytes(args)?;
                self.vm
                    .write(addr, &bytes)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(
                    id,
                    command,
                    serde_json::json!({ "state": to_json_value(&self.vm.state())? }),
                )
            }
            "state" => response(
                id,
                command,
                serde_json::json!({ "state": to_json_value(&self.vm.state())? }),
            ),
            "dump" => response(
                id,
                command,
                serde_json::json!({ "dump": to_json_value(&self.vm.dump())? }),
            ),
            "restore" => {
                let dump = parse_dump_value(args)?;
                let state = self
                    .vm
                    .restore(dump)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                response(
                    id,
                    command,
                    serde_json::json!({ "state": to_json_value(&state)? }),
                )
            }
            other => error_response(
                id,
                other.to_string(),
                format!("unsupported control command: {other}"),
            ),
        };

        Ok(reply)
    }
}

#[wasm_bindgen]
impl HecateVmWasm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<HecateVmWasm, JsValue> {
        Ok(Self { vm: default_vm()? })
    }

    #[wasm_bindgen(js_name = createHecateVm)]
    pub fn create_hecate_vm() -> Result<HecateVmWasm, JsValue> {
        Self::new()
    }

    #[wasm_bindgen(js_name = control)]
    pub fn control(&mut self, request: JsValue) -> Result<JsValue, JsValue> {
        let parsed: ControlRequest = serde_wasm_bindgen::from_value(request)
            .map_err(|e| JsValue::from_str(&format!("invalid control request: {e}")))?;
        let reply = self.execute(parsed)?;
        let payload = serde_json::to_string(&reply)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {e}")))?;
        Ok(JsValue::from_str(&payload))
    }

    #[wasm_bindgen(js_name = listExamples)]
    pub fn list_examples(&mut self) -> Result<JsValue, JsValue> {
        let payload = serde_json::to_string(&serde_json::json!({ "examples": builtin_examples() }))
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {e}")))?;
        Ok(JsValue::from_str(&payload))
    }
}
