//! How fast the sample engines simulate, per quality: simulated seconds per second
//! (above 1: real time), with their sound. `cargo bench -p open-racing-engine-sim`.

use criterion::{Criterion, criterion_group, criterion_main};
use open_racing_engine_sim::acoustics::{Acoustics, SoundSettings};
use open_racing_engine_sim::{Build, Controls, Load, Quality, samples};

fn engine(c: &mut Criterion) {
    let cases = [
        (
            "i4",
            samples::i4(),
            samples::i4_intake(),
            samples::i4_exhaust(),
        ),
        (
            "v8",
            samples::v8(),
            samples::v8_intake(),
            samples::v8_exhaust(),
        ),
    ];
    for (name, e, i, x) in cases {
        for q in [Quality::Draft, Quality::Normal] {
            let b = Build::new(&e)
                .system("intake", &i)
                .system("exhaust", &x)
                .quality(q);
            let (mut m, _) = b.build().unwrap();
            m.set_crank(0.0, 6000.0);
            let ctl = Controls {
                pedal: 1.0,
                load: Load::Speed(6000.0),
                ..Default::default()
            };
            let mut ac = Acoustics::new(&m, SoundSettings::default());
            let mut out = vec![Vec::new()];
            // 10 ms of engine per iteration.
            let steps = (q.rate() / 100) as usize;
            c.bench_function(&format!("{name} {q:?} 10 ms at 6000 rpm"), |bench| {
                bench.iter(|| {
                    for _ in 0..steps {
                        m.step(&ctl);
                        out[0].clear();
                        ac.process(&m, &mut out);
                    }
                })
            });
        }
    }
}

criterion_group!(benches, engine);
criterion_main!(benches);
