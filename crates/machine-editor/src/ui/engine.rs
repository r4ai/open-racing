//! The engine workbench: an engine on its bench (with the intake and exhaust it names) or
//! a machine's engine, on the dyno, cycle by cycle, as a gas network, running live with
//! its sound, and recorded to WAV.

use bevy_egui::egui;
use open_racing_engine_sim::dsp;
use open_racing_engine_sim::render::{PRESETS, Script};
use open_racing_engine_sim::{Build, Quality};
use open_racing_machine_project::part::Design;
use open_racing_machine_project::{Kind, engines, sound};

use super::plot::{self, BLUE, GREEN, GREY, Line, PURPLE, RED, YELLOW};
use super::{Ctx, EngineTab};

pub fn quality_combo(ui: &mut egui::Ui, id: &str, q: &mut Quality) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{q:?}"))
        .show_ui(ui, |ui| {
            for x in [
                Quality::Draft,
                Quality::Normal,
                Quality::High,
                Quality::Ultra,
            ] {
                ui.selectable_value(q, x, format!("{x:?}"));
            }
        });
}

/// The engine target's build at a quality.
pub fn build(c: &Ctx, q: Quality) -> Result<Build, String> {
    let t = c
        .editor
        .selection
        .engine
        .clone()
        .ok_or("no engine chosen")?;
    engines::for_target(&c.editor.lib, &t, q)
}

