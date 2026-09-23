use glam::DVec3;
use open_racing_sim::{Track, TrackQuery};

/// Tracks progress along the centreline and times laps from start-line crossings.
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
}

impl LapTimer {
    pub fn new(track: &Track, position: DVec3) -> Self {
        let q = track.query(position, track.nearest_index(position));
        Self {
            hint: q.index,
            last_s: q.s,
            ..Default::default()
        }
    }

    /// Advances with the car's new position; returns the query and the progress delta.
    pub fn update(&mut self, track: &Track, position: DVec3, time: f64) -> (TrackQuery, f64) {
        let q = track.query(position, self.hint);
        self.hint = q.index;
        let delta = track.delta_s(self.last_s, q.s);
        self.progress += delta;
        // Start/finish line crossed forwards. A lap only counts if (almost) the whole
        // track was covered since the previous crossing.
        if q.s < self.last_s && delta > 0.0 {
            if let Some((t0, p0)) = self.lap_start
                && self.progress - p0 > 0.9 * track.length
            {
                let lap = time - t0;
                self.laps += 1;
                self.last_lap = Some(lap);
                self.best_lap = Some(self.best_lap.map_or(lap, |b| b.min(lap)));
            }
            self.lap_start = Some((time, self.progress));
        }
        self.current_lap = self.lap_start.map(|(t0, _)| time - t0);
        self.last_s = q.s;
        (q, delta)
    }

    pub fn hint(&self) -> usize {
        self.hint
    }
}
