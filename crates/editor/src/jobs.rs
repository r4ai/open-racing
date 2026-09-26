//! Baking the project into a package in the background, checking it with a test lap,
//! and launching the game on it.

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use open_racing_track_project::validate::LapSample;
use open_racing_track_project::{Cache, Project, bake, validate};

pub struct Baked {
    pub name: String,
    pub dir: PathBuf,
    pub report: String,
    pub ok: bool,
    pub lap: Vec<LapSample>,
}

#[derive(Resource, Default)]
pub struct Jobs {
    bake: Option<(Task<Result<Baked, String>>, bool)>,
    /// Report of the last bake.
    pub report: Option<String>,
    /// Whether the last bake passed its checks.
    pub passed: Option<bool>,
    /// The last bake's test lap, to show and replay in the view.
    pub lap: Vec<LapSample>,
}

impl Jobs {
    pub fn running(&self) -> bool {
        self.bake.is_some()
    }

    /// Drops the last bake's report and lap: another project was opened.
    pub fn forget(&mut self) {
        self.report = None;
        self.passed = None;
        self.lap.clear();
    }

    /// Bakes the project into `<content>/tracks/<name>/`; `play` launches the game on it
    /// if it passes.
    pub fn bake(&mut self, project: &Project, dir: &Path, play: bool) {
        if self.bake.is_some() {
            return;
        }
        let (project, dir) = (project.clone(), dir.to_path_buf());
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let package =
                bake::bake(&project, &dir, &mut Cache::default()).map_err(|e| e.to_string())?;
            let report = validate::check(&package, true);
            let out = open_racing_track::tracks_dir().join(&project.name);
            package.save(&out).map_err(|e| e.to_string())?;
            Ok(Baked {
                name: project.name.clone(),
                dir: out,
                report: report.to_string(),
                ok: report.ok(),
                // The race-pace lap when there was one: where the car brakes, how
                // fast it goes and where it runs wide say most about the track.
                lap: report
                    .pace
                    .or(report.drive)
                    .map(|d| d.path)
                    .unwrap_or_default(),
            })
        });
        self.bake = Some((task, play));
        self.passed = None;
        self.report = Some("baking and driving a test lap…".into());
    }
}

pub fn poll(mut jobs: ResMut<Jobs>, mut editor: ResMut<crate::state::Editor>) {
    let Some((task, play)) = &mut jobs.bake else {
        return;
    };
    let Some(result) = check_ready(task) else {
        return;
    };
    let play = *play;
    jobs.bake = None;
    match result {
        Ok(b) => {
            editor.status = format!("baked into {}", b.dir.display());
            jobs.report = Some(b.report);
            jobs.passed = Some(b.ok);
            jobs.lap = b.lap;
            if play && b.ok {
                match launch(&b.name) {
                    Ok(how) => editor.status = format!("driving \"{}\" ({how})", b.name),
                    Err(e) => editor.status = e,
                }
            } else if play {
                editor.status = "the track did not pass its checks; not launching".into();
            }
        }
        Err(e) => {
            editor.status = format!("bake failed: {e}");
            jobs.report = Some(e);
            jobs.passed = Some(false);
        }
    }
}

/// Starts the game on a baked track: the app next to this executable, or else through
/// cargo from the source tree.
fn launch(track: &str) -> Result<&'static str, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let app = exe.with_file_name(format!("open-racing-app{}", std::env::consts::EXE_SUFFIX));
    if app.is_file() {
        Command::new(app)
            .args(["--track", track])
            .spawn()
            .map_err(|e| format!("launching the app: {e}"))?;
        return Ok("app");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Command::new("cargo")
        .current_dir(root)
        .args([
            "run",
            "-p",
            "open-racing-app",
            "--features",
            "dev",
            "--",
            "--track",
            track,
        ])
        .spawn()
        .map_err(|e| format!("launching the app through cargo: {e}"))?;
    Ok("building the app with cargo")
}
