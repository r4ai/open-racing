//! A road's tabs: the road itself, and the strips, painted lines and barriers along it.
//! What was laid round corners is worked on in the Corners tab and only counted here.

use super::*;

pub(super) fn road_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
    let Some((r, road)) = c.editor.road() else {
        return;
    };
    let road = road.clone();
    let name = road.name.clone();
    let (surfaces, materials) = names(&c.editor.project);
    let period = road.period();

    section(ui, "Road", ("road", r), true, |ui| {
        name_row(ui, c, state, Item::Road(r));
        if name == c.editor.project.main_road {
            row(ui, "", |ui| ui.label("★ The main road: the circuit"));
        } else {
            row(ui, "", |ui| commands::button(ui, c, Cmd::SetMain));
        }
        let length = c.built.roads.get(r).map_or(0.0, |s| s.length);
        row(ui, "Length", |ui| {
            ui.label(format!("{length:.0} m, {} nodes", road.nodes.len()))
        });
        let (mut closed, mut crown, mut resolution) = (road.closed, road.crown, road.resolution);
        let (mut surface, mut material) = (road.surface.clone(), road.material.clone());
        let mut changed = check(ui, &mut closed, "Closed loop");
        changed |= drag(ui, "Crown m", &mut crown, 0.005, -0.5..=0.5);
        changed |= drag(ui, "Resolution m", &mut resolution, 0.1, 0.25..=10.0);
        changed |= combo_row(ui, "Surface", ("road surface", r), &mut surface, &surfaces);
        changed |= combo_row(
            ui,
            "Material",
            ("road material", r),
            &mut material,
            &materials,
        );
        if changed {
            c.editor.apply(
                vec![Op::SetRoad {
                    road: name.clone(),
                    closed: Some(closed),
                    crown: Some(crown),
                    surface: Some(surface),
                    material: Some(material),
                    resolution: Some(resolution),
                }],
                Some(&format!("road props {name}")),
            );
        }
    });

    // The keys themselves, for exact values: widths and bank are shaped in the view
    // (Alt S, Ctrl T, dragging the edges), in the sidebar (N) and in the Curves graph.
    section(ui, "Width & bank keys", ("keys", r), false, |ui| {
        ui.weak("Exact values at node numbers (u). Shape them more easily in the view (drag a selected node's edges, Alt S, Ctrl T), the sidebar (N) or the Curves graph below.");
        let id = ui.make_persistent_id(("key curve", r));
        let mut which: usize = ui.data(|d| d.get_temp(id)).unwrap_or(0);
        row(ui, "Curve", |ui| {
            for (i, label) in ["Left width", "Right width", "Bank"].iter().enumerate() {
                ui.selectable_value(&mut which, i, *label);
            }
        });
        ui.data_mut(|d| d.insert_temp(id, which));
        let (curve, title, scale) = [
            (Curve::WidthLeft, "left width", 1.0),
            (Curve::WidthRight, "right width", 1.0),
            (Curve::Bank, "bank", 180.0 / std::f64::consts::PI),
        ][which];
        let cv = edit::profile(&road, curve);
        let mut keys = cv.keys.clone();
        let mut changed = false;
        let mut remove = None;
        // A road is never narrower than 10 cm a side.
        let least = if curve == Curve::Bank { f64::MIN } else { 0.1 };
        for i in 0..keys.len() {
            // Each key stays between its neighbours, so that the list keeps its order
            // (and the field its key) while a value is dragged; a closed road's last
            // key stays short of its end, which is its start.
            let lo = if i > 0 { keys[i - 1].u + 1e-3 } else { 0.0 };
            let hi = match keys.get(i + 1) {
                Some(next) => next.u - 1e-3,
                None if road.closed => period - 1e-3,
                None => period,
            };
            let k = &mut keys[i];
            row(ui, &format!("Key {i}"), |ui| {
                ui.label("u");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut k.u)
                            .speed(0.01)
                            .range(lo..=hi.max(lo)),
                    )
                    .changed();
                let mut v = k.value * scale;
                let unit = if scale == 1.0 { " m" } else { "°" };
                if ui
                    .add(egui::DragValue::new(&mut v).speed(0.05).suffix(unit))
                    .changed()
                {
                    k.value = (v / scale).max(least);
                    changed = true;
                }
                if cv.keys.len() > 1 && ui.small_button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            keys.remove(i);
            changed = true;
        }
        let n = c.editor.selection.node().filter(|&n| n < road.nodes.len());
        if let Some(n) = n
            && !keys.iter().any(|k| (k.u - n as f64).abs() < 1e-3)
            && ui.small_button(format!("+ Key at node {n}")).clicked()
        {
            let v = cv.eval(n as f64, period, road.closed);
            keys.push(Key::new(n as f64, v));
            changed = true;
        }
        if changed {
            c.editor.apply(
                vec![Op::SetProfile {
                    road: name.clone(),
                    curve,
                    keys,
                }],
                Some(&format!("profile {name} {title}")),
            );
        }
    });
}

