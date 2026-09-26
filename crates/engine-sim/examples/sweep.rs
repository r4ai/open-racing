//! Full-load sweep of the sample inline four: `cargo run -p open-racing-engine-sim --example sweep`.
use open_racing_engine_sim::*;

fn main() {
    let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
    let q = if std::env::args().any(|a| a == "draft") {
        Quality::Draft
    } else {
        Quality::Normal
    };
    let b = Build::new(&e)
        .system("intake", &i)
        .system("exhaust", &x)
        .quality(q);
    let t = std::time::Instant::now();
    let run = dyno::sweep(&b, &dyno::speeds(1000.0, 7500.0, 500.0), 1.0).unwrap();
    println!("{:?}", t.elapsed());
    println!("  rpm   T Nm   P kW  bmep  pmep  fmep    VE  bsfc   pmax  @deg  knock  resid  drag");
    for (p, d) in run.points.iter().zip(&run.drag) {
        println!(
            "{:5.0} {:6.1} {:6.1} {:5.2} {:5.2} {:5.2} {:5.3} {:5.0} {:6.1} {:5.1} {:6.2} {:6.3} {:5.1} {}",
            p.rpm,
            p.brake_torque,
            p.power / 1e3,
            p.bmep / 1e5,
            p.pmep / 1e5,
            p.fmep / 1e5,
            p.volumetric_efficiency,
            p.bsfc,
            p.peak_pressure / 1e5,
            p.peak_deg,
            p.knock,
            p.residual,
            d.1,
            if p.converged { "" } else { "(not converged)" }
        );
    }
}