/// The list on the left: engines to develop, and what the bench fits.
pub fn targets(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.heading("Engines");
    let lib = &c.editor.lib;
    let mut pick = None;
    let current = c.editor.selection.engine.clone();
    for m in lib.machines.keys() {
        let t = format!("machine/{m}");
        if ui
            .selectable_label(current.as_deref() == Some(t.as_str()), format!("🏁 {m}"))
            .clicked()
        {
            pick = Some(t);
        }
    }
    for r in lib.parts.keys().filter(|r| r.kind == Kind::Engine) {
        let t = r.to_string();
        if ui
            .selectable_label(
                current.as_deref() == Some(t.as_str()),
                format!("⚙ {}", r.name),
            )
            .clicked()
        {
            pick = Some(t);
        }
    }
    if let Some(p) = pick {
        c.editor.selection.engine = Some(p);
        c.editor.selection.element = None;
    }
    ui.separator();
    let Some(target) = c.editor.selection.engine.clone() else {
        return;
    };
    if target.starts_with("machine/") {
        ui.label("The machine's own intake and exhaust.");
    } else if let Ok(p) = c.editor.lib.part(&target)
        && let Design::Engine(e) = &p.design
    {
        ui.label("On the bench with:");
        let mut ops = Vec::new();
        for (kind, cur, field) in [
            (Kind::Intake, e.bench.intake.clone(), "intake"),
            (Kind::Exhaust, e.bench.exhaust.clone(), "exhaust"),
        ] {
            let mut sel = cur.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt(field)
                .selected_text(if sel.is_empty() {
                    "open ports".to_string()
                } else {
                    sel.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sel, String::new(), "open ports");
                    for r in c.editor.lib.parts.keys().filter(|r| r.kind == kind) {
                        ui.selectable_value(&mut sel, r.to_string(), r.to_string());
                    }
                });
            if Some(sel.clone()) != cur && !(sel.is_empty() && cur.is_none()) {
                let value = if sel.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(sel)
                };
                ops.push(open_racing_machine_project::ops::Op::Set {
                    target: target.clone(),
                    path: format!("design.Engine.bench.{field}"),
                    value,
                });
            }
        }
        if !ops.is_empty() {
            c.editor.apply(ops, None);
        }
    }
    ui.separator();
    if let Ok(b) = build(c, Quality::Draft) {
        let s = open_racing_machine_project::inspect::engine(&b.engine);
        ui.label(format!(
            "{} cylinders, {:.2} l",
            s.cylinders, s.displacement
        ));
        ui.label(format!(
            "{:.1} × {:.1} mm, {:.1}:1",
            s.bore_stroke.0 * 1e3,
            s.bore_stroke.1 * 1e3,
            s.compression_ratio
        ));
        ui.label(format!("firing {:?}", s.firing_order));
        ui.label(format!(
            "intervals {}",
            s.firing_intervals
                .iter()
                .map(|v| format!("{v:.0}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        ui.label(format!(
            "IVO {:.0}° BTDC  IVC {:.0}° ABDC",
            s.intake_opens_btdc, s.intake_closes_abdc
        ));
        ui.label(format!(
            "EVO {:.0}° BBDC  EVC {:.0}° ATDC",
            s.exhaust_opens_bbdc, s.exhaust_closes_atdc
        ));
        ui.label(format!(
            "overlap {:.0}°, limiter {:.0} rpm",
            s.overlap, s.limiter_rpm
        ));
        match b.build() {
            Ok((m, w)) => {
                ui.label(format!(
                    "{} pipes, {} cells at Draft",
                    m.pipes.len(),
                    m.pipes.iter().map(|p| p.cells()).sum::<usize>()
                ));
                for x in w {
                    ui.colored_label(YELLOW, x);
                }
            }
            Err(e) => {
                ui.colored_label(RED, e);
            }
        }
    }
}

/// The workbench's middle.
pub fn bench(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.horizontal(|ui| {
        for (t, label) in [
            (EngineTab::Dyno, "Dyno"),
            (EngineTab::Cycle, "Cycle"),
            (EngineTab::Network, "Network"),
            (EngineTab::Run, "Run"),
            (EngineTab::Sound, "Sound"),
        ] {
            ui.selectable_value(&mut c.state.engine_tab, t, label);
        }
    });
    ui.separator();
    match c.state.engine_tab {
        EngineTab::Dyno => dyno(ui, c),
        EngineTab::Cycle => cycle(ui, c),
        EngineTab::Network => network(ui, c),
        EngineTab::Run => run(ui, c),
        EngineTab::Sound => sound_tab(ui, c),
    }
}

fn dyno(ui: &mut egui::Ui, c: &mut Ctx) {
    let s = &mut c.state;
    ui.horizontal(|ui| {
        ui.label("rpm");
        ui.add(
            egui::DragValue::new(&mut s.dyno_rpm[0])
                .speed(50.0)
                .range(300.0..=20000.0),
        );
        ui.label("to");
        ui.add(
            egui::DragValue::new(&mut s.dyno_rpm[1])
                .speed(50.0)
                .range(300.0..=20000.0),
        );
        ui.label("every");
        ui.add(
            egui::DragValue::new(&mut s.dyno_rpm[2])
                .speed(10.0)
                .range(50.0..=2000.0),
        );
        ui.label("throttle");
        ui.add(egui::Slider::new(&mut s.dyno_throttle, 0.0..=1.0));
        quality_combo(ui, "dyno quality", &mut s.dyno_quality);
    });
    let busy = c.jobs.busy("dyno");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(if busy { "Running…" } else { "Run dyno" }),
            )
            .clicked()
        {
            match build(c, c.state.dyno_quality) {
                Ok(b) => {
                    let r = c.state.dyno_rpm;
                    let rpms = open_racing_engine_sim::dyno::speeds(r[0], r[1], r[2]);
                    let t = c.editor.selection.engine.clone().unwrap_or_default();
                    c.jobs.dyno(t, b, rpms, c.state.dyno_throttle);
                }
                Err(e) => c.editor.status = e,
            }
        }
        if let Ok(b) = build(c, Quality::Draft)
            && ui.small_button("idle to limiter").clicked()
        {
            c.state.dyno_rpm = [
                (b.engine.ecu.idle_rpm - 200.0).max(800.0),
                b.engine.ecu.limiter_rpm,
                500.0,
            ];
        }
    });
    let Some((target, run)) = &c.jobs.dyno else {
        ui.weak("Run the dyno to see torque, power, losses and breathing across the rev range.");
        return;
    };
    ui.label(format!("{target} ({:?})", run.quality));
    let mut l1 = vec![
        Line::new(
            "torque N·m",
            RED,
            run.points.iter().map(|p| [p.rpm, p.brake_torque]).collect(),
        ),
        Line::new(
            "drag N·m",
            RED,
            run.drag.iter().map(|d| [d.0, d.1]).collect(),
        )
        .dashed(),
        Line::new(
            "power kW",
            BLUE,
            run.points.iter().map(|p| [p.rpm, p.power / 1e3]).collect(),
        )
        .right(),
    ];
    if let Some((_, prev)) = &c.jobs.previous_dyno {
        l1.push(
            Line::new(
                "before N·m",
                GREY,
                prev.points
                    .iter()
                    .map(|p| [p.rpm, p.brake_torque])
                    .collect(),
            )
            .dashed(),
        );
    }
    plot::plot(ui, "rpm", "N·m", Some("kW"), &l1, 230.0, true);
    let l2 = vec![
        Line::new(
            "VE",
            GREEN,
            run.points
                .iter()
                .map(|p| [p.rpm, p.volumetric_efficiency])
                .collect(),
        ),
        Line::new(
            "BSFC g/kWh",
            YELLOW,
            run.points.iter().map(|p| [p.rpm, p.bsfc]).collect(),
        )
        .right(),
    ];
    plot::plot(ui, "rpm", "VE", Some("g/kWh"), &l2, 170.0, false);
    let l3 = vec![
        Line::new(
            "bmep",
            RED,
            run.points.iter().map(|p| [p.rpm, p.bmep / 1e5]).collect(),
        ),
        Line::new(
            "pmep",
            BLUE,
            run.points.iter().map(|p| [p.rpm, p.pmep / 1e5]).collect(),
        ),
        Line::new(
            "fmep",
            GREY,
            run.points.iter().map(|p| [p.rpm, p.fmep / 1e5]).collect(),
        ),
        Line::new(
            "knock index",
            PURPLE,
            run.points.iter().map(|p| [p.rpm, p.knock]).collect(),
        )
        .right(),
    ];
    plot::plot(ui, "rpm", "bar", Some("knock"), &l3, 170.0, true);
    egui::CollapsingHeader::new("Points").show(ui, |ui| {
        egui::Grid::new("dyno points").striped(true).show(ui, |ui| {
            for h in [
                "rpm", "N·m", "kW", "bmep", "pmep", "fmep", "VE", "BSFC", "pmax bar", "at °",
                "knock", "resid", "",
            ] {
                ui.strong(h);
            }
            ui.end_row();
            for p in &run.points {
                for v in [
                    format!("{:.0}", p.rpm),
                    format!("{:.1}", p.brake_torque),
                    format!("{:.1}", p.power / 1e3),
                    format!("{:.2}", p.bmep / 1e5),
                    format!("{:.2}", p.pmep / 1e5),
                    format!("{:.2}", p.fmep / 1e5),
                    format!("{:.3}", p.volumetric_efficiency),
                    format!("{:.0}", p.bsfc),
                    format!("{:.1}", p.peak_pressure / 1e5),
                    format!("{:.0}", p.peak_deg),
                    format!("{:.2}", p.knock),
                    format!("{:.3}", p.residual),
                    if p.converged {
                        String::new()
                    } else {
                        "not settled".into()
                    },
                ] {
                    ui.label(v);
                }
                ui.end_row();
            }
        });
    });
}

