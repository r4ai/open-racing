//! Renders a preset script of the sample inline four to a WAV file:
//! `cargo run -p open-racing-engine-sim --example sound -- i4|v8 sweep out.wav [draft|high]`.
use open_racing_engine_sim::*;

fn main() {
    let mut a = std::env::args().skip(1);
    let engine = a.next().unwrap_or("i4".into());
    let preset = a.next().unwrap_or("sweep".into());
    let out = a.next().unwrap_or("sound.wav".into());
    let q = match a.next().as_deref() {
        Some("draft") => Quality::Draft,
        Some("high") => Quality::High,
        _ => Quality::Normal,
    };
    let (e, i, x) = if engine == "v8" {
        (samples::v8(), samples::v8_intake(), samples::v8_exhaust())
    } else {
        (samples::i4(), samples::i4_intake(), samples::i4_exhaust())
    };
    let b = Build::new(&e)
        .system("intake", &i)
        .system("exhaust", &x)
        .quality(q);
    let script = Script::preset(&preset, &e).expect("preset");
    let t = std::time::Instant::now();
    let rec = render::render(&b, &script, None).unwrap();
    println!(
        "{:?} for {} s; peak {:.2} Pa",
        t.elapsed(),
        script.duration,
        rec.peak()
    );
    rec.write_wav(out.as_ref(), -1.0, None).unwrap();
    let rep = analysis::analyse(&rec, 8);
    for m in &rep.mics {
        println!(
            "{}: {:.1} dB, peaks {:?}",
            m.name,
            m.level_db,
            m.peaks
                .iter()
                .take(5)
                .map(|(f, d)| (f.round(), d.round()))
                .collect::<Vec<_>>()
        );
        for s in &m.order_track {
            let top: Vec<String> = s
                .orders_db
                .iter()
                .zip(&m.orders)
                .filter(|(d, _)| **d > s.level_db - 12.0)
                .map(|(d, o)| format!("{o}:{d:.0}"))
                .collect();
            println!(
                "  {:4.1}s {:5.0} rpm {:5.1} dB  {}",
                s.time,
                s.rpm,
                s.level_db,
                top.join(" ")
            );
        }
    }
}
