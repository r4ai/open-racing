//! Properties from a part's or machine's JSON form: every field is shown and editable,
//! numbers by dragging or typing, and each change becomes the same `Set` operation an
//! agent sends (`target`, `path`, `value`). What a part holds is what the panel shows,
//! however deep: an engine's cam, a pipe's diameters, a tyre's thermal model.

use bevy_egui::egui;
use serde_json::Value;

/// A change the panel made: (path, new value).
pub type Change = (String, Value);

/// The unit a field is in, from its name.
pub fn unit(key: &str) -> &'static str {
    if key.ends_with("_deg") {
        return "°";
    }
    if key.ends_with("_rpm") {
        return "rpm";
    }
    match key {
        "bore" | "stroke" | "rod" | "pin_offset" | "length" | "lift" | "radius" | "width"
        | "track" | "stem" | "ride_height" | "bump_travel" | "droop_travel" | "diameter"
        | "shaft" | "peak_slip_length" | "relaxation_x" | "relaxation_y" | "pneumatic_trail"
        | "wheelbase" => "m",
        "mass" | "reciprocating_mass" | "rotating_mass" | "unsprung_mass" => "kg",
        "volume" => "m³",
        "wall_temperature" | "piston" | "head" | "liner" => "K",
        "pressure" => "bar",
        "spring_rate" | "anti_roll_rate" | "bump_stop_rate" | "vertical_stiffness" | "rate" => {
            "N/m"
        }
        "bump_damping"
        | "rebound_damping"
        | "fast_bump_damping"
        | "fast_rebound_damping"
        | "damping" => "N·s/m",
        "inertia" | "flywheel_inertia" | "crank_inertia" | "wheel_inertia" => "kg·m²",
        "capacity" => "l",
        "max_torque" | "preload" | "creep_torque" | "dog_release_torque" => "N·m",
        "camber" | "toe" | "angle" | "lock" => "rad",
        "constant" | "piston_speed" | "piston_speed_sq" | "accessories" => "Pa",
        "lhv" => "J/kg",
        _ => "",
    }
}

fn speed(v: f64) -> f64 {
    (v.abs() * 0.004).max(1e-4)
}

/// A number field; returns the new value when changed.
fn number(ui: &mut egui::Ui, v: f64, u: &str) -> Option<f64> {
    number_of(ui, v, u, false)
}

fn number_of(ui: &mut egui::Ui, v: f64, u: &str, integer: bool) -> Option<f64> {
    let mut x = v;
    let decimals = if integer {
        0
    } else if v != 0.0 && v.abs() < 0.01 {
        6
    } else if v.abs() < 10.0 {
        4
    } else {
        1
    };
    let r = ui.add(
        egui::DragValue::new(&mut x)
            .speed(speed(v))
            .max_decimals(decimals)
            .suffix(if u.is_empty() {
                String::new()
            } else {
                format!(" {u}")
            }),
    );
    (r.changed() && x != v && x.is_finite()).then_some(x)
}

fn is_table(a: &[Value]) -> bool {
    !a.is_empty()
        && a.iter()
            .all(|e| matches!(e, Value::Array(p) if p.len() == 2 && p.iter().all(Value::is_number)))
}

fn is_vector(a: &[Value]) -> bool {
    !a.is_empty() && a.len() <= 4 && a.iter().all(Value::is_number)
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else if key.starts_with('[') {
        format!("{path}{key}")
    } else {
        format!("{path}.{key}")
    }
}

