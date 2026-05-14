use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use hecate_vm::{
    DEFAULT_CONFIG, HecateClock, HecateMemory, RunStats, SimConfig, SimConfigRaw, handle_syscall,
    load_config, load_elf, syscall_cycles_for,
};
use rvsim::{CpuError, CpuState, Interp};

mod debug_ui;

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
