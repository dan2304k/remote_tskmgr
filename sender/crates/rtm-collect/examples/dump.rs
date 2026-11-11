//! Build every known collector, sample twice 500 ms apart and print the second sample as a
//! JSON `metrics` object. `cargo run -p rtm-collect --example dump [-- cpu mem ...]`

use rtm_collect::{CollectorConfig, Collectors, KNOWN_IDS};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = CollectorConfig::standard();
    cfg.enabled = if args.is_empty() {
        KNOWN_IDS.iter().map(|s| s.to_string()).collect()
    } else {
        args
    };
    let t0 = Instant::now();
    let mut c = Collectors::new(&cfg);
    eprintln!("enabled: {:?} (init {:?})", c.ids(), t0.elapsed());
    let dt = Duration::from_millis(500);
    c.sample_all(dt);
    std::thread::sleep(dt);
    let t1 = Instant::now();
    let sample = c.sample_all(dt);
    eprintln!("sample took {:?}", t1.elapsed());
    let mut m = serde_json::Map::new();
    for (k, v) in sample {
        m.insert(k, serde_json::to_value(v).unwrap());
    }
    println!("{}", serde_json::to_string_pretty(&m).unwrap());
}
