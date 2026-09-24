//! `flowlin-bench <folder>`: times enumeration and thumbnail latency to
//! check the performance targets (Section 5 of the spec).

use std::path::PathBuf;
use std::time::Instant;

use flowlin::fs::scan::{spawn_scan, Cancel, ScanMsg, ScanOptions};

fn main() {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: flowlin-bench <folder> [--recursive]");
        std::process::exit(2);
    });
    let recursive = std::env::args().any(|a| a == "--recursive");
    flowlin::fs::formats::init();

    let opts = ScanOptions { recursive, show_hidden: false, same_device: false, cap: 10_000_000, videos: false };
    let t0 = Instant::now();
    let rx = spawn_scan(dir.clone(), opts, Cancel::default());
    let mut first = None;
    let mut all = Vec::new();
    while let Ok(msg) = rx.recv_blocking() {
        match msg {
            ScanMsg::Batch(v) => {
                if first.is_none() {
                    first = Some(t0.elapsed());
                }
                all.extend(v);
            }
            ScanMsg::Done { .. } => break,
            ScanMsg::Error(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
    let total = t0.elapsed();
    println!("files:              {}", all.len());
    println!("first batch:        {:?}", first.unwrap_or_default());
    println!("full enumeration:   {total:?}");

    all.sort_by(|a, b| flowlin::util::natural_cmp(&a.name, &b.name));
    let sample: Vec<_> = all.iter().take(64).collect();
    if sample.is_empty() {
        return;
    }
    let t1 = Instant::now();
    let r = flowlin::decode::thumbnail(&sample[0].path, 256);
    println!("first thumbnail:    {:?} ({})", t1.elapsed(), if r.is_ok() { "ok" } else { "failed" });

    use rayon::prelude::*;
    let t2 = Instant::now();
    let ok = sample.par_iter().filter(|e| flowlin::decode::thumbnail(&e.path, 256).is_ok()).count();
    let el = t2.elapsed();
    println!(
        "{} thumbnails (256px, parallel): {el:?} ({:.1} ms each, {ok} ok)",
        sample.len(),
        el.as_secs_f64() * 1000.0 / sample.len() as f64
    );

    let t3 = Instant::now();
    let r = flowlin::decode::load_full(&sample[0].path);
    println!("first full decode:  {:?} ({})", t3.elapsed(), if r.is_ok() { "ok" } else { "failed" });
}
