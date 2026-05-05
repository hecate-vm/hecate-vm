use std::path::PathBuf;

use clap::{Parser, Subcommand};
use hecate_common::{
    native::{HostIO, NativeCpu},
    BytecodeFile, RunMode,
};

#[cfg(feature = "experimental_ui")]
use macroquad::prelude::*;

#[cfg(feature = "experimental_ui")]
use macroquad::ui::root_ui;

#[cfg(not(feature = "experimental_ui"))]
use macroquad::prelude::Conf;

#[derive(Parser)]
struct Args {
    #[arg(short, long, global = true)]
    verbose: bool,
    #[arg(short, long, global = true)]
    print_memory_access: bool,
    #[arg(short, long, global = true)]
    show_cpu_state: bool,
    #[arg(short, long, global = true)]
    addresses_as_integers: bool,
    #[arg(short, long, global = true, default_value = "1048576")]
    memory_size: u32,
    #[arg(short, long, global = true, default_value = "32")]
    register_count: u8,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    #[cfg(feature = "experimental_ui")]
    Gui,
    Run {
        path: PathBuf,
    },
}

#[derive(Debug)]
struct SimpleHostIo;

impl HostIO for SimpleHostIo {
    fn syscall(
        &mut self,
        code: u32,
        cpu: &mut NativeCpu<Self>,
    ) -> Result<usize, hecate_common::ExecutionError>
    where
        Self: Sized,
    {
        match code {
            0 => {
                let start = cpu.get_registers()[2] as usize;
                let length = cpu.get_registers()[3] as usize;
                let mem = &cpu.get_memory()[start..start + length];
                let s = String::from_utf8(
                    mem.iter()
                        .map(|v| u8::from_le_bytes([v.to_le_bytes()[0]]))
                        .collect::<Vec<_>>(),
                )?;
                eprint!("{s}");
                Ok(2500 + (length * 300))
            }
            _ => Err(hecate_common::ExecutionError::InvalidSyscall(code)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    rom: &[u32],
    total_memory_size: u32,
    entrypoint: u32,
    register_count: u8,
    verbose: bool,
    print_memory_access: bool,
    show_cpu_state: bool,
    addresses_as_integers: bool,
) -> anyhow::Result<()> {
    let mut cpu = NativeCpu::new(total_memory_size, register_count, SimpleHostIo);
    cpu.set_verbose(verbose);
    cpu.set_print_memory_access(print_memory_access);
    cpu.set_entrypoint(entrypoint);
    cpu.set_addresses_as_integers(addresses_as_integers);

    cpu.load_protected_memory(0, rom);

    let result = cpu.execute(RunMode::Run);

    println!();
    println!("========== RESULT/STATS ===========");
    println!();

    println!("{:#?}", result);

    if show_cpu_state {
        cpu.print_state();
    }
    Ok(())
}

#[cfg(feature = "experimental_ui")]
async fn run_gui() -> anyhow::Result<()> {
    let mut cpu = NativeCpu::new(1024 * 1024, 32, SimpleHostIo);
    cpu.set_verbose(true);

    let mut paused = true;
    let mut running = true;
    while running {
        clear_background(WHITE);

        if if paused {
            root_ui().button(None, "Unpause")
        } else {
            root_ui().button(None, "Pause")
        } {
            paused = !paused;
        }

        if root_ui().button(None, "Stop") {
            running = false;
        }

        if !paused {
            cpu.execute(RunMode::RunFor(1))?;
        }

        if cpu.get_halted() {
            running = false;
        }

        next_frame().await
    }

    Ok(())
}

fn conf() -> Conf {
    Conf {
        window_title: "Hecate VM".to_string(),
        window_width: 800,
        window_height: 600,
        ..Default::default()
    }
}

#[macroquad::main(conf)]
async fn main() -> anyhow::Result<()> {
    let Args {
        action,
        verbose,
        print_memory_access,
        show_cpu_state,
        memory_size,
        register_count,
        addresses_as_integers,
    } = Args::parse();

    match action {
        #[cfg(feature = "experimental_ui")]
        Action::Gui => {
            run_gui().await?;
        }
        Action::Run { path } => {
            let file = BytecodeFile::load(path).unwrap();

            run(
                &file.data,
                memory_size,
                file.header.entrypoint,
                register_count,
                verbose,
                print_memory_access,
                show_cpu_state,
                addresses_as_integers,
            )?;
        }
    }

    Ok(())
}