/// "12 more laid round corners", with a button to the Corners tab.
fn cornered(ui: &mut egui::Ui, c: &mut Ctx, count: usize, what: &str) {
    if count == 0 {
        return;
    }
    ui.horizontal(|ui| {
        ui.weak(format!("{count} {what} laid round corners"));
        if ui
            .small_button("↩ Corners")
            .on_hover_text("Kerbs, gravel and walls round each corner are set in the Corners tab")
            .clicked()
        {
            c.shell.tab = PropTab::Corners;
        }
    });
}

pub(super) fn strips_tab(ui: &mut egui::Ui, c: &mut Ctx, library: &Library) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (surfaces, materials) = names(&c.editor.project);
    let (strip_styles, _) = style_names(&c.editor.project);
    let node = c.editor.selection.node();
    let picked = c.editor.selection.nodes.clone();
    let period = road.period();
    ui.weak("Bands beside the road, from its edge outwards: kerbs, run-off, gravel, verges.");
    for side in [Side::Left, Side::Right] {
        let title = match side {
            Side::Left => "Left side",
            Side::Right => "Right side",
        };
        section(ui, title, ("strips", r, side as u8), true, |ui| {
            let strips = road.strips(side).clone();
            cornered(
                ui,
                c,
                strips
                    .iter()
                    .filter(|s| crate::corners::held(c.built, r, &s.corner))
                    .count(),
                "more",
            );
            for (i, strip) in strips.iter().enumerate() {
                if crate::corners::held(c.built, r, &strip.corner) {
                    continue;
                }
                let mut s = strip.clone();
                let (mut changed, mut removed) = (false, false);
                let open = focused(c, Focus::Strip(side, i));
                let kind = s.style.as_deref().unwrap_or("custom");
                let wide = if s.keys.is_empty() {
                    format!("{:.1} m", s.width)
                } else {
                    let narrowest = s.keys.iter().map(|k| k.width).fold(f64::INFINITY, f64::min);
                    format!("{narrowest:.1}–{:.1} m, {} nodes", s.widest(), s.keys.len())
                };
                let mut add_key = false;
                let resp =
                    egui::CollapsingHeader::new(format!("{}  ·  {kind}, {wide}", s.name))
                        .id_salt(("strip", r, side as u8, i))
                        .open(open)
                        .show(ui, |ui| {
                            let mut style = s.style.clone();
                            if row(ui, "Type", |ui| {
                                style_combo(
                                    ui,
                                    ("stype", r, side as u8, i),
                                    &mut style,
                                    &strip_styles,
                                    "custom",
                                )
                            }) {
                                match style
                                    .as_deref()
                                    .and_then(|n| c.editor.project.strip_style(n))
                                {
                                    Some(t) => t.restyle(&mut s),
                                    None => s.style = None,
                                }
                                changed = true;
                            }
                            let mut width = s.width;
                            if drag(ui, "Width m", &mut width, 0.05, 0.0..=200.0) {
                                // Its nodes each wider or narrower alike.
                                s.set_width(width);
                                changed = true;
                            }
                            changed |= strip_keys_ui(ui, &mut s, period);
                            add_key = row(ui, "", |ui| {
                                ui.small_button(match node {
                                    Some(n) => format!("+ Node at node {n}"),
                                    None => "+ Node halfway along it".to_string(),
                                })
                                .on_hover_text("A node to make it wider, narrower, higher or lower there; drag it in the view")
                                .clicked()
                            });
                            // Its own look: changing any of it leaves its type.
                            let look = (
                                s.surface.clone(),
                                s.material.clone(),
                                s.profile.clone(),
                                s.fade,
                                s.model.clone(),
                            );
                            combo_row(
                                ui,
                                "Surface",
                                ("ss", r, side as u8, i),
                                &mut s.surface,
                                &surfaces,
                            );
                            combo_row(
                                ui,
                                "Material",
                                ("sm", r, side as u8, i),
                                &mut s.material,
                                &materials,
                            );
                            profile_ui(ui, &mut s.profile, (r, side as u8, i));
                            drag(ui, "Fade m", &mut s.fade, 0.1, 0.0..=100.0);
                            model_ui(ui, ("strip model", r, side as u8, i), &mut s.model, library, Along::Strip);
                            if look
                                != (
                                    s.surface.clone(),
                                    s.material.clone(),
                                    s.profile.clone(),
                                    s.fade,
                                    s.model.clone(),
                                )
                            {
                                s.style = None;
                                changed = true;
                            }
                            changed |= ranges_ui(ui, &mut s.ranges, &picked, period, road.closed);
                            removed = row(ui, "", |ui| ui.button("Remove strip").clicked());
                        });
                if open.is_some() {
                    resp.header_response.scroll_to_me(Some(egui::Align::TOP));
                }
                if add_key && let Some(smp) = c.built.roads.get(r) {
                    let s_at = match node {
                        Some(n) => smp.s_at(n as f64),
                        None => strip.ranges.first().map_or(0.5 * smp.length, |rg| {
                            let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
                            if b < a && smp.closed {
                                b += smp.length;
                            }
                            (0.5 * (a + b)).rem_euclid(smp.length.max(1e-9))
                        }),
                    };
                    let at = smp.frame_at(s_at).pos;
                    crate::viewport::add_strip_key(c.editor, c.built, r, side, i, at);
                    return;
                }
                if removed {
                    c.editor.apply(
                        vec![Op::RemoveStrip {
                            road: name.clone(),
                            side,
                            name: s.name,
                        }],
                        None,
                    );
                    return;
                }
                if changed {
                    c.editor.apply(
                        vec![Op::PutStrip {
                            road: name.clone(),
                            side,
                            strip: s,
                            at: None,
                        }],
                        Some(&format!("strip {name} {side:?} {i}")),
                    );
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Add");
                let styles = c.editor.project.strip_styles.clone();
                for style in styles {
                    if ui
                        .small_button(format!("+ {}", style.name))
                        .on_hover_text(format!(
                            "{}; drag its ends in the view",
                            where_laid(&picked)
                        ))
                        .clicked()
                    {
                        let sname = crate::presets::free_name(&style.name, |n| {
                            strips.iter().any(|s| s.name == n)
                        });
                        let strip =
                            style.strip(&sname, stretch_round(&picked, period, road.closed));
                        // Kerbs go against the road; the rest outermost.
                        let kerb =
                            c.editor
                                .project
                                .surface_index(&style.surface)
                                .is_some_and(|k| {
                                    c.editor.project.surfaces[k].props.kind == Surface::Kerb
                                });
                        c.editor.apply(
                            vec![Op::PutStrip {
                                road: name.clone(),
                                side,
                                strip,
                                at: kerb.then_some(0),
                            }],
                            None,
                        );
                    }
                }
            });
        });
    }
}

