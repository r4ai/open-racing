//! The assembly workspace's panels: the machine's parts as a tree, the library to take
//! parts from, and the machine's figures and bake.

use bevy_egui::egui;
use open_racing_machine_project::machine::Placed;
use open_racing_machine_project::ops::Op;
use open_racing_machine_project::{Kind, PartRef, inspect};

use super::Ctx;
use super::plot::{RED, YELLOW};

/// The machine's parts, each under what it attaches to.
pub fn outliner(ui: &mut egui::Ui, c: &mut Ctx) {
    let names: Vec<String> = c.editor.lib.machines.keys().cloned().collect();
    let mut m = c.editor.selection.machine.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Machine");
        egui::ComboBox::from_id_salt("machine")
            .selected_text(&m)
            .show_ui(ui, |ui| {
                for n in &names {
                    ui.selectable_value(&mut m, n.clone(), n);
                }
            });
    });
    if c.editor.selection.machine.as_deref() != Some(m.as_str()) && !m.is_empty() {
        c.editor.selection.machine = Some(m.clone());
        c.editor.selection.placed = None;
    }
    let Some(machine) = c.editor.lib.machines.get(&m).cloned() else {
        ui.weak("No machine: File › Write Samples, or copy one with machinectl.");
        return;
    };
    ui.separator();
    fn tree(ui: &mut egui::Ui, c: &mut Ctx, parts: &[Placed], parent: Option<&str>, depth: usize) {
        for p in parts
            .iter()
            .filter(|p| p.attach.as_ref().map(|a| a.to.as_str()) == parent)
        {
            let sel = c.editor.selection.placed.as_deref() == Some(p.name.as_str());
            let icon = PartRef::parse(&p.part).map(|r| icon(r.kind)).unwrap_or("?");
            let label = format!("{}{icon} {}   {}", "   ".repeat(depth), p.name, p.part);
            let resp = ui.selectable_label(
                sel,
                egui::RichText::new(label).color(if sel {
                    egui::Color32::from_rgb(255, 160, 40)
                } else {
                    egui::Color32::from_gray(210)
                }),
            );
            if resp.clicked() {
                c.editor.selection.placed = Some(p.name.clone());
                c.editor.selection.part = Some(p.part.clone());
            }
            if depth < 12 {
                tree(ui, c, parts, Some(&p.name), depth + 1);
            }
        }
    }
    egui::ScrollArea::vertical()
        .id_salt("outliner")
        .show(ui, |ui| tree(ui, c, &machine.parts, None, 0));
    if let Some(p) = c.editor.selection.placed.clone() {
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Remove from machine").clicked() {
                c.editor.apply(
                    vec![Op::Unplace {
                        machine: m.clone(),
                        name: p.clone(),
                    }],
                    None,
                );
                c.editor.selection.placed = None;
            }
        });
    }
}

pub fn icon(k: Kind) -> &'static str {
    match k {
        Kind::Frame => "▦",
        Kind::Body => "◼",
        Kind::Aero => "✈",
        Kind::Engine => "⚙",
        Kind::Intake => "⇢",
        Kind::Exhaust => "⇠",
        Kind::Transmission => "⚙",
        Kind::Driveline => "↔",
        Kind::Suspension => "⌇",
        Kind::Wheel => "◎",
        Kind::Brakes => "◉",
        Kind::Steering => "☸",
        Kind::Interior => "▣",
        Kind::FuelTank => "⛽",
        Kind::Electronics => "⌁",
    }
}

