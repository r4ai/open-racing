//! Work that takes seconds — dyno sweeps, cycle traces, sound renders, bakes — run on
//! threads while the editor stays live; their results land here.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use bevy::prelude::*;
use open_racing_engine_sim::analysis::SoundReport;
use open_racing_engine_sim::dyno::DynoRun;
use open_racing_engine_sim::trace::CycleTrace;
use open_racing_engine_sim::{Build, Script, analysis, dyno, render, trace};
use open_racing_machine_project::Library;
use open_racing_machine_project::bake::{BakeOptions, BakeReport, bake, package_dir};

/// What a job produced.
pub enum Done {
    Dyno(String, DynoRun),
    Trace(String, CycleTrace),
    Sound(String, PathBuf, SoundReport),
    Bake(String, BakeReport),
    Failed(String, String),
}

#[derive(Resource)]
pub struct Jobs {
    tx: Mutex<Sender<Done>>,
    rx: Mutex<Receiver<Done>>,
    /// Names of the jobs running.
    pub running: Vec<String>,
    /// Latest results, for the panels.
    pub dyno: Option<(String, DynoRun)>,
    /// The dyno run before the latest, to compare an edit against.
    pub previous_dyno: Option<(String, DynoRun)>,
    pub trace: Option<(String, CycleTrace)>,
    pub sound: Option<(String, PathBuf, SoundReport)>,
    pub bake: Option<(String, BakeReport)>,
    pub status: Option<String>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            tx: Mutex::new(tx),
            rx: Mutex::new(rx),
            running: Vec::new(),
            dyno: None,
            previous_dyno: None,
            trace: None,
            sound: None,
            bake: None,
            status: None,
        }
    }
}

impl Jobs {
    pub fn busy(&self, name: &str) -> bool {
        self.running.iter().any(|r| r == name)
    }

    fn spawn(&mut self, name: &str, f: impl FnOnce() -> Done + Send + 'static) {
        self.running.push(name.to_string());
        let tx = self.tx.lock().expect("not poisoned").clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    pub fn dyno(&mut self, target: String, b: Build, rpms: Vec<f64>, throttle: f64) {
        self.spawn("dyno", move || match dyno::sweep(&b, &rpms, throttle) {
            Ok(r) => Done::Dyno(target, r),
            Err(e) => Done::Failed("dyno".into(), e),
        });
    }

    pub fn trace(&mut self, target: String, b: Build, rpm: f64, throttle: f64, pipes: Vec<String>) {
        self.spawn("trace", move || {
            match trace::cycle(&b, rpm, throttle, 0, &pipes) {
                Ok(t) => Done::Trace(target, t),
                Err(e) => Done::Failed("trace".into(), e),
            }
        });
    }

    pub fn sound(
        &mut self,
        target: String,
        b: Build,
        script: Script,
        sound: open_racing_engine_sim::SoundSettings,
        out: PathBuf,
    ) {
        self.spawn("sound", move || {
            let r = b
                .build()
                .and_then(|(mut m, _)| render::record(&mut m, &script, sound));
            match r {
                Ok(rec) => match rec.write_wav(&out, -1.0, None) {
                    Ok(()) => Done::Sound(target, out, analysis::analyse(&rec, 16)),
                    Err(e) => Done::Failed("sound".into(), e.to_string()),
                },
                Err(e) => Done::Failed("sound".into(), e),
            }
        });
    }

    pub fn bake(&mut self, lib: Library, machine: String) {
        self.spawn("bake", move || {
            let o = BakeOptions::default();
            match bake(&lib, &machine, &o) {
                Ok((pkg, report)) => match pkg.save(&package_dir(&machine)) {
                    Ok(()) => Done::Bake(machine, report),
                    Err(e) => Done::Failed("bake".into(), e.to_string()),
                },
                Err(e) => Done::Failed("bake".into(), e),
            }
        });
    }

    /// Takes the results that arrived.
    pub fn poll(&mut self) {
        loop {
            let Ok(d) = self.rx.lock().expect("not poisoned").try_recv() else {
                break;
            };
            let name = match &d {
                Done::Dyno(..) => "dyno",
                Done::Trace(..) => "trace",
                Done::Sound(..) => "sound",
                Done::Bake(..) => "bake",
                Done::Failed(n, _) => n.as_str(),
            }
            .to_string();
            if let Some(i) = self.running.iter().position(|r| *r == name) {
                self.running.remove(i);
            }
            match d {
                Done::Dyno(t, r) => {
                    self.status = Some(format!("dyno of {t} done"));
                    self.previous_dyno = self.dyno.replace((t, r));
                }
                Done::Trace(t, r) => self.trace = Some((t, r)),
                Done::Sound(t, p, r) => {
                    self.status = Some(format!("{} written", p.display()));
                    self.sound = Some((t, p, r));
                }
                Done::Bake(m, r) => {
                    self.status = Some(format!("baked {m}: cargo dev -- --car {m}"));
                    self.bake = Some((m, r));
                }
                Done::Failed(n, e) => self.status = Some(format!("{n} failed: {e}")),
            }
        }
    }
}

pub fn poll(mut jobs: ResMut<Jobs>) {
    jobs.poll();
}
