//! The engine heard: running live on the simulator's real-time thread, or — where the
//! machine cannot simulate it in real time, or at a quality it cannot — rendered into
//! memory first and played from there, as a video editor's RAM preview. Either feeds
//! Bevy's audio as a stream of samples.
//!
//! A live run records the pedal as it goes (a take), so that the same take can be
//! rendered again at a higher quality and played back.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::audio::{AddAudioSource, ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;
use open_racing_engine_sim::realtime::{AtomicF64, AudioOut, Realtime, Telemetry};
use open_racing_engine_sim::render::{Script, ScriptLoad, record_streaming};
use open_racing_engine_sim::{Build, SoundSettings};

/// A run rendered into memory, and where playback is in it.
pub struct Preview {
    pub label: String,
    /// Interleaved stereo frames at 48 kHz, Pa.
    frames: Mutex<Vec<f32>>,
    /// Frames rendered so far, and in all.
    pub rendered: AtomicUsize,
    pub total: usize,
    pub playhead: AtomicUsize,
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub done: AtomicBool,
    cancel: AtomicBool,
    /// Output per pascal.
    pub gain: AtomicF64,
    pub error: Mutex<Option<String>>,
    /// Speed along the run: (s, rpm).
    pub rpm: Mutex<Vec<(f64, f64)>>,
    started: Instant,
    /// Wall-clock seconds the render took, once done.
    pub took: Mutex<Option<f64>>,
}

impl Preview {
    pub fn fraction(&self) -> f64 {
        self.rendered.load(Ordering::Relaxed) as f64 / self.total.max(1) as f64
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.playing.store(false, Ordering::Relaxed);
    }

    /// Seconds rendered per second of wall clock, so far.
    pub fn speed(&self) -> f64 {
        let secs = self.rendered.load(Ordering::Relaxed) as f64
            / open_racing_engine_sim::dsp::OUTPUT_RATE as f64;
        let wall = match *self.took.lock().expect("not poisoned") {
            Some(t) => t,
            None => self.started.elapsed().as_secs_f64(),
        };
        secs / wall.max(1e-3)
    }
}

/// What the audio stream plays.
enum Feed {
    Live(AudioOut),
    Preview(Arc<Preview>),
}

/// Where a new feed is handed to the playing stream.
type Slot = Arc<Mutex<Option<Feed>>>;

/// The running engine or the preview, if any.
#[derive(Resource, Default)]
pub struct Live {
    pub engine: Option<Realtime>,
    /// What it runs: the engine target and the library revision it was built from.
    pub target: Option<(String, u64)>,
    pub telemetry: Telemetry,
    slot: Slot,
    pub volume: f32,
    pub muted: bool,
    pub error: Option<String>,
    pub preview: Option<Arc<Preview>>,
    /// The pedal and dyno speed of the live run over time: (s, pedal, held rpm, rpm).
    pub take: Vec<(f64, f64, f64, f64)>,
    take_start: Option<Instant>,
}

impl Live {
    pub fn running(&self) -> bool {
        self.engine.is_some()
    }

    /// Starts (or restarts) the engine.
    pub fn start(&mut self, target: String, revision: u64, b: Build, sound: SoundSettings) {
        self.stop();
        self.stop_preview();
        match Realtime::start(b, sound) {
            Ok((rt, out)) => {
                if let Ok(mut s) = self.slot.lock() {
                    *s = Some(Feed::Live(out));
                }
                rt.controls.full_scale.set(self.full_scale());
                self.engine = Some(rt);
                self.target = Some((target, revision));
                self.error = None;
                self.take.clear();
                self.take_start = Some(Instant::now());
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.stop();
        }
        self.target = None;
        self.take_start = None;
    }

    /// Full-scale pressure for the volume, Pa.
    pub fn full_scale(&self) -> f64 {
        if self.muted {
            1e9
        } else {
            60.0 / (self.volume.max(0.01) as f64)
        }
    }

    /// The last live run's pedal as a script, from its start.
    pub fn take_script(&self) -> Option<Script> {
        let (first, last) = (self.take.first()?, self.take.last()?);
        let duration = last.0 - first.0;
        if duration < 0.5 {
            return None;
        }
        let held = first.2 > 0.0;
        Some(Script {
            duration,
            load: if held {
                ScriptLoad::Speed
            } else {
                ScriptLoad::Free {
                    inertia: 0.0,
                    torque: 0.0,
                }
            },
            rpm: if held {
                self.take.iter().map(|k| (k.0 - first.0, k.2)).collect()
            } else {
                vec![(0.0, first.3.max(300.0))]
            },
            throttle: self.take.iter().map(|k| (k.0 - first.0, k.1)).collect(),
            warmup: 0.3,
        })
    }

    /// Renders a script into memory at any quality, and plays it once it is all cached;
    /// `play` plays what is cached so far.
    pub fn start_preview(
        &mut self,
        label: String,
        b: Build,
        script: Script,
        sound: SoundSettings,
        looping: bool,
    ) {
        self.stop();
        self.stop_preview();
        let total = (script.duration * open_racing_engine_sim::dsp::OUTPUT_RATE as f64) as usize;
        let p = Arc::new(Preview {
            label,
            frames: Mutex::new(Vec::with_capacity(total * 2)),
            rendered: AtomicUsize::new(0),
            total,
            playhead: AtomicUsize::new(0),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(looping),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            gain: AtomicF64::new(1.0 / self.full_scale()),
            error: Mutex::new(None),
            rpm: Mutex::new(Vec::new()),
            started: Instant::now(),
            took: Mutex::new(None),
        });
        if let Ok(mut s) = self.slot.lock() {
            *s = Some(Feed::Preview(p.clone()));
        }
        self.preview = Some(p.clone());
        let spawned = std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                let r = b.build().and_then(|(mut m, _)| {
                    record_streaming(&mut m, &script, sound, |new, _| {
                        if p.cancel.load(Ordering::Relaxed) {
                            return false;
                        }
                        let n = new.first().map_or(0, |c| c.len());
                        if let Ok(mut f) = p.frames.lock() {
                            for i in 0..n {
                                let l = new[0][i];
                                let r = new.get(1).map_or(l, |c| c[i]);
                                f.push(l);
                                f.push(r);
                            }
                            p.rendered.store(f.len() / 2, Ordering::Relaxed);
                        }
                        true
                    })
                });
                match r {
                    Ok(rec) => {
                        if let Ok(mut rpm) = p.rpm.lock() {
                            *rpm = rec.telemetry.iter().map(|t| (t.0, t.1)).collect();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut x) = p.error.lock() {
                            *x = Some(e);
                        }
                    }
                }
                if let Ok(mut t) = p.took.lock() {
                    *t = Some(p.started.elapsed().as_secs_f64());
                }
                p.done.store(true, Ordering::Relaxed);
                // All cached: play it from the start.
                if !p.cancel.load(Ordering::Relaxed) {
                    p.playhead.store(0, Ordering::Relaxed);
                    p.playing.store(true, Ordering::Relaxed);
                }
            });
        if let Err(e) = spawned {
            self.error = Some(e.to_string());
        }
    }

    pub fn stop_preview(&mut self) {
        if let Some(p) = self.preview.take() {
            p.cancel();
        }
    }
}

