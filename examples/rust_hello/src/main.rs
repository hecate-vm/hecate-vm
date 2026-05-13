#![no_std]
#![no_main]

use hecate::prelude::*;

#[hecate::main]
fn main() -> Result<(), i32> {
    hprintln!("Hello from Rust on Hecate!");

    let mut data: Vec<i32> = Vec::new();
    for v in [3, 1, 4, 1, 5, 9, 2, 6] {
        data.push(v);
    }

    let mut sum = 0i32;
    for v in data {
        sum += v;
    }

    let mut msg = String::from("sum=");
    if sum >= 0 {
        let mut n = sum as u32;
        let mut digits = Vec::new();
        if n == 0 {
            digits.push(b'0');
        } else {
            while n > 0 {
                digits.push(b'0' + (n % 10) as u8);
                n /= 10;
            }
            digits.reverse();
        }
        for d in digits {
            msg.push(d as char);
        }
    } else {
        msg.push('?');
    }

    hprintln!("{}", msg);
    Ok(())
}