pub(super) fn lines_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (_, materials) = names(&c.editor.project);
    let node = c.editor.selection.node();
    let picked = c.editor.selection.nodes.clone();
    let period = road.period();
    for (i, line) in road.lines.iter().enumerate() {
        let mut l = line.clone();
        let (mut changed, mut removed) = (false, false);
        let open = focused(c, Focus::Line(i));
        let resp = egui::CollapsingHeader::new(&l.name)
            .id_salt(("line", r, i))
            .default_open(true)
            .show_background(true)
            .open(open)
            .show(ui, |ui| {
                changed |= drag(ui, "Offset m", &mut l.offset, 0.05, -100.0..=100.0);
                changed |= drag(ui, "Width m", &mut l.width, 0.01, 0.01..=5.0);
                changed |= combo_row(ui, "Material", ("lm", r, i), &mut l.material, &materials);
                let mut dashed = l.dash.is_some();
                if check(ui, &mut dashed, "Dashed") {
                    l.dash = dashed.then_some((3.0, 9.0));
                    changed = true;
                }
                if let Some((on, off)) = &mut l.dash {
                    changed |= drag(ui, "Dash m", on, 0.1, 0.1..=100.0);
                    changed |= drag(ui, "Gap m", off, 0.1, 0.1..=100.0);
                }
                changed |= ranges_ui(ui, &mut l.ranges, &picked, period, road.closed);
                removed = row(ui, "", |ui| ui.button("Remove line").clicked());
            });
        if open.is_some() {
            resp.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        if removed {
            c.editor.apply(
                vec![Op::RemoveLine {
                    road: name.clone(),
                    name: l.name,
                }],
                None,
            );
            return;
        }
        if changed {
            c.editor.apply(
                vec![Op::PutLine {
                    road: name.clone(),
                    line: l,
                }],
                Some(&format!("line {name} {i}")),
            );
        }
    }
    ui.weak("Offsets are from the road's centre, positive to the left.");
    marks_ui(ui, c, r, &road, node);
    if ui.button("+ Line").clicked() {
        let n = (1..)
            .map(|k| format!("line {k}"))
            .find(|n| road.lines.iter().all(|l| &l.name != n))
            .expect("some name is free");
        let line = PaintLine {
            name: n,
            offset: 0.0,
            width: 0.12,
            material: "paint".into(),
            ranges: vec![],
            dash: Some((3.0, 9.0)),
        };
        c.editor.apply(
            vec![Op::PutLine {
                road: name.clone(),
                line,
            }],
            None,
        );
    }
}

