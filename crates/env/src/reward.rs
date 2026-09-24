//! Pluggable reward and termination rules.

/// What happened during one agent step, as seen by reward / termination rules.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepInfo {
    /// Distance gained along the centreline during this step, m (negative = backwards).
    pub progress: f64,
    /// Total distance gained since the episode start, m.
    pub total_progress: f64,
    pub speed: f64,
    /// Lateral offset normalised by the half width on that side (|x| > 1 = beyond the edge).
    pub offset: f64,
    /// Number of wheels off the track (on anything but asphalt and kerbs).
    pub wheels_off: usize,
    /// Grip the tyres have lost to tread temperature, pressure, wear and dirt, summed
    /// over the four tyres (0 = all at their best).
    pub grip_loss: f64,
    /// Hardest hit into a wall or barrier during the step, m/s into the wall.
    pub barrier_impact: f64,
    /// cos of the angle between the car's heading and the track direction.
    pub heading_cos: f64,
    /// Change of normalised steering input during the step.
    pub steer_change: f64,
    /// Episode time, s.
    pub time: f64,
    /// Time spent continuously with all wheels off, s.
    pub off_track_time: f64,
    /// Time spent continuously almost stationary, s.
    pub stuck_time: f64,
    /// Time spent continuously facing the wrong way while moving, s.
    pub wrong_way_time: f64,
    /// Physics produced a non-finite state.
    pub invalid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    /// The episode ended because of the agent (crash, off track, …).
    Terminated,
    /// The episode was cut off by a limit (time).
    Truncated,
}

pub trait RewardFn: Send + Sync {
    fn reward(&self, info: &StepInfo, done: Option<Done>) -> f64;
}

pub trait TerminationFn: Send + Sync {
    fn done(&self, info: &StepInfo) -> Option<Done>;
}

/// Progress along the track, with penalties for leaving it and for ending the episode.
#[derive(Clone, Debug)]
pub struct DefaultReward {
    /// Reward per metre of progress.
    pub progress_weight: f64,
    /// Penalty per step per wheel off the track.
    pub off_track_weight: f64,
    /// Penalty per unit of steering input change (smoothness).
    pub steer_change_weight: f64,
    /// Penalty per step per unit of tyre grip lost (see [`StepInfo::grip_loss`]): an
    /// immediate price for overheating the tyres, whose cost in lap time comes too late
    /// for the discount horizon.
    pub grip_loss_weight: f64,
    /// Penalty on termination.
    pub termination_penalty: f64,
}

impl Default for DefaultReward {
    fn default() -> Self {
        Self {
            progress_weight: 0.1,
            off_track_weight: 0.02,
            steer_change_weight: 0.0,
            grip_loss_weight: 0.0,
            termination_penalty: 10.0,
        }
    }
}

impl RewardFn for DefaultReward {
    fn reward(&self, info: &StepInfo, done: Option<Done>) -> f64 {
        let mut r = self.progress_weight * info.progress
            - self.off_track_weight * info.wheels_off as f64
            - self.steer_change_weight * info.steer_change.abs()
            - self.grip_loss_weight * info.grip_loss;
        if done == Some(Done::Terminated) {
            r -= self.termination_penalty;
        }
        r
    }
}

#[derive(Clone, Debug)]
pub struct DefaultTermination {
    /// Episode length limit, s.
    pub max_time: f64,
    /// Allowed time with all four wheels off the track, s.
    pub max_off_track_time: f64,
    /// Allowed time below 1 m/s (after the first seconds), s.
    pub max_stuck_time: f64,
    /// Allowed time facing the wrong way while moving, s.
    pub max_wrong_way_time: f64,
    /// Hardest allowed hit into a wall, m/s into the wall; anything harder is a crash.
    pub max_barrier_impact: f64,
}

impl Default for DefaultTermination {
    fn default() -> Self {
        Self {
            max_time: 180.0,
            max_off_track_time: 0.5,
            max_stuck_time: 4.0,
            max_wrong_way_time: 1.0,
            max_barrier_impact: 3.0,
        }
    }
}

impl TerminationFn for DefaultTermination {
    fn done(&self, i: &StepInfo) -> Option<Done> {
        if i.invalid
            || i.off_track_time > self.max_off_track_time
            || i.stuck_time > self.max_stuck_time
            || i.wrong_way_time > self.max_wrong_way_time
            || i.barrier_impact > self.max_barrier_impact
        {
            Some(Done::Terminated)
        } else if i.time >= self.max_time {
            Some(Done::Truncated)
        } else {
            None
        }
    }
}
