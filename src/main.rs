#[cfg(not(target_arch = "wasm32"))]
mod native_main {
    use std::path::PathBuf;

    use anyhow::{Context, anyhow};
    use clap::{Parser, Subcommand};
    use hecate_vm::debug_ui;
    use hecate_vm::vm::{
        HecateVm, SimConfig, SimConfigRaw, VmRuntimeOptions, load_config, syscall_cycles_for,
    };

    const DEFAULT_CONFIG: &str = include_str!("default.toml");

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
            #[arg(long)]
            config: Option<PathBuf>,
            #[arg(long, default_value_t = false)]
            debug_ui: bool,
            #[arg(long, default_value_t = 8581)]
            debug_port: u16,
        },
    }

    fn report_result(vm: &HecateVm, dump_registers: bool, config: &SimConfig) {
        let state = vm.state();

        println!();
        println!("========== RESULT/STATS ==========");
        println!();

        println!("Stop reason: {:?}", state.stop_reason);
        println!("PC: {:#010x}", state.pc);
        println!("Entry: {:#010x}", state.entry_point);
        println!(
            "Loaded binary: {}",
            state.loaded_binary_name.as_deref().unwrap_or("<none>")
        );
        if let Some(hash) = state.loaded_binary_hash.as_deref() {
            println!("Loaded binary SHA-256: {hash}");
        }
        println!("Current instruction: {}", state.current_instruction.text);
        println!("Score (cycles): {}", state.stats.cycles);
        println!("Instructions retired: {}", state.stats.instret);
        println!("Memory accesses: {}", state.stats.memory_access_count);
        println!("Instruction fetches: {}", state.stats.instruction_fetches);
        println!("Data loads: {}", state.stats.data_loads);
        println!("Data stores: {}", state.stats.data_stores);
        println!("Syscalls: {}", state.stats.syscall_count);
        println!(
            "Syscall cycles contribution: {}",
            state.stats.syscall_cycles
        );
        println!("I/O cycles contribution: {}", state.stats.io_cycles);
        println!("IO Bytes Written: {}", state.stats.io_bytes_written);
        println!("Cache hits L1I: {}", state.stats.cache_hits.l1i);
        println!("Cache hits L1D: {}", state.stats.cache_hits.l1d);
        println!("Cache hits L2: {}", state.stats.cache_hits.l2);
        println!("Cache hits L3: {}", state.stats.cache_hits.l3);

        if !state.stats.syscall_hits.is_empty() {
            println!();
            println!("Syscall breakdown:");
            let mut calls: Vec<(u32, u64)> = state
                .stats
                .syscall_hits
                .iter()
                .map(|(code, count)| (*code, *count))
                .collect();
            calls.sort_by_key(|(code, _)| *code);

            for (code, count) in calls {
                let base_cycles =
                    syscall_cycles_for(code, config.default_syscall_cycles, &config.syscall_cycles);
                let subtotal = state
                    .stats
                    .syscall_cycle_totals
                    .get(&code)
                    .copied()
                    .unwrap_or(0);
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
            for (idx, reg) in state.registers.iter().enumerate() {
                println!("x{idx:02}: {:#010x} ({})", reg, *reg as i32);
            }
        }
    }

    fn run_elf(
        path: PathBuf,
        options: VmRuntimeOptions,
        dump_registers: bool,
        config: SimConfig,
    ) -> anyhow::Result<()> {
        let mut vm = HecateVm::new(options, config.clone());
        vm.load_file(&path)?;
        vm.run()?;
        while vm.is_running() {
            vm.tick_running(50_000);
        }
        report_result(&vm, dump_registers, &config);
        Ok(())
    }

    pub fn main() -> anyhow::Result<()> {
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
                debug_ui: enable_debug_ui,
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

                let options = VmRuntimeOptions {
                    cache_line_size,
                    l1_size,
                    l2_size,
                    l3_size,
                    max_instructions,
                };

                if enable_debug_ui {
                    debug_ui::serve(path, options, config, debug_port)?;
                } else {
                    let Some(path) = path else {
                        return Err(anyhow!(
                            "A binary path is required unless --debug-ui is enabled."
                        ));
                    };

                    run_elf(path, options, dump_registers, config)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    native_main::main()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
