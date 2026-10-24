//! Reproducible offline measurements, not robot/network or end-to-end latency.
use serde_json::{json, Value};
use std::hint::black_box;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use unitree_webrtc_rs::datachannel::lidar::bits_to_points_optimized;
use unitree_webrtc_rs::protocol::events::event_channel;
use unitree_webrtc_rs::protocol::models::CallbackEvent;

const SAMPLES: usize = 2_000;
const WARMUP: usize = 100;
const HANDOFF_BATCH: usize = 128;
const SATURATION_BATCH: usize = 32;

fn measure(
    name: &str,
    unit: &str,
    operations_per_sample: usize,
    workload: Value,
    mut operation: impl FnMut() -> usize,
) -> Value {
    for _ in 0..WARMUP {
        black_box(operation());
    }
    let mut nanos = Vec::with_capacity(SAMPLES);
    let mut units = 0usize;
    let started = Instant::now();
    for _ in 0..SAMPLES {
        let sample = Instant::now();
        for _ in 0..operations_per_sample {
            units += black_box(operation());
        }
        nanos.push(sample.elapsed().as_nanos());
    }
    let seconds = started.elapsed().as_secs_f64();
    nanos.sort_unstable();
    // Nearest-rank percentiles; SAMPLES is fixed and divisible by 100.
    json!({
        "name": name,
        "samples": SAMPLES,
        "warmup_iterations": WARMUP,
        "operations_per_sample": operations_per_sample,
        "total_operations": SAMPLES * operations_per_sample,
        "sample_latency_ns": {
            "p50": nanos[SAMPLES / 2 - 1],
            "p95": nanos[SAMPLES * 95 / 100 - 1],
            "p99": nanos[SAMPLES * 99 / 100 - 1],
            "max": nanos[SAMPLES - 1],
        },
        "batch_average_operation_ns": {
            "p50": nanos[SAMPLES / 2 - 1] as f64 / operations_per_sample as f64,
            "p95": nanos[SAMPLES * 95 / 100 - 1] as f64 / operations_per_sample as f64,
            "p99": nanos[SAMPLES * 99 / 100 - 1] as f64 / operations_per_sample as f64,
            "max": nanos[SAMPLES - 1] as f64 / operations_per_sample as f64,
        },
        "unit": unit,
        "total_units": units,
        "units_per_second": units as f64 / seconds,
        "samples_per_second": SAMPLES as f64 / seconds,
        "operations_per_second": (SAMPLES * operations_per_sample) as f64 / seconds,
        "workload": workload,
        "elapsed_seconds": seconds,
    })
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("run this benchmark with cargo bench (optimized profile)".into());
    }

    // One set bit per byte: 131,072 output points, well below the decoder quota.
    let bitmap = vec![0x80; 128 * 1024];
    let lidar = measure(
        "lidar_bitmap_128k_sparse",
        "points",
        1,
        json!({
            "bitmap_bytes": 131_072,
            "set_bits_per_byte": 1,
            "output_points_per_iteration": 131_072,
            "output_f32_elements_per_iteration": 393_216,
            "output_bytes_per_iteration": 1_572_864,
            "includes": "bitmap validation, point allocation/decoding and output drop",
        }),
        || {
            let points = bits_to_points_optimized(black_box(&bitmap), &[1.0, 2.0, 3.0], 0.05)
                .expect("fixed valid offline fixture");
            assert_eq!(points.len(), 393_216);
            black_box(points).len() / 3
        },
    );

    // Reuse the same owned allocation: this measures admission/ownership transfer,
    // not frame allocation, GStreamer, Python attachment, or callback execution.
    let (sender, receiver) = event_channel(4)?;
    let mut event = Some(CallbackEvent::VideoFrame {
        data: vec![0; 64 * 48 * 3],
        width: 64,
        height: 48,
    });
    let events = measure(
        "owned_event_handoff_64x48",
        "events",
        HANDOFF_BATCH,
        json!({
            "width": 64,
            "height": 48,
            "buffer_bytes": 9_216,
            "includes": "same-thread media admission, dequeue and owned allocation reuse",
            "excludes": "frame allocation, GStreamer, Python attachment and callbacks",
        }),
        || {
            sender
                .try_send(event.take().expect("previous iteration returned ownership"))
                .expect("one event in an empty live queue");
            event = Some(receiver.try_recv().expect("one admitted event"));
            1
        },
    );

    // A bounded saturation/recovery workload: rejected events must not retain quota.
    let saturation = measure(
        "media_saturation_recovery",
        "rounds",
        SATURATION_BATCH,
        json!({
            "accepted_per_iteration": 4,
            "rejected_full_per_iteration": 1,
            "drained_per_iteration": 4,
            "audio_i16_elements_per_event": 48,
            "audio_buffer_bytes_per_event": 96,
            "audio_sample_rate_hz": 48_000,
            "audio_channels": 2,
            "measured_accepted_events": 4 * SAMPLES * SATURATION_BATCH,
            "measured_rejected_events": SAMPLES * SATURATION_BATCH,
            "includes": "audio allocation, count-quota rejection, drain and buffer drop",
        }),
        || {
            for _ in 0..4 {
                sender
                    .try_send(CallbackEvent::AudioFrame {
                        data: vec![0; 48],
                        sample_rate: 48_000,
                        channels: 2,
                    })
                    .expect("four media slots");
            }
            assert!(matches!(
                sender.try_send(CallbackEvent::AudioFrame {
                    data: vec![0; 48],
                    sample_rate: 48_000,
                    channels: 2,
                }),
                Err(crossbeam_channel::TrySendError::Full(_))
            ));
            for _ in 0..4 {
                black_box(receiver.try_recv().expect("drain every admitted event"));
            }
            1
        },
    );

    let report = json!({
        "schema_version": 1,
        "measurement_kind": "single-process same-thread offline microbenchmark",
        "baseline": null,
        "limits": "No speedup comparison; no robot, network, fleet, GPU, audio/video codec or Python measurement. Not a soak or retained-array/RSS budget.",
        "scope": "offline Rust bitmap decoder and owned-event channel only",
        "crate_version": env!("CARGO_PKG_VERSION"),
        "source_revision": command_output("git", &["rev-parse", "HEAD"]),
        "tracked_worktree_dirty": Command::new("git")
            .args(["diff", "--quiet", "HEAD", "--", "."])
            .status()
            .ok()
            .map(|status| !status.success()),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "available_parallelism": std::thread::available_parallelism().ok().map(|n| n.get()),
        "platform": command_output("uname", &["-a"]),
        "cpu_model": command_output("sysctl", &["-n", "machdep.cpu.brand_string"]),
        "rustc": command_output("rustc", &["--version", "--verbose"]),
        "unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "profile": "optimized cargo bench, no default Python features",
        "timing_note": "Instant instrumentation included; throughput includes sample recording; host scheduling affects tails",
        "latency_note": "Percentiles are timed batch durations. Normalized operation values are batch averages, not individual-operation tail latency. Small channel operations are batched to reduce clock quantization.",
        "workloads": [lidar, events, saturation],
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
