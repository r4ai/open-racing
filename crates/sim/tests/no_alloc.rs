//! `Car::step` must not touch the heap: it runs millions of times per second in training.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use open_racing_sim::{
    Car, CarModel, Controls, RubberMap, Shift, Track, TrackEvolution, Weather, WeatherSettings,
};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn step_does_not_allocate() {
    let track = Track::default_circuit();
    let mut car = Car::new(Arc::new(CarModel::gt3()), &track, 0.0, 0.0, 20.0, 2);
    let before = ALLOCS.load(Ordering::Relaxed);
    for k in 0..10_000 {
        let shift = if k == 5000 { Shift::Up } else { Shift::None };
        car.step(
            &track,
            &Controls {
                throttle: 0.7,
                steer_wheel_angle: 0.3,
                shift,
                ..Default::default()
            },
        );
    }
    assert_eq!(ALLOCS.load(Ordering::Relaxed) - before, 0);

    let mut car = Car::new(Arc::new(CarModel::gt3()), &track, 0.0, 0.0, 20.0, 2);
    let mut evolution = TrackEvolution::new(
        Arc::new(RubberMap::new(&track)),
        0.94,
        TrackEvolution::DEFAULT_GAIN_PER_LAP,
    );
    let weather = Weather::new(&track, None, WeatherSettings::default());
    car.step_in(&track, &mut evolution, &weather, &Controls::default());
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..1000 {
        car.step_in(&track, &mut evolution, &weather, &Controls::default());
    }
    assert_eq!(ALLOCS.load(Ordering::Relaxed) - before, 0);
}