fn cycle(ui: &mut egui::Ui, c: &mut Ctx) {
    let s = &mut c.state;
    ui.horizontal(|ui| {
        ui.label("rpm");
        ui.add(
            egui::DragValue::new(&mut s.trace_rpm)
                .speed(50.0)
                .range(300.0..=20000.0),
        );
        ui.label("throttle");
        ui.add(egui::Slider::new(&mut s.trace_throttle, 0.0..=1.0));
    });
    let busy = c.jobs.busy("trace");
    if ui
        .add_enabled(
            !busy,
            egui::Button::new(if busy {
                "Recording…"
            } else {
                "Record a cycle of cylinder 1"
            }),
        )
        .clicked()
    {
        match build(c, Quality::Normal) {
            Ok(b) => {
                let t = c.editor.selection.engine.clone().unwrap_or_default();
                c.jobs
                    .trace(t, b, c.state.trace_rpm, c.state.trace_throttle, vec![]);
            }
            Err(e) => c.editor.status = e,
        }
    }
    let Some((target, t)) = &c.jobs.trace else {
        ui.weak("Record a cycle to see the cylinder's pressure, the p–V loop, the valves and their flows.");
        return;
    };
    ui.label(format!("{target} at {:.0} rpm", t.rpm));
    let deg = &t.deg;
    let pv = |v: &Vec<f64>, k: f64| {
        deg.iter()
            .zip(v)
            .map(|(d, x)| [*d, x * k])
            .collect::<Vec<_>>()
    };
    plot::plot(
        ui,
        "° from firing TDC",
        "bar",
        Some("mm"),
        &[
            Line::new("pressure bar", RED, pv(&t.pressure, 1e-5)),
            Line::new("intake lift mm", BLUE, pv(&t.intake_lift, 1e3)).right(),
            Line::new("exhaust lift mm", GREEN, pv(&t.exhaust_lift, 1e3)).right(),
        ],
        220.0,
        true,
    );
    ui.columns(2, |cols| {
        let lv: Vec<[f64; 2]> = t
            .volume
            .iter()
            .zip(&t.pressure)
            .map(|(v, p)| [(v * 1e6).log10(), (p * 1e-5).log10()])
            .collect();
        // The p–V loop is not a function of V: draw it as two strokes.
        let (up, down): (Vec<_>, Vec<_>) = lv
            .iter()
            .enumerate()
            .partition(|(i, _)| deg[*i] < 180.0 || deg[*i] >= 540.0);
        let mut up: Vec<[f64; 2]> = up.into_iter().map(|x| *x.1).collect();
        let mut down: Vec<[f64; 2]> = down.into_iter().map(|x| *x.1).collect();
        up.sort_by(|a, b| a[0].total_cmp(&b[0]));
        down.sort_by(|a, b| a[0].total_cmp(&b[0]));
        plot::plot(
            &mut cols[0],
            "log V (cm³)",
            "log p (bar)",
            None,
            &[
                Line::new("compression, expansion", RED, up),
                Line::new("gas exchange", BLUE, down),
            ],
            200.0,
            false,
        );
        plot::plot(
            &mut cols[1],
            "°",
            "g/s",
            Some("K"),
            &[
                Line::new("intake flow g/s", BLUE, pv(&t.intake_flow, 1e3)),
                Line::new("exhaust flow g/s", GREEN, pv(&t.exhaust_flow, 1e3)),
                Line::new("temperature K", YELLOW, pv(&t.temperature, 1.0)).right(),
            ],
            200.0,
            true,
        );
    });
}