/// The sound source Bevy plays: whatever feed the slot holds.
#[derive(Asset, TypePath)]
pub struct EngineSound {
    slot: Slot,
}

impl Decodable for EngineSound {
    type Decoder = EngineDecoder;

    fn decoder(&self) -> Self::Decoder {
        EngineDecoder {
            slot: self.slot.clone(),
            feed: None,
            until_check: 0,
            chunk: Vec::new(),
            at: 0,
        }
    }
}

pub struct EngineDecoder {
    slot: Slot,
    feed: Option<Feed>,
    until_check: u32,
    /// Samples copied from a preview, so as not to lock it for each.
    chunk: Vec<f32>,
    at: usize,
}

/// The next sample of a preview (interleaved stereo).
fn preview_sample(p: &Preview, chunk: &mut Vec<f32>, at: &mut usize) -> f32 {
    if *at >= chunk.len() {
        chunk.clear();
        *at = 0;
        let head = p.playhead.load(Ordering::Relaxed);
        if p.playing.load(Ordering::Relaxed) && head >= p.total {
            if p.looping.load(Ordering::Relaxed) && p.done.load(Ordering::Relaxed) {
                p.playhead.store(0, Ordering::Relaxed);
            } else {
                p.playing.store(false, Ordering::Relaxed);
            }
        }
        let head = p.playhead.load(Ordering::Relaxed);
        // Only what is cached plays; past it, silence until more is.
        let n = 256.min(p.rendered.load(Ordering::Relaxed).saturating_sub(head));
        if !p.playing.load(Ordering::Relaxed) || n == 0 {
            chunk.extend([0.0; 64]);
        } else if let Ok(f) = p.frames.try_lock() {
            let g = p.gain.get() as f32;
            chunk.extend(f[head * 2..(head + n) * 2].iter().map(|v| (v * g).tanh()));
            p.playhead.store(head + n, Ordering::Relaxed);
        } else {
            chunk.extend([0.0; 2]);
        }
    }
    let v = chunk[*at];
    *at += 1;
    v
}

impl Iterator for EngineDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.until_check == 0 {
            self.until_check = 1024;
            if let Ok(mut s) = self.slot.try_lock()
                && let Some(f) = s.take()
            {
                self.feed = Some(f);
                self.chunk.clear();
                self.at = 0;
            }
        }
        self.until_check -= 1;
        Some(match &mut self.feed {
            None => 0.0,
            Some(Feed::Live(o)) => o.next_sample(),
            Some(Feed::Preview(p)) => preview_sample(p, &mut self.chunk, &mut self.at),
        })
    }
}

impl Source for EngineDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(open_racing_engine_sim::dsp::OUTPUT_RATE).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct LivePlugin;

impl Plugin for LivePlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<EngineSound>()
            .insert_resource(Live {
                volume: 0.5,
                ..Default::default()
            })
            .add_systems(Startup, spawn)
            .add_systems(Update, telemetry);
    }
}

fn spawn(mut commands: Commands, mut sounds: ResMut<Assets<EngineSound>>, live: Res<Live>) {
    commands.spawn(AudioPlayer(sounds.add(EngineSound {
        slot: live.slot.clone(),
    })));
}

fn telemetry(mut live: ResMut<Live>) {
    let scale = live.full_scale();
    if let Some(p) = &live.preview {
        p.gain.set(1.0 / scale);
    }
    let Some(e) = &live.engine else { return };
    e.controls.full_scale.set(scale);
    let t = e.telemetry();
    let (hold, pedal) = (e.controls.hold_rpm.get(), e.controls.pedal.get());
    // A take of up to a minute, sampled at the frame rate.
    if let Some(t0) = live.take_start {
        let now = t0.elapsed().as_secs_f64();
        if now < 60.0 {
            let rpm = t.rpm;
            live.take.push((now, pedal, hold, rpm));
        }
    }
    live.telemetry = t;
}
