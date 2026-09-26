//! The World tab: the sky and the light the track is shown in, which the game starts
//! in. The time of day, the month and the latitude set where the sun runs; the weather
//! sets the clouds, the air and how bright the day is; the exposure and the haze how
//! the picture looks. The view is lit by them as they are set.

use open_racing_sim::Sky;
use open_racing_track::{Environment, Season};

use super::*;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A time of day as the clock shows it.
pub fn clock(hour: f64) -> String {
    let minutes = (hour * 60.0).round() as i64 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Where the sun is: how high, degrees, and which way, as a compass point.
fn sun_words(e: &Environment, latitude: f64) -> String {
    let d = open_racing_sim::weather::sun_direction(latitude, e.day_of_year(), e.hour);
    let elevation = d.z.asin().to_degrees();
    if elevation < -6.0 {
        return "night: the sun is down".into();
    }
    let bearing = d.x.atan2(d.y).to_degrees().rem_euclid(360.0);
    let points = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let from = points[((bearing + 22.5) / 45.0) as usize % 8];
    if elevation < 0.0 {
        format!("twilight: the sun just below the horizon in the {from}")
    } else {
        format!("the sun {elevation:.0}° up in the {from}")
    }
}

pub(super) fn world_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let mut e = c.editor.project.environment;
    let geo = c.editor.project.geo.map(|g| g.lat);
    let latitude = e.latitude.or(geo).unwrap_or(48.0);
    let mut changed = false;
    ui.weak("The sky and the light the track is shown in. The game starts in them (its weather settings can start in others), and the view is lit by them.");
    section(ui, "Sun", "world sun", true, |ui| {
        let mut hour = e.hour;
        if row(ui, "Time", |ui| {
            ui.add(
                egui::Slider::new(&mut hour, 0.0..=23.99)
                    .custom_formatter(|h, _| clock(h))
                    .step_by(1.0 / 12.0),
            )
        })
        .changed()
        {
            e.hour = hour;
            changed = true;
        }
        row(ui, "", |ui| {
            for (label, h) in [
                ("Dawn", 6.5),
                ("Morning", 9.5),
                ("Noon", 12.0),
                ("Afternoon", 15.5),
                ("Dusk", 19.5),
            ] {
                if ui.small_button(label).clicked() {
                    e.hour = h;
                    changed = true;
                }
            }
        });
        row(ui, "Month", |ui| {
            egui::ComboBox::from_id_salt("world month")
                .selected_text(MONTHS[e.month.clamp(1, 12) as usize - 1])
                .show_ui(ui, |ui| {
                    for (i, m) in MONTHS.iter().enumerate() {
                        changed |= ui
                            .selectable_value(&mut e.month, i as u32 + 1, *m)
                            .changed();
                    }
                })
        });
        let mut own = e.latitude.is_some();
        row(ui, "Latitude", |ui| {
            if ui
                .checkbox(&mut own, "its own")
                .on_hover_text(match geo {
                    Some(g) => format!("Otherwise the place's, {g:.2}° (File › Import Centreline, or the elevation data, set it)"),
                    None => "Otherwise 48° north, until the project is placed on the Earth".into(),
                })
                .changed()
            {
                e.latitude = own.then_some(latitude);
                changed = true;
            }
            match &mut e.latitude {
                Some(l) => {
                    changed |= ui
                        .add(
                            egui::DragValue::new(l)
                                .speed(0.1)
                                .range(-89.0..=89.0)
                                .suffix("°"),
                        )
                        .changed();
                }
                None => {
                    ui.weak(format!("{latitude:.1}°"));
                }
            }
        });
        ui.weak(sun_words(&e, latitude));
        row(ui, "Plants", |ui| {
            let name = |s: Option<Season>| match s {
                None => "as the month",
                Some(Season::Spring) => "spring",
                Some(Season::Summer) => "summer",
                Some(Season::Autumn) => "autumn",
                Some(Season::Winter) => "winter",
            };
            egui::ComboBox::from_id_salt("world season")
                .selected_text(name(e.season))
                .show_ui(ui, |ui| {
                    for s in std::iter::once(None).chain(Season::ALL.map(Some)) {
                        changed |= ui.selectable_value(&mut e.season, s, name(s)).changed();
                    }
                })
                .response
                .on_hover_text("The season scattered plants show: fresh leaves, summer's, autumn's colours, or bare branches and straw. As the month, half a year on south of the equator.")
        });
    });
    section(ui, "Weather", "world weather", true, |ui| {
        row(ui, "Sky", |ui| {
            egui::ComboBox::from_id_salt("world sky")
                .selected_text(e.sky.name())
                .show_ui(ui, |ui| {
                    for sky in Sky::ALL {
                        changed |= ui.selectable_value(&mut e.sky, sky, sky.name()).changed();
                    }
                })
        });
        changed |= check(
            ui,
            &mut e.changing,
            "The sky changes by itself as time goes by",
        );
        changed |= drag(ui, "Warmer by K", &mut e.temperature, 0.5, -30.0..=30.0);
        ui.weak("Added to the air temperature the month and the sky give: the road warms and the air thins with it.");
    });
    section(ui, "Picture", "world picture", true, |ui| {
        changed |= row(ui, "Exposure", |ui| {
            ui.add(egui::Slider::new(&mut e.exposure, -3.0..=3.0).suffix(" EV"))
                .on_hover_text("Brighter (above 0) or darker than the daylight calls for")
                .changed()
        });
        changed |= row(ui, "Haze", |ui| {
            ui.add(egui::Slider::new(&mut e.haze, 0.0..=4.0).max_decimals(2))
                .on_hover_text(
                    "How thick the haze is against what the weather gives: 0 none, 1 as it gives",
                )
                .changed()
        });
        ui.checkbox(&mut c.tool.overlays.sky, "Light the view by them")
            .on_hover_text("Off: an even light to work in, whatever the time of day");
    });
    if changed && e != c.editor.project.environment {
        c.editor.apply(
            vec![Op::SetEnvironment { environment: e }],
            Some("environment"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_is_up_at_noon_and_down_at_midnight() {
        let mut e = Environment {
            hour: 12.0,
            ..Default::default()
        };
        assert!(
            sun_words(&e, 48.0).contains("in the S"),
            "{}",
            sun_words(&e, 48.0)
        );
        e.hour = 0.0;
        assert!(sun_words(&e, 48.0).starts_with("night"));
        assert_eq!(clock(14.5), "14:30");
        assert_eq!(clock(23.999), "00:00");
    }
}
