#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::asm;
use core::fmt::{self, Write};
use core::ptr::null_mut;

pub const HECATE_SYS_WRITE: usize = 64;
pub const HECATE_SYS_EXIT: usize = 93;
pub type ExitCode = i32;

pub trait MainReturn {
    fn into_exit_code(self) -> ExitCode;
}

impl MainReturn for () {
    fn into_exit_code(self) -> ExitCode {
        0
    }
}

impl MainReturn for ExitCode {
    fn into_exit_code(self) -> ExitCode {
        self
    }
}

impl<T, E> MainReturn for Result<T, E>
where
    T: MainReturn,
    E: core::fmt::Debug,
{
    fn into_exit_code(self) -> ExitCode {
        match self {
            Ok(v) => v.into_exit_code(),
            Err(err) => {
                print(core::format_args!("main error: {:?}\n", err));
                1
            }
        }
    }
}

const HEAP_SIZE: usize = 256 * 1024;

struct BumpAllocator;

#[repr(align(16))]
struct AlignedHeap([u8; HEAP_SIZE]);

static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_SIZE]);
static mut NEXT: usize = 0;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let align = layout.align();
            let size = layout.size();
            if size == 0 {
                return null_mut();
            }

            let aligned = (NEXT + (align - 1)) & !(align - 1);
            let next = aligned.saturating_add(size);
            if next > HEAP_SIZE {
                return null_mut();
            }

            let base = core::ptr::addr_of_mut!(HEAP.0) as *mut u8;
            NEXT = next;
            base.add(aligned)
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        /* no-op: monotonic bump allocator */
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: BumpAllocator = BumpAllocator;

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    let _ = write_stderr(b"alloc error\n");
    process::exit(1)
}

#[inline(always)]
pub fn sys_write(fd: i32, buf: *const u8, len: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") fd as isize => ret,
            in("a1") buf,
            in("a2") len,
            in("a7") HECATE_SYS_WRITE,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub fn write_stdout(bytes: &[u8]) -> isize {
    sys_write(1, bytes.as_ptr(), bytes.len())
}

#[inline(always)]
pub fn write_stderr(bytes: &[u8]) -> isize {
    sys_write(2, bytes.as_ptr(), bytes.len())
}

#[inline(always)]
pub fn puts(s: &str) {
    let _ = write_stdout(s.as_bytes());
    let _ = write_stdout(b"\n");
}

#[inline(always)]
pub fn exit(code: i32) -> ! {
    unsafe {
        asm!(
            "ecall",
            in("a0") code,
            in("a7") HECATE_SYS_EXIT,
            clobber_abi("C"),
            options(noreturn)
        );
    }
}

pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _ = write_stdout(s.as_bytes());
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let mut console = Console;
    let _ = console.write_fmt(args);
}

pub fn write_u32(mut value: u32) {
    let mut buf = [0u8; 10];
    let mut len = 0usize;

    if value == 0 {
        let _ = write_stdout(b"0");
        return;
    }

    while value > 0 {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }

    let mut out = [0u8; 10];
    for i in 0..len {
        out[i] = buf[len - 1 - i];
    }
    let _ = write_stdout(&out[..len]);
}

pub fn write_i32(value: i32) {
    if value < 0 {
        let _ = write_stdout(b"-");
        write_u32(value.unsigned_abs());
    } else {
        write_u32(value as u32);
    }
}

pub mod io {
    #[inline(always)]
    pub fn write_stdout(bytes: &[u8]) -> isize {
        crate::write_stdout(bytes)
    }

    #[inline(always)]
    pub fn write_stderr(bytes: &[u8]) -> isize {
        crate::write_stderr(bytes)
    }

    #[inline(always)]
    pub fn puts(s: &str) {
        crate::puts(s)
    }
}

pub mod process {
    #[inline(always)]
    pub fn exit(code: i32) -> ! {
        crate::exit(code)
    }
}

pub mod prelude {
    pub use crate::std::{String, Vec};
    pub use crate::{ExitCode, MainReturn, hprint, hprintln, write_i32, write_u32};
}

/* Minimal std-like compatibility layer for no_std + alloc apps. */
pub mod std {
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;

    pub mod string {
        pub use alloc::string::{String, ToString};
    }

    pub mod vec {
        pub use alloc::vec::Vec;
    }
}

#[macro_export]
macro_rules! hprint {
    ($($arg:tt)*) => {
        $crate::print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! hprintln {
    () => {
        $crate::hprint!("\n")
    };
    ($fmt:expr) => {
        $crate::hprint!(concat!($fmt, "\n"))
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::hprint!(concat!($fmt, "\n"), $($arg)*)
    };
}

#[macro_export]
macro_rules! hecate_entry {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start() -> ! {
            let code: $crate::ExitCode = $main();
            $crate::process::exit(code)
        }

        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            $crate::hprintln!("panic");
            $crate::process::exit(1)
        }
    };
}