/// Marks painted across the road: the start line, grid slots, pit lane lines.
fn marks_ui(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    r: usize,
    road: &open_racing_track_project::Road,
    node: Option<usize>,
) {
    let (_, materials) = names(&c.editor.project);
    let period = road.period();
    section(
        ui,
        format!("Marks across ({})", road.marks.len()),
        ("marks", r),
        false,
        |ui| {
            let mut remove = None;
            for (i, before) in road.marks.iter().enumerate() {
                let mut m = before.clone();
                egui::CollapsingHeader::new(format!("{}  ·  u {:.2}", m.name, m.at))
                    .id_salt(("mark", r, i))
                    .show(ui, |ui| {
                        drag(ui, "At u", &mut m.at, 0.005, 0.0..=period);
                        drag(ui, "Length m", &mut m.length, 0.01, 0.01..=20.0);
                        drag(ui, "From m", &mut m.from, 0.05, -100.0..=100.0);
                        drag(ui, "To m", &mut m.to, 0.05, -100.0..=100.0);
                        combo_row(ui, "Material", ("mm", r, i), &mut m.material, &materials);
                        if row(ui, "", |ui| ui.button("Remove mark").clicked()) {
                            remove = Some(m.name.clone());
                        }
                    });
                if m != *before && m.to > m.from {
                    c.editor.apply(
                        vec![Op::PutMark {
                            road: road.name.clone(),
                            mark: m,
                        }],
                        Some(&format!("mark {} {i}", road.name)),
                    );
                }
            }
            if let Some(name) = remove {
                c.editor.apply(
                    vec![Op::RemoveMark {
                        road: road.name.clone(),
                        name,
                    }],
                    None,
                );
            }
            let label = match node {
                Some(n) => format!("+ Mark at node {n}"),
                None => "+ Mark at the start".into(),
            };
            if ui.small_button(label).clicked() {
                let name =
                    crate::presets::free_name("mark", |n| road.marks.iter().any(|m| m.name == n));
                let (l, rt) = (
                    road.width_left
                        .eval(node.unwrap_or(0) as f64, period, road.closed),
                    road.width_right
                        .eval(node.unwrap_or(0) as f64, period, road.closed),
                );
                let mark = open_racing_track_project::project::Mark {
                    name,
                    at: node.unwrap_or(0) as f64,
                    length: 0.5,
                    from: -rt,
                    to: l,
                    material: if c.editor.project.material_index("paint").is_some() {
                        "paint".into()
                    } else {
                        materials.first().cloned().unwrap_or_default()
                    },
                };
                c.editor.apply(
                    vec![Op::PutMark {
                        road: road.name.clone(),
                        mark,
                    }],
                    None,
                );
            }
        },
    );
}