fn network(ui: &mut egui::Ui, c: &mut Ctx) {
    let Ok(b) = build(c, Quality::Draft) else {
        ui.weak("Choose an engine.");
        return;
    };
    // Which part each system comes from, to edit it.
    let target = c.editor.selection.engine.clone().unwrap_or_default();
    let owner = |sys: &str| -> String {
        if let Some(m) = target.strip_prefix("machine/") {
            c.editor
                .lib
                .machines
                .get(m)
                .and_then(|m| m.placed(sys))
                .map(|p| p.part.clone())
                .unwrap_or_default()
        } else {
            match c.editor.lib.part(&target).map(|p| &p.design) {
                Ok(Design::Engine(e)) if sys == "intake" => {
                    e.bench.intake.clone().unwrap_or_default()
                }
                Ok(Design::Engine(e)) if sys == "exhaust" => {
                    e.bench.exhaust.clone().unwrap_or_default()
                }
                _ => String::new(),
            }
        }
    };
    let sides: Vec<super::schematic::Side> = b
        .systems
        .iter()
        .map(|s| {
            let part = owner(&s.name);
            super::schematic::Side {
                left: part.starts_with("intake/"),
                part,
                network: &s.network,
            }
        })
        .collect();
    ui.weak("Click a pipe or volume to edit it on the right.");
    if let Some(sel) = super::schematic::show(
        ui,
        &sides,
        b.engine.layout.cylinders.len(),
        c.editor.selection.element.as_ref(),
    ) {
        c.editor.selection.element = Some(sel);
    }
}

