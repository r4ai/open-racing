//! Sound lab: renders a preset of a sample engine and writes the WAV, a spectrogram PNG
//! (log frequency, 20 Hz – 16 kHz, 70 dB) and the figures of `analysis` along it:
//! `cargo run -p open-racing-engine-sim --example lab -- i4|v8 sweep out-prefix [draft|normal|high]`.
//! `FS=<Pa>` writes the WAV at a fixed full scale, for comparing recordings' levels;
//! `POPS=spark,lambda,throttle,above` fits a pop map and `CUT=spark` a spark-cut limiter.
use std::f64::consts::PI;

use open_racing_engine_sim::*;

fn main() {
    let mut a = std::env::args().skip(1);
    let engine = a.next().unwrap_or("i4".into());
    let preset = a.next().unwrap_or("sweep".into());
    let out = a.next().unwrap_or("lab".into());
    let q = match a.next().as_deref() {
        Some("draft") => Quality::Draft,
        Some("high") => Quality::High,
        _ => Quality::Normal,
    };
    let (mut e, i, x) = if engine == "v8" {
        (samples::v8(), samples::v8_intake(), samples::v8_exhaust())
    } else {
        (samples::i4(), samples::i4_intake(), samples::i4_exhaust())
    };
    // `POPS=spark,lambda,throttle,above` fits a pop map; `CUT=spark` a spark-cut limiter.
    if let Ok(v) = std::env::var("POPS") {
        let v: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect();
        e.ecu.pops = Some(spec::Pops {
            spark_deg: v[0],
            lambda: v[1],
            throttle: v[2],
            above_rpm: v[3],
        });
    }
    if std::env::var("CUT").is_ok_and(|v| v == "spark") {
        e.ecu.limiter_cut = spec::Cut::Spark;
    }
    let b = Build::new(&e)
        .system("intake", &i)
        .system("exhaust", &x)
        .quality(q);
    let script = Script::preset(&preset, &e).expect("preset");
    let t = std::time::Instant::now();
    let rec = render::render(&b, &script, None).unwrap();
    let el = t.elapsed().as_secs_f64();
    let fs = std::env::var("FS").ok().and_then(|v| v.parse().ok());
    rec.write_wav(format!("{out}.wav").as_ref(), -1.0, fs)
        .unwrap();
    let x: Vec<f64> = rec.channels[0].iter().map(|&v| v as f64).collect();
    spectrogram(&x, rec.rate as f64, &format!("{out}.png"));
    println!(
        "{engine} {preset} {q:?}: {:.2}x real time, peak {:.2} Pa",
        script.duration / el,
        rec.peak(),
    );
    let rep = analysis::analyse(&rec, (script.duration / 0.17) as usize);
    let m = &rep.mics[0];
    println!("    t    rpm     dB  sone  acum  tonal  strongest orders");
    for s in &m.order_track {
        let mut top: Vec<(f64, f64)> = m
            .orders
            .iter()
            .copied()
            .zip(s.orders_db.iter().copied())
            .collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top: Vec<String> = top
            .iter()
            .take(4)
            .map(|(o, d)| format!("{o}:{d:.0}"))
            .collect();
        println!(
            "{:5.2} {:6.0} {:6.1} {:5.0} {:5.2} {:5.1}%  {}",
            s.time,
            s.rpm,
            s.level_db,
            s.loudness,
            s.sharpness,
            100.0 * s.tonal,
            top.join(" ")
        );
    }
    println!(
        "{:.1} dB, {:.0} sone, {:.2} acum, {:.1} % tonal",
        m.level_db,
        m.loudness,
        m.sharpness,
        100.0 * m.tonal
    );
}

/// Log-frequency spectrogram, 20 Hz – 16 kHz, 70 dB range.
fn spectrogram(x: &[f64], rate: f64, path: &str) {
    let (w, h) = (900usize, 420usize);
    let n = 4096;
    let hop = (x.len().saturating_sub(n) / w).max(1);
    let mut img = vec![0u8; w * h * 3];
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(w);
    let mut peak = -1e9f64;
    for c in 0..w {
        let s = (c * hop).min(x.len().saturating_sub(n));
        let mut re: Vec<f64> = (0..n)
            .map(|i| {
                x.get(s + i).copied().unwrap_or(0.0)
                    * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
            })
            .collect();
        let mut im = vec![0.0; n];
        dsp::fft(&mut re, &mut im);
        let mag: Vec<f64> = (0..n / 2)
            .map(|k| 10.0 * (re[k] * re[k] + im[k] * im[k] + 1e-30).log10())
            .collect();
        let col: Vec<f64> = (0..h)
            .map(|r| {
                let f = 20.0 * (16_000.0f64 / 20.0).powf(1.0 - r as f64 / (h - 1) as f64);
                let b = f / rate * n as f64;
                let (i, fr) = (b.floor() as usize, b.fract());
                let i = i.min(n / 2 - 2);
                mag[i] * (1.0 - fr) + mag[i + 1] * fr
            })
            .collect();
        for &v in &col {
            peak = peak.max(v);
        }
        cols.push(col);
    }
    for (c, col) in cols.iter().enumerate() {
        for (r, &v) in col.iter().enumerate() {
            let t = ((v - peak + 70.0) / 70.0).clamp(0.0, 1.0);
            let (rr, gg, bb) = colour(t);
            let k = (r * w + c) * 3;
            img[k] = rr;
            img[k + 1] = gg;
            img[k + 2] = bb;
        }
    }
    // Octave lines at 100 Hz, 1 kHz, 10 kHz.
    for f in [100.0, 1000.0, 10_000.0] {
        let r = ((1.0 - (f / 20.0f64).ln() / (16_000.0f64 / 20.0).ln()) * (h - 1) as f64) as usize;
        for c in (0..w).step_by(4) {
            let k = (r * w + c) * 3;
            img[k] = 255;
            img[k + 1] = 255;
            img[k + 2] = 255;
        }
    }
    std::fs::write(path, png(w, h, &img)).unwrap();
}

fn colour(t: f64) -> (u8, u8, u8) {
    // Dark blue → purple → orange → yellow.
    let stops = [
        (0.0, [0.0, 0.0, 0.05]),
        (0.35, [0.3, 0.05, 0.45]),
        (0.65, [0.85, 0.25, 0.25]),
        (0.85, [0.98, 0.6, 0.1]),
        (1.0, [1.0, 1.0, 0.7]),
    ];
    for k in 0..stops.len() - 1 {
        let (a, ca) = stops[k];
        let (b, cb) = stops[k + 1];
        if t <= b {
            let f = (t - a) / (b - a);
            let c: Vec<u8> = (0..3)
                .map(|i| ((ca[i] + f * (cb[i] - ca[i])) * 255.0) as u8)
                .collect();
            return (c[0], c[1], c[2]);
        }
    }
    (255, 255, 180)
}

fn png(w: usize, h: usize, rgb: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for r in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgb[r * w * 3..(r + 1) * w * 3]);
    }
    // zlib, stored blocks.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
