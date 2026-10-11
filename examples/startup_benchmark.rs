//! Run equivalent --version subprocess launches; report measured wall time only.
use anyhow::{ensure, Result};
use serde_json::json;
use std::{
    process::{Command, Stdio},
    time::Instant,
};
fn main() -> Result<()> {
    let executable = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: startup_benchmark EXECUTABLE [COUNT]"))?;
    let count = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(20);
    ensure!(count > 0, "count must be positive");
    let mut times = Vec::new();
    for _ in 0..count {
        let start = Instant::now();
        let status = Command::new(&executable)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        ensure!(status.success(), "--version failed");
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let median = if count % 2 == 0 {
        (times[count / 2 - 1] + times[count / 2]) / 2.0
    } else {
        times[count / 2]
    };
    println!(
        "{}",
        json!({"executable":executable,"launches":count,"median_ms":median,"min_ms":times[0],"max_ms":times[count-1],"peak_rss_bytes":null})
    );
    Ok(())
}