fn run(ui: &mut egui::Ui, c: &mut Ctx) {
    let target = c.editor.selection.engine.clone().unwrap_or_default();
    let running = c.live.running();
    ui.horizontal(|ui| {
        if ui
            .button(if running {
                "⏹ Stop"
            } else {
                "▶ Start the engine"
            })
            .clicked()
        {
            if running {
                c.live.stop();
            } else {
                start_live(c);
            }
        }
        quality_combo(ui, "run quality", &mut c.state.run_quality);
        ui.label("volume");
        ui.add(egui::Slider::new(&mut c.live.volume, 0.0..=1.0));
        ui.checkbox(&mut c.live.muted, "mute");
    });
    if let Some(e) = &c.live.error {
        ui.colored_label(RED, e);
    }
    ram_preview(ui, c);
    let Some(engine) = &c.live.engine else {
        ui.weak("Start the engine to rev it with the pedal (W or the Up arrow held, or the slider) and hear it. Edits restart it. Where it cannot run in real time, render a RAM preview above.");
        return;
    };
    let ctl = &engine.controls;
    let mut pedal = ctl.pedal.get() as f32;
    let keys = ui.input(|i| i.key_down(egui::Key::W) || i.key_down(egui::Key::ArrowUp));
    ui.horizontal(|ui| {
        ui.label("pedal");
        ui.add(egui::Slider::new(&mut pedal, 0.0..=1.0));
    });
    ctl.pedal.set(if keys { 1.0 } else { pedal as f64 });
    let mut hold = ctl.hold_rpm.get() as f32;
    let mut inertia = ctl.load_inertia.get() as f32;
    ui.horizontal(|ui| {
        ui.label("dyno holds");
        ui.add(egui::Slider::new(&mut hold, 0.0..=12000.0).suffix(" rpm"));
        ui.weak("(0: free)");
        ui.label("added inertia");
        ui.add(egui::Slider::new(&mut inertia, 0.0..=5.0).suffix(" kg·m²"));
    });
    ctl.hold_rpm.set(hold as f64);
    ctl.load_inertia.set(inertia as f64);
    let t = &c.live.telemetry;
    ui.horizontal(|ui| {
        ui.heading(format!("{:5.0} rpm", t.rpm));
        ui.label(format!(
            "{:6.1} N·m   manifold {:.2} bar   throttle {:.0} %   {:.2}× real time   {} under-runs",
            t.torque,
            t.manifold_pressure / 1e5,
            t.throttle * 100.0,
            t.realtime_factor,
            ctl.underruns.load(std::sync::atomic::Ordering::Relaxed)
        ));
    });
    if t.realtime_factor > 0.0 && t.realtime_factor < 0.98 {
        ui.colored_label(
            YELLOW,
            "The engine runs slower than real time: choose Draft, or a faster machine.",
        );
    }
    let scope: Vec<[f64; 2]> = t
        .scope
        .iter()
        .enumerate()
        .map(|(i, v)| [i as f64 / 48.0, *v as f64])
        .collect();
    plot::plot(
        ui,
        "ms",
        "Pa",
        None,
        &[Line::new("microphone", GREEN, scope)],
        140.0,
        true,
    );
    ui.columns(2, |cols| {
        let x: Vec<f64> = t.scope.iter().map(|v| *v as f64).collect();
        let sp: Vec<[f64; 2]> = dsp::spectrum(&x, 48_000.0)
            .into_iter()
            .filter(|(f, _)| *f > 20.0 && *f < 5000.0)
            .map(|(f, a)| [f, dsp::spl(a / std::f64::consts::SQRT_2)])
            .collect();
        plot::plot(
            &mut cols[0],
            "Hz",
            "dB",
            None,
            &[Line::new("spectrum", BLUE, sp)],
            160.0,
            false,
        );
        let pv: Vec<[f64; 2]> = t
            .cylinder
            .iter()
            .map(|(_, p, v)| [v * 1e6, p / 1e5])
            .collect();
        let pt: Vec<[f64; 2]> = t.cylinder.iter().map(|(d, p, _)| [*d, p / 1e5]).collect();
        let _ = pv;
        plot::plot(
            &mut cols[1],
            "° from firing TDC",
            "bar",
            None,
            &[Line::new("cylinder 1", RED, pt)],
            160.0,
            true,
        );
    });
    let _ = target;
    ui.ctx().request_repaint();
}

/// Starts the live engine on the chosen target.
pub fn start_live(c: &mut Ctx) {
    let target = c.editor.selection.engine.clone().unwrap_or_default();
    match build(c, c.state.run_quality) {
        Ok(b) => {
            let machine = target.strip_prefix("machine/");
            let settings = b.build().and_then(|(m, _)| {
                sound::mic_settings(&c.editor.lib, machine, &m, &c.state.run_mics)
            });
            match settings {
                Ok(s) => c.live.start(target, c.editor.revision, b, s),
                Err(e) => c.live.error = Some(e),
            }
        }
        Err(e) => c.live.error = Some(e),
    }
}

