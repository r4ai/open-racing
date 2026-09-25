//! Capture measurements and report I/O, independent of scene control.

use std::{collections::BTreeMap, fs, io};

use bevy::{diagnostic::DiagnosticsStore, prelude::info};

use super::CloudCpuTimings;

#[derive(Default)]
pub(super) struct Measurements {
    frames: Vec<f64>,
    gpu: BTreeMap<String, Vec<f64>>,
    cpu: [Vec<f64>; 3],
}

impl Measurements {
    pub fn record(&mut self, frame_ms: f64, cpu: &CloudCpuTimings, gpu: &DiagnosticsStore) {
        self.frames.push(frame_ms);
        for (samples, value) in
            self.cpu
                .iter_mut()
                .zip([cpu.weather_ms, cpu.upload_ms, cpu.shadow_ms])
        {
            samples.push(value);
        }
        let mut cloud_total = 0.0;
        for diagnostic in gpu.iter() {
            let path = diagnostic.path().to_string();
            if path.contains("elapsed_gpu")
                && let Some(value) = diagnostic.value()
            {
                if path.contains("/cloud_") {
                    cloud_total += value;
                }
                self.gpu.entry(path).or_default().push(value);
            }
        }
        if cloud_total > 0.0 {
            self.gpu
                .entry("cloud_total".into())
                .or_default()
                .push(cloud_total);
        }
    }

    pub fn write(&mut self, path: &str, stamps: &str) -> io::Result<()> {
        let s = Statistics::new(&mut self.frames)
            .ok_or_else(|| io::Error::other("capture ended without measured frames"))?;
        let report = format!(
            "frames={},p50_ms={:.3},p95_ms={:.3},p99_ms={:.3}\n",
            s.count, s.p50, s.p95, s.p99
        );
        info!("{report}");
        fs::write(format!("{path}.csv"), report)?;
        let mut report = String::from("pass,samples,p50_ms,p95_ms\n");
        for (name, samples) in &mut self.gpu {
            if let Some(s) = Statistics::new(samples) {
                report.push_str(&format!("{name},{},{:.4},{:.4}\n", s.count, s.p50, s.p95));
            }
        }
        fs::write(format!("{path}.gpu.csv"), report)?;
        let mut report = String::from("operation,frames,p95_ms,p99_ms,max_ms\n");
        for (name, samples) in ["weather", "field_upload_cpu", "shadow"]
            .into_iter()
            .zip(&mut self.cpu)
        {
            if let Some(s) = Statistics::new(samples) {
                report.push_str(&format!(
                    "{name},{},{:.4},{:.4},{:.4}\n",
                    s.count, s.p95, s.p99, s.max
                ));
            }
        }
        fs::write(format!("{path}.cpu.csv"), report)?;
        fs::write(format!("{path}.frames.csv"), stamps)
    }
}

struct Statistics {
    count: usize,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

impl Statistics {
    fn new(samples: &mut [f64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_by(f64::total_cmp);
        let percentile = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
        Some(Self {
            count: samples.len(),
            p50: percentile(0.5),
            p95: percentile(0.95),
            p99: percentile(0.99),
            max: samples[samples.len() - 1],
        })
    }
}