/// The library: parts by kind, to inspect, swap into the machine, or place.
pub fn library(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut c.state.library_kind, None, "All");
        for k in Kind::ALL {
            ui.selectable_value(
                &mut c.state.library_kind,
                Some(k),
                format!("{} {}", icon(k), k.dir()),
            );
        }
    });
    ui.separator();
    let parts: Vec<PartRef> = c
        .editor
        .lib
        .parts
        .keys()
        .filter(|r| c.state.library_kind.is_none_or(|k| k == r.kind))
        .cloned()
        .collect();
    let machine = c.editor.selection.machine.clone();
    let placed = c.editor.selection.placed.clone();
    let placed_kind = machine
        .as_ref()
        .zip(placed.as_ref())
        .and_then(|(m, p)| c.editor.lib.machines.get(m)?.placed(p))
        .and_then(|p| PartRef::parse(&p.part).ok())
        .map(|r| r.kind);
    egui::ScrollArea::vertical()
        .id_salt("library")
        .show(ui, |ui| {
            for r in parts {
                let s = r.to_string();
                ui.horizontal(|ui| {
                    let sel = c.editor.selection.part.as_deref() == Some(s.as_str());
                    if ui
                        .selectable_label(sel, format!("{} {s}", icon(r.kind)))
                        .clicked()
                    {
                        c.editor.selection.part = Some(s.clone());
                        c.editor.selection.placed = None;
                    }
                    let desc = c
                        .editor
                        .lib
                        .parts
                        .get(&r)
                        .map(|p| p.description.clone())
                        .unwrap_or_default();
                    ui.weak(desc);
                    if let (Some(m), Some(p)) = (&machine, &placed)
                        && placed_kind == Some(r.kind)
                        && ui
                            .small_button("Swap in")
                            .on_hover_text(format!("Fit it in place of \"{p}\""))
                            .clicked()
                    {
                        c.editor.apply(
                            vec![Op::Set {
                                target: format!("machine/{m}"),
                                path: format!("parts[{p}].part"),
                                value: serde_json::json!(s),
                            }],
                            None,
                        );
                    }
                    if let Some(m) = &machine
                        && ui
                            .small_button("Place")
                            .on_hover_text("Add it to the machine (then attach it in Placement)")
                            .clicked()
                    {
                        let lib = &c.editor.lib;
                        let mut name = r.name.clone();
                        let mut k = 2;
                        while lib
                            .machines
                            .get(m)
                            .is_some_and(|x| x.placed(&name).is_some())
                        {
                            name = format!("{} {k}", r.name);
                            k += 1;
                        }
                        let axle = matches!(r.kind, Kind::Suspension | Kind::Wheel)
                            .then_some(open_racing_machine_project::machine::Axle::Front);
                        c.editor.apply(
                            vec![Op::Place {
                                machine: m.clone(),
                                placed: Placed {
                                    name: name.clone(),
                                    part: s.clone(),
                                    attach: None,
                                    at: [0.0; 3],
                                    rotation_deg: [0.0; 3],
                                    axle,
                                },
                            }],
                            None,
                        );
                        c.editor.selection.placed = Some(name);
                    }
                    if r.kind == Kind::Engine && ui.small_button("Develop").clicked() {
                        c.editor.selection.engine = Some(s.clone());
                        c.editor.workspace = crate::state::Workspace::Engine;
                    }
                });
            }
        });
}

/// The machine's figures, and baking it.
pub fn report(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(m) = c.editor.selection.machine.clone() else {
        return;
    };
    match inspect::machine(&c.editor.lib, &m) {
        Ok(s) => {
            let x = &s.mass;
            ui.label(format!(
                "{:.0} kg, {:.1} % on the front, CG {:.3} m high and {:.3} m behind the front axle",
                x.mass,
                x.front_weight * 100.0,
                s.cg_height,
                -x.centre[0]
            ));
            ui.label(format!(
                "wheelbase {:.3} m, tracks {:.3} / {:.3} m, inertia {:.0} / {:.0} / {:.0} kg·m² (roll, pitch, yaw), unsprung {:.1} / {:.1} kg a wheel",
                x.wheelbase, x.track[0], x.track[1], x.inertia[0], x.inertia[1], x.inertia[2], x.unsprung[0], x.unsprung[1]
            ));
            for w in &s.warnings {
                ui.colored_label(YELLOW, w);
            }
        }
        Err(e) => {
            ui.colored_label(RED, e);
        }
    }
    ui.separator();
    let busy = c.jobs.busy("bake");
    if ui
        .add_enabled(
            !busy,
            egui::Button::new(if busy {
                "Baking… (dyno sweep)"
            } else {
                "Bake for the game"
            }),
        )
        .clicked()
    {
        c.jobs.bake(c.editor.lib.clone(), m.clone());
    }
    if let Some((name, r)) = &c.jobs.bake {
        ui.label(format!(
            "{name}: {:.0} N·m at {:.0} rpm, {:.0} kW at {:.0} rpm",
            r.peak_torque.1,
            r.peak_torque.0,
            r.peak_power.1 / 1e3,
            r.peak_power.0
        ));
        if let Some(d) = &r.drive {
            let t = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.1} s"));
            ui.label(format!(
                "0-100 km/h {}, 0-200 km/h {}, {:.0} km/h after 60 s; drive it: cargo dev -- --car {name}",
                t(d.zero_100),
                t(d.zero_200),
                d.speed_60s
            ));
        }
    }
}