fn sound_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let s = &mut c.state;
    ui.horizontal(|ui| {
        ui.label("script");
        egui::ComboBox::from_id_salt("preset")
            .selected_text(&s.sound_preset)
            .show_ui(ui, |ui| {
                for p in PRESETS {
                    ui.selectable_value(&mut s.sound_preset, p.to_string(), p);
                }
            });
        ui.label("microphones");
        for m in sound::MICS {
            let mut on = s.sound_mics.iter().any(|x| x == m);
            if ui.checkbox(&mut on, m).changed() {
                if on {
                    s.sound_mics.push(m.to_string());
                } else {
                    s.sound_mics.retain(|x| x != m);
                }
            }
        }
        quality_combo(ui, "sound quality", &mut s.sound_quality);
    });
    let busy = c.jobs.busy("sound");
    if ui
        .add_enabled(
            !busy && !c.state.sound_mics.is_empty(),
            egui::Button::new(if busy { "Rendering…" } else { "Render WAV" }),
        )
        .clicked()
    {
        let target = c.editor.selection.engine.clone().unwrap_or_default();
        let r = build(c, c.state.sound_quality).and_then(|b| {
            let script =
                Script::preset(&c.state.sound_preset, &b.engine).ok_or("no such script")?;
            let (m, _) = b.build()?;
            let settings = sound::mic_settings(
                &c.editor.lib,
                target.strip_prefix("machine/"),
                &m,
                &c.state.sound_mics,
            )?;
            Ok((b, script, settings))
        });
        match r {
            Ok((b, script, settings)) => {
                let dir = c.editor.lib.dir.join("renders");
                let _ = std::fs::create_dir_all(&dir);
                let file = format!("{}-{}.wav", target.replace('/', "-"), c.state.sound_preset);
                c.jobs.sound(target, b, script, settings, dir.join(file));
            }
            Err(e) => c.editor.status = e,
        }
    }
    let Some((target, path, report)) = &c.jobs.sound else {
        ui.weak("Render a script to a WAV file (one channel per microphone) and see its levels and engine orders.");
        return;
    };
    ui.label(format!("{target}: {}", path.display()));
    for m in &report.mics {
        ui.label(format!(
            "{}: {:.1} dB overall; peaks {}",
            m.name,
            m.level_db,
            m.peaks
                .iter()
                .take(4)
                .map(|(f, d)| format!("{f:.0} Hz {d:.0} dB"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(m) = report.mics.first() {
        let rising = m.order_track.windows(2).all(|w| w[1].rpm >= w[0].rpm - 1.0);
        let x =
            |s: &open_racing_engine_sim::analysis::OrderSlice| if rising { s.rpm } else { s.time };
        let colours = [RED, BLUE, GREEN, PURPLE, YELLOW];
        let mut idx: Vec<usize> = (0..m.orders.len()).collect();
        let level = |k: usize| m.order_track.iter().map(|s| s.orders_db[k]).sum::<f64>();
        idx.sort_by(|a, b| level(*b).total_cmp(&level(*a)));
        let mut lines: Vec<Line> = idx
            .iter()
            .take(5)
            .enumerate()
            .map(|(n, &k)| {
                Line::new(
                    &format!("order {}", m.orders[k]),
                    colours[n],
                    m.order_track
                        .iter()
                        .map(|s| [x(s), s.orders_db[k]])
                        .collect(),
                )
            })
            .collect();
        lines.push(
            Line::new(
                "overall",
                GREY,
                m.order_track.iter().map(|s| [x(s), s.level_db]).collect(),
            )
            .dashed(),
        );
        plot::plot(
            ui,
            if rising { "rpm" } else { "s" },
            "dB",
            None,
            &lines,
            220.0,
            false,
        );
    }
}

/// A RAM preview: a script, or the last live take, rendered into memory at any quality
/// and played from there — for when the machine cannot simulate it in real time.
fn ram_preview(ui: &mut egui::Ui, c: &mut Ctx) {
    egui::CollapsingHeader::new("RAM preview")
        .default_open(c.live.preview.is_some() || (c.live.running() && c.live.telemetry.realtime_factor > 0.0 && c.live.telemetry.realtime_factor < 0.98))
        .show(ui, |ui| {
            let take = c.live.take_script();
            ui.horizontal(|ui| {
                ui.label("run");
                let mut options: Vec<String> = PRESETS.iter().map(|p| p.to_string()).collect();
                if let Some(t) = &take {
                    options.insert(0, format!("last take ({:.1} s)", t.duration));
                }
                if !options.contains(&c.state.preview_source) {
                    c.state.preview_source = options[0].clone();
                }
                egui::ComboBox::from_id_salt("preview source").selected_text(&c.state.preview_source).show_ui(ui, |ui| {
                    for o in &options {
                        ui.selectable_value(&mut c.state.preview_source, o.clone(), o);
                    }
                });
                quality_combo(ui, "preview quality", &mut c.state.preview_quality);
                ui.checkbox(&mut c.state.preview_loop, "loop");
                if ui.button("⏺ Render and play").clicked() {
                    start_preview(c, take.clone());
                }
            });
            let Some(p) = c.live.preview.clone() else {
                ui.weak("Renders the run in the background (at any quality, however slow), then plays it from memory. A live run records the pedal as a take to render again finer.");
                return;
            };
            use std::sync::atomic::Ordering::Relaxed;
            p.looping.store(c.state.preview_loop, Relaxed);
            let rate = open_racing_engine_sim::dsp::OUTPUT_RATE as f64;
            let secs = p.total as f64 / rate;
            ui.label(format!(
                "{}: {:.1} of {:.1} s cached, rendering at {:.2}× real time{}",
                p.label,
                p.rendered.load(Relaxed) as f64 / rate,
                secs,
                p.speed(),
                if p.done.load(Relaxed) { "" } else { "…" }
            ));
            // The cache bar (green: rendered) with the playhead, as in a video editor; a
            // click moves the playhead.
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), egui::Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, egui::Color32::from_gray(35));
            let mut cached = rect;
            cached.set_width(rect.width() * p.fraction() as f32);
            painter.rect_filled(cached, 2.0, egui::Color32::from_rgb(60, 140, 70));
            let head = p.playhead.load(Relaxed) as f32 / p.total.max(1) as f32;
            let x = rect.left() + head * rect.width();
            painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 160, 255)));
            if let Some(pos) = resp.interact_pointer_pos() {
                let f = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                p.playhead.store((f as f64 * p.total as f64) as usize, Relaxed);
            }
            ui.horizontal(|ui| {
                let playing = p.playing.load(Relaxed);
                if ui.button(if playing { "⏸ Pause" } else { "▶ Play what is cached" }).clicked() {
                    if !playing && p.playhead.load(Relaxed) >= p.rendered.load(Relaxed) {
                        p.playhead.store(0, Relaxed);
                    }
                    p.playing.store(!playing, Relaxed);
                }
                if ui.button("⏹ Stop").clicked() {
                    p.playing.store(false, Relaxed);
                    p.playhead.store(0, Relaxed);
                }
                if ui.button("✕ Discard").clicked() {
                    c.live.stop_preview();
                }
            });
            if let Ok(e) = p.error.lock()
                && let Some(e) = e.as_ref()
            {
                ui.colored_label(RED, e);
            }
            if let Ok(rpm) = p.rpm.lock()
                && !rpm.is_empty()
            {
                let pts: Vec<[f64; 2]> = rpm.iter().map(|k| [k.0, k.1]).collect();
                plot::plot(ui, "s", "rpm", None, &[Line::new("engine speed", RED, pts)], 110.0, true);
            }
            ui.ctx().request_repaint();
        });
}

/// Starts a RAM preview of the chosen run.
pub fn start_preview(c: &mut Ctx, take: Option<Script>) {
    let target = c.editor.selection.engine.clone().unwrap_or_default();
    let r = build(c, c.state.preview_quality).and_then(|b| {
        let script = if c.state.preview_source.starts_with("last take") {
            take.ok_or("no take yet")?
        } else {
            Script::preset(&c.state.preview_source, &b.engine).ok_or("no such script")?
        };
        let (m, _) = b.build()?;
        let settings = sound::mic_settings(
            &c.editor.lib,
            target.strip_prefix("machine/"),
            &m,
            &c.state.run_mics,
        )?;
        Ok((b, script, settings))
    });
    match r {
        Ok((b, script, settings)) => {
            let label = format!(
                "{target}, {} at {:?}",
                c.state.preview_source, c.state.preview_quality
            );
            c.live
                .start_preview(label, b, script, settings, c.state.preview_loop);
        }
        Err(e) => c.live.error = Some(e),
    }
}
