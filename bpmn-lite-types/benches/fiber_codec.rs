#![forbid(unsafe_code)]

use std::time::Instant;

#[derive(
    Clone, serde::Serialize, serde::Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
enum BenchValue {
    Bool(bool),
    I64(i64),
    Interned(u32),
}

#[derive(
    serde::Serialize, serde::Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
struct BenchFiberFrame {
    pc: u32,
    stack: Vec<BenchValue>,
    registers: Vec<BenchValue>,
    loop_epoch: u64,
    wait_tag: u8,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frame = BenchFiberFrame {
        pc: 413,
        stack: (0..64).map(|value| BenchValue::I64(value * 17)).collect(),
        registers: (0..32)
            .map(|index| {
                if index % 2 == 0 {
                    BenchValue::Bool(false)
                } else {
                    BenchValue::Interned(7)
                }
            })
            .collect(),
        loop_epoch: 9,
        wait_tag: 2,
    };
    let postcard_bytes = postcard::to_allocvec(&frame)?;
    let rkyv_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&frame)?;

    const ITERATIONS: usize = 100_000;
    let start = Instant::now();
    let mut postcard_total = 0usize;
    for _ in 0..ITERATIONS {
        postcard_total = postcard_total.saturating_add(postcard::to_allocvec(&frame)?.len());
    }
    let postcard_elapsed = start.elapsed();

    let start = Instant::now();
    let mut rkyv_total = 0usize;
    for _ in 0..ITERATIONS {
        rkyv_total =
            rkyv_total.saturating_add(rkyv::to_bytes::<rkyv::rancor::Error>(&frame)?.len());
    }
    let rkyv_elapsed = start.elapsed();

    println!(
        "postcard_bytes={} rkyv_bytes={} postcard_ns_per_encode={} rkyv_ns_per_encode={} checksum={}",
        postcard_bytes.len(),
        rkyv_bytes.len(),
        postcard_elapsed.as_nanos() / ITERATIONS as u128,
        rkyv_elapsed.as_nanos() / ITERATIONS as u128,
        postcard_total.saturating_add(rkyv_total),
    );
    Ok(())
}
