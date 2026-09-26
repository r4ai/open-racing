//! Runs the sample inline four at a speed and prints each cylinder's last cycle:
//! `cargo run -p open-racing-engine-sim --example probe -- 3000 draft`.
use open_racing_engine_sim::*;

fn main() {
    let e = samples::i4();
    let (i, x) = (samples::i4_intake(), samples::i4_exhaust());
    let mut args = std::env::args().skip(1);
    let rpm: f64 = args.next().map(|s| s.parse().unwrap()).unwrap_or(3000.0);
    let q = match args.next().as_deref() {
        Some("draft") => Quality::Draft,
        Some("high") => Quality::High,
        _ => Quality::Normal,
    };
    let (mut m, _) = Build::new(&e)
        .system("intake", &i)
        .system("exhaust", &x)
        .quality(q)
        .build()
        .unwrap();
    let cells: usize = m.pipes.iter().map(|p| p.cells()).sum();
    m.set_crank(0.0, rpm);
    let c = Controls {
        pedal: 1.0,
        load: Load::Speed(rpm),
        ..Default::default()
    };
    let t0 = std::time::Instant::now();
    while m.time < 12.0 * 120.0 / rpm {
        m.step(&c);
    }
    let el = t0.elapsed().as_secs_f64();
    println!(
        "{cells} cells, {:.2}× real time, {} split steps",
        m.time / el,
        m.split_steps
    );
    let vd = m.kinematics.swept();
    let rho = 101_325.0 / (287.05 * 298.15);
    for (n, c) in m.cylinders.iter().enumerate() {
        let s = &c.last;
        println!(
            "cyl {} imep {:.2} bar pmep {:.2} VE {:.3} pmax {:.1} bar at {:.0}° knock {:.2} residual {:.3}",
            n + 1,
            s.work / vd / 1e5,
            s.pump_work / vd / 1e5,
            s.intake_mass / (rho * vd),
            s.peak_pressure / 1e5,
            s.peak_deg,
            s.knock,
            s.residual
        );
    }
}
