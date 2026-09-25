use glam::DVec3;
use open_racing_sim::{Track, TrackCoords};

/// Most sectors a lap is timed in; later sector boundaries are ignored.
pub const MAX_SECTORS: usize = 8;

/// Tracks progress along the centreline and times laps from start-line crossings, and
/// their sectors from the track layout's boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct LapTimer {
    hint: usize,
    last_s: f64,
    /// Distance gained since `reset`, m (negative when going backwards).
    pub progress: f64,
    lap_start: Option<(f64, f64)>,
    pub laps: u32,
    pub last_lap: Option<f64>,
    pub best_lap: Option<f64>,
    /// Time of the current lap so far, if one is running.
    pub current_lap: Option<f64>,
    /// Sector the car is in, and when it entered it (while a lap is running).
    sector: usize,
    sector_start: Option<f64>,
    /// Sector times of the current lap (after the start line), then of the last one.
    pub sectors: [Option<f64>; MAX_SECTORS],
    pub last_sectors: [Option<f64>; MAX_SECTORS],
    pub best_sectors: [Option<f64>; MAX_SECTORS],
}

impl LapTimer {
    pub fn new(track: &Track, position: DVec3) -> Self {
        let q = track.locate(position, track.nearest_index(position));
        // Projection of the exact spawn point can land a few millimetres
        // past the line (0.0044 m on Watkins Glen).
        let on_line = q.s < 1.0 || track.length - q.s < 1.0;
        Self {
            hint: q.index,
            last_s: q.s,
            // A standing start on the timing line already begins a lap. Random
            // starts elsewhere still need to reach the line before timing one.
            lap_start: on_line.then_some((0.0, 0.0)),
            current_lap: on_line.then_some(0.0),
            sector: track.layout.sector_at(q.s),
            ..Default::default()
        }
    }

    /// Advances with the car's new position; returns its track coordinates and the
    /// progress delta.
    pub fn update(&mut self, track: &Track, position: DVec3, time: f64) -> (TrackCoords, f64) {
        let q = track.locate(position, self.hint);
        self.hint = q.index;
        let delta = track.delta_s(self.last_s, q.s);
        self.progress += delta;
        let sector = track.layout.sector_at(q.s).min(MAX_SECTORS - 1);
        // Start/finish line crossed forwards. A lap only counts if (almost) the whole
        // track was covered since the previous crossing.
        let crossed = q.s < self.last_s && delta > 0.0;
        if sector != self.sector {
            // Only a forward step into the next sector (or the finish) times one.
            let next = (self.sector + 1) % (track.layout.sectors.len().min(MAX_SECTORS - 1) + 1);
            if let Some(t0) = self.sector_start
                && sector == next
                && delta > 0.0
            {
                let t = time - t0;
                self.sectors[self.sector] = Some(t);
                let best = &mut self.best_sectors[self.sector];
                *best = Some(best.map_or(t, |b| b.min(t)));
            }
            self.sector_start = (sector == next && delta > 0.0).then_some(time);
            self.sector = sector;
        }
        if crossed {
            if let Some((t0, p0)) = self.lap_start
                && self.progress - p0 > 0.9 * track.length
            {
                let lap = time - t0;
                self.laps += 1;
                self.last_lap = Some(lap);
                self.best_lap = Some(self.best_lap.map_or(lap, |b| b.min(lap)));
            }
            self.lap_start = Some((time, self.progress));
            self.last_sectors = self.sectors;
            self.sectors = [None; MAX_SECTORS];
        }
        self.current_lap = self.lap_start.map(|(t0, _)| time - t0);
        self.last_s = q.s;
        (q, delta)
    }

    pub fn hint(&self) -> usize {
        self.hint
    }
}

#[cfg(test)]
mod tests {
    use open_racing_sim::{Layout, Track};

    use super::*;

    #[test]
    fn times_sectors_between_boundaries() {
        let base = Track::default_circuit();
        let length = base.length;
        let track = base.with_layout(Layout {
            sectors: vec![length / 3.0, 2.0 * length / 3.0],
            ..Default::default()
        });
        // Drive one and a half laps at 1 m per "second", from just before the line.
        let mut lap = LapTimer::new(&track, track.sample_at(-5.0).pos);
        let mut t = 0.0;
        for k in 0..(1.5 * length) as usize {
            t = k as f64;
            lap.update(&track, track.sample_at(k as f64 - 4.0).pos, t);
        }
        assert!(t > 0.0);
        assert_eq!(lap.laps, 1);
        let third = length / 3.0;
        for i in 0..3 {
            let s = lap.last_sectors[i].unwrap();
            assert!((s - third).abs() < 3.0, "sector {i}: {s} vs {third}");
        }
        assert!(lap.last_sectors[3].is_none());
    }

    #[test]
    fn standing_start_counts_first_completed_lap() {
        let track = Track::default_circuit();
        let mut lap = LapTimer::new(&track, track.sample_at(0.01).pos);
        for s in 1..=(track.length.ceil() as usize + 5) {
            lap.update(&track, track.sample_at(s as f64 + 0.01).pos, s as f64);
        }
        assert_eq!(lap.laps, 1);
    }
}