pub(super) fn barriers_tab(ui: &mut egui::Ui, c: &mut Ctx, library: &Library) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (_, materials) = names(&c.editor.project);
    let (_, wall_styles) = style_names(&c.editor.project);
    let picked = c.editor.selection.nodes.clone();
    let period = road.period();
    cornered(
        ui,
        c,
        road.barriers
            .iter()
            .filter(|b| crate::corners::held(c.built, r, &b.corner))
            .count(),
        "more",
    );
    for (i, barrier) in road.barriers.iter().enumerate() {
        if crate::corners::held(c.built, r, &barrier.corner) {
            continue;
        }
        let mut b = barrier.clone();
        let (mut changed, mut removed) = (false, false);
        let open = focused(c, Focus::Barrier(i));
        let kind = b.style.as_deref().unwrap_or("custom");
        let resp = egui::CollapsingHeader::new(format!("{}  ·  {kind}, {:?}", b.name, b.side))
            .id_salt(("barrier", r, i))
            .default_open(true)
            .show_background(true)
            .open(open)
            .show(ui, |ui| {
                let mut style = b.style.clone();
                if row(ui, "Type", |ui| {
                    style_combo(ui, ("btype", r, i), &mut style, &wall_styles, "custom")
                }) {
                    match style
                        .as_deref()
                        .and_then(|n| c.editor.project.wall_style(n))
                    {
                        Some(t) => t.restyle(&mut b),
                        None => b.style = None,
                    }
                    changed = true;
                }
                changed |= row(ui, "Side", |ui| {
                    choice(
                        ui,
                        &mut b.side,
                        &[(Side::Left, "Left"), (Side::Right, "Right")],
                    )
                });
                changed |= drag(ui, "From edge m", &mut b.offset, 0.1, 0.0..=500.0);
                // Its own shape and look: changing any of it leaves its type.
                let look = (b.height, b.thickness, b.material.clone(), b.model.clone());
                drag(ui, "Height m", &mut b.height, 0.05, 0.1..=20.0);
                drag(ui, "Thickness m", &mut b.thickness, 0.05, 0.0..=5.0);
                combo_row(ui, "Material", ("bm", r, i), &mut b.material, &materials);
                model_ui(ui, ("barrier", r, i), &mut b.model, library, Along::Wall);
                if look != (b.height, b.thickness, b.material.clone(), b.model.clone()) {
                    b.style = None;
                    changed = true;
                }
                changed |= ranges_ui(ui, &mut b.ranges, &picked, period, road.closed);
                removed = row(ui, "", |ui| ui.button("Remove barrier").clicked());
            });
        if open.is_some() {
            resp.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        if removed {
            c.editor.apply(
                vec![Op::RemoveBarrier {
                    road: name.clone(),
                    name: b.name,
                }],
                None,
            );
            return;
        }
        if changed {
            c.editor.apply(
                vec![Op::PutBarrier {
                    road: name.clone(),
                    barrier: b,
                }],
                Some(&format!("barrier {name} {i}")),
            );
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Add");
        let styles = c.editor.project.wall_styles.clone();
        for style in styles {
            if ui
                .small_button(format!("+ {}", style.name))
                .on_hover_text(format!(
                    "{}, on the left; drag it in the view",
                    where_laid(&picked)
                ))
                .clicked()
            {
                let n = crate::presets::free_name(&style.name, |n| {
                    road.barriers.iter().any(|b| b.name == n)
                });
                let barrier = style.barrier(
                    &n,
                    Side::Left,
                    10.0,
                    stretch_round(&picked, period, road.closed),
                );
                c.editor.apply(
                    vec![Op::PutBarrier {
                        road: name.clone(),
                        barrier,
                    }],
                    None,
                );
            }
        }
    });
}