/// Shows `v` (at `path`) and collects changes. `depth` opens the first levels.
pub fn value(
    ui: &mut egui::Ui,
    key: &str,
    path: &str,
    v: &Value,
    depth: usize,
    out: &mut Vec<Change>,
) {
    match v {
        Value::Object(m) => {
            let show = |ui: &mut egui::Ui, out: &mut Vec<Change>| {
                for (k, x) in m {
                    value(ui, k, &join(path, k), x, depth + 1, out);
                }
            };
            if path.is_empty() {
                show(ui, out);
            } else {
                egui::CollapsingHeader::new(pretty(key))
                    .id_salt(path)
                    .default_open(depth < 2)
                    .show(ui, |ui| show(ui, out));
            }
        }
        Value::Array(a) if is_vector(a) => {
            ui.horizontal(|ui| {
                label(ui, key);
                for (i, e) in a.iter().enumerate() {
                    if let Some(x) = number(ui, e.as_f64().unwrap_or(0.0), "") {
                        let mut n = a.clone();
                        n[i] = serde_json::json!(x);
                        out.push((path.to_string(), Value::Array(n)));
                    }
                }
                let u = unit(key);
                if !u.is_empty() {
                    ui.weak(u);
                }
            });
        }
        Value::Array(a) if is_table(a) => {
            egui::CollapsingHeader::new(format!("{} ({} points)", pretty(key), a.len()))
                .id_salt(path)
                .show(ui, |ui| {
                    let mut rows: Vec<[f64; 2]> = a
                        .iter()
                        .map(|p| [p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)])
                        .collect();
                    let mut changed = false;
                    let mut remove = None;
                    for (i, r) in rows.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            for c in r.iter_mut() {
                                if let Some(x) = number(ui, *c, "") {
                                    *c = x;
                                    changed = true;
                                }
                            }
                            if ui
                                .small_button("✕")
                                .on_hover_text("Remove this point")
                                .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        rows.remove(i);
                        changed = true;
                    }
                    if ui.small_button("+ point").clicked() {
                        let last = rows.last().copied().unwrap_or([0.0, 0.0]);
                        rows.push([last[0] + 1.0, last[1]]);
                        changed = true;
                    }
                    if changed {
                        out.push((path.to_string(), serde_json::json!(rows)));
                    }
                });
        }
        Value::Array(a) => {
            egui::CollapsingHeader::new(format!("{} ({})", pretty(key), a.len()))
                .id_salt(path)
                .default_open(depth < 1)
                .show(ui, |ui| {
                    for (i, e) in a.iter().enumerate() {
                        let name = e.get("name").and_then(Value::as_str);
                        let (k, p) = match name {
                            Some(n) => (n.to_string(), format!("{path}[{n}]")),
                            None => (format!("{i}"), join(path, &i.to_string())),
                        };
                        value(ui, &k, &p, e, depth + 1, out);
                    }
                });
        }
        Value::Number(n) => {
            ui.horizontal(|ui| {
                label(ui, key);
                let integer = n.is_u64() || n.is_i64();
                if let Some(x) = number_of(ui, n.as_f64().unwrap_or(0.0), unit(key), integer) {
                    let v = if n.is_u64() || n.is_i64() {
                        serde_json::json!(x.round() as i64)
                    } else {
                        serde_json::json!(x)
                    };
                    out.push((path.to_string(), v));
                }
            });
        }
        Value::Bool(b) => {
            let mut x = *b;
            if ui.checkbox(&mut x, pretty(key)).changed() {
                out.push((path.to_string(), Value::Bool(x)));
            }
        }
        Value::String(s) => {
            ui.horizontal(|ui| {
                label(ui, key);
                let mut x = s.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut x).desired_width(180.0));
                if r.lost_focus() && x != *s {
                    out.push((path.to_string(), Value::String(x)));
                }
            });
        }
        Value::Null => {
            ui.horizontal(|ui| {
                label(ui, key);
                ui.weak("none");
            });
        }
    }
}

fn label(ui: &mut egui::Ui, key: &str) {
    ui.add_sized([140.0, 18.0], egui::Label::new(pretty(key)).truncate());
}

/// `centreline_deg` → "centreline".
pub fn pretty(key: &str) -> String {
    let k = key.trim_end_matches("_deg").trim_end_matches("_rpm");
    k.replace('_', " ")
}
