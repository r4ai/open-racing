use open_racing_engine_sim::*;
fn main() {
    let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
    let b = Build::new(&e)
        .system("intake", &i)
        .system("exhaust", &x)
        .quality(Quality::Draft);
    let (mut m, _) = b.build().unwrap();
    m.set_crank(0.0, e.ecu.idle_rpm);
    let hold = Controls {
        load: Load::Speed(e.ecu.idle_rpm),
        ..Default::default()
    };
    let pl = m
        .lumps
        .iter()
        .position(|l| l.name.ends_with("plenum"))
        .unwrap();
    let mut next = 0.0;
    while m.time < 3.0 {
        let c = if m.time < 0.5 {
            hold
        } else {
            Controls::default()
        };
        m.step(&c);
        if m.time >= next {
            next += 0.1;
            let fuel: f64 = m.cylinders.iter().map(|c| c.last.fuel).sum();
            let work: f64 = m.cylinders.iter().map(|c| c.last.work).sum();
            println!(
                "t {:.1} rpm {:.0} thr {:.3} map {:.2} bar gas {:.1} fric {:.1} fuel {:.2e} work {:.1} peak {:.1}",
                m.time,
                m.rpm(),
                m.throttle,
                m.lumps[pl].p / 1e5,
                m.gas_torque,
                m.friction_torque,
                fuel,
                work,
                m.cylinders[0].last.peak_pressure / 1e5
            );
        }
    }
}