/// Rows of models beside the road: trees, cones, boards, lights, stands, garages.
pub(super) fn rows_tab(ui: &mut egui::Ui, c: &mut Ctx, library: &Library) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let picked = c.editor.selection.nodes.clone();
    let period = road.period();
    ui.weak("A model repeated beside the road, facing it (its +X along the road, +Y towards it): every so many metres along its stretches, or at given places.");
    for (i, row_) in road.rows.iter().enumerate() {
        let mut w = row_.clone();
        let (mut changed, mut removed) = (false, false);
        let open = focused(c, Focus::Row(i));
        let count = c.built.roads.get(r).map_or(0, |smp| {
            open_racing_track_project::rows::copies(&road, smp, &w).len()
        });
        let resp = egui::CollapsingHeader::new(format!(
            "{}  ·  {count} × {}, {:?}",
            w.name,
            w.model.file_stem().unwrap_or_default().to_string_lossy(),
            w.side
        ))
        .id_salt(("row", r, i))
        .show_background(true)
        .open(open)
        .show(ui, |ui| {
            row(ui, "Model", |ui| {
                egui::ComboBox::from_id_salt(("row model", r, i))
                    .selected_text(w.model.to_string_lossy())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for m in library.model_paths() {
                            let label = m.to_string_lossy().into_owned();
                            changed |= ui.selectable_value(&mut w.model, m, label).changed();
                        }
                    });
            });
            changed |= row(ui, "Side", |ui| {
                choice(
                    ui,
                    &mut w.side,
                    &[(Side::Left, "Left"), (Side::Right, "Right")],
                )
            });
            changed |= drag(ui, "From edge m", &mut w.offset, 0.1, 0.0..=1000.0);
            if w.at.is_empty() {
                changed |= drag(ui, "Every m", &mut w.spacing, 0.5, 0.5..=5000.0);
            } else {
                row(ui, "Places", |ui| {
                    ui.label(format!("{} given (u)", w.at.len()));
                    if ui.small_button("Every so many metres instead").clicked() {
                        w.at.clear();
                        changed = true;
                    }
                });
            }
            let mut deg = w.yaw.to_degrees();
            if row(ui, "Turn", |ui| number(ui, &mut deg, 1.0, "°")) {
                w.yaw = deg.to_radians();
                changed = true;
            }
            changed |= drag(ui, "Scale", &mut w.scale, 0.01, 0.01..=100.0);
            ui.weak("Each copy differs by up to:");
            changed |= drag(ui, "Across m", &mut w.jitter.offset, 0.05, 0.0..=100.0);
            let mut jd = w.jitter.yaw.to_degrees();
            if row(ui, "Turn °", |ui| number(ui, &mut jd, 1.0, "°")) {
                w.jitter.yaw = jd.to_radians().max(0.0);
                changed = true;
            }
            changed |= drag(ui, "Size", &mut w.jitter.scale, 0.01, 0.0..=0.9);
            changed |= row(ui, "", |ui| {
                ui.checkbox(&mut w.drape, "Stand on the ground").changed()
                    | ui.checkbox(&mut w.collide, "Cars collide with them")
                        .changed()
            });
            if w.at.is_empty() {
                changed |= ranges_ui(ui, &mut w.ranges, &picked, period, road.closed);
            }
            removed = row(ui, "", |ui| ui.button("Remove row").clicked());
        });
        if open.is_some() {
            resp.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        if removed {
            c.editor.apply(
                vec![Op::RemoveRow {
                    road: name.clone(),
                    name: w.name,
                }],
                None,
            );
            return;
        }
        if changed {
            c.editor.apply(
                vec![Op::PutRow {
                    road: name.clone(),
                    row: w,
                }],
                Some(&format!("row {name} {i}")),
            );
        }
    }
    let models = library.model_paths();
    ui.horizontal_wrapped(|ui| {
        ui.label("Add a row of");
        for model in models {
            let stem = crate::assets::model_name(&model);
            if ui
                .small_button(format!("+ {stem}"))
                .on_hover_text(format!(
                    "{}, on the left, every 20 m; drag its ends and distance in the view",
                    where_laid(&picked)
                ))
                .clicked()
            {
                let n = crate::presets::free_name(&stem, |n| road.rows.iter().any(|w| w.name == n));
                let row = open_racing_track_project::project::PropRow {
                    name: n,
                    model: model.clone(),
                    side: Side::Left,
                    offset: 8.0,
                    spacing: 20.0,
                    ranges: stretch_round(&picked, period, road.closed),
                    at: vec![],
                    yaw: 0.0,
                    scale: 1.0,
                    jitter: Default::default(),
                    drape: true,
                    collide: false,
                };
                c.editor.apply(
                    vec![Op::PutRow {
                        road: name.clone(),
                        row,
                    }],
                    None,
                );
            }
        }
    });
}
