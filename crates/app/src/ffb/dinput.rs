//! DirectInput constant-force effect: the API every Windows wheel driver implements
//! force feedback for (Logitech, Thrustmaster, Fanatec, MOZA, Simucube, ...).

use std::ffi::c_void;

use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::{BOOL, GUID, Interface};

/// `MAKEDIPROP(9)`: DirectInput property ids are small integers cast to GUID pointers.
const DIPROP_AUTOCENTER_ID: usize = 9;
const INFINITE: u32 = u32::MAX;

/// An acquired force-feedback wheel playing one constant force on its X axis, and
/// sine vibrations on top of it where the device can play them.
/// Dropping it stops the forces and releases the device.
pub struct Wheel {
    _di: IDirectInput8W,
    device: IDirectInputDevice8W,
    effect: IDirectInputEffect,
    vibrations: Vec<Vibration>,
    pub name: String,
    magnitude: i32,
}

/// A sine effect and the magnitude and period (µs) it plays.
struct Vibration {
    effect: IDirectInputEffect,
    magnitude: u32,
    period: u32,
}

impl Wheel {
    /// Opens the attached force-feedback controller with these USB ids (or the first
    /// one when the ids are unknown) for exclusive use by the window `hwnd`.
    pub fn open(hwnd: isize, usb: Option<(u16, u16)>) -> Result<Self, String> {
        // SAFETY: plain COM calls; every pointer passed outlives the call it is passed to.
        unsafe {
            let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let mut raw: *mut c_void = std::ptr::null_mut();
            DirectInput8Create(
                module.into(),
                DIRECTINPUT_VERSION,
                &IDirectInput8W::IID,
                &mut raw,
                None,
            )
            .map_err(|e| e.to_string())?;
            let di = IDirectInput8W::from_raw(raw);

            let mut found: Vec<DIDEVICEINSTANCEW> = Vec::new();
            di.EnumDevices(
                DI8DEVCLASS_GAMECTRL,
                Some(collect),
                (&raw mut found).cast(),
                DIEDFL_ATTACHEDONLY | DIEDFL_FORCEFEEDBACK,
            )
            .map_err(|e| e.to_string())?;
            // guidProduct.data1 is MAKELONG(vendor, product).
            let instance = found
                .iter()
                .find(|d| {
                    usb.is_none_or(|(vendor, product)| {
                        d.guidProduct.data1 == (product as u32) << 16 | vendor as u32
                    })
                })
                .ok_or("no force-feedback device found for the steering input")?;
            let name = String::from_utf16_lossy(&instance.tszProductName)
                .trim_end_matches('\0')
                .to_owned();

            let mut device = None;
            di.CreateDevice(&instance.guidInstance, &mut device, None)
                .map_err(|e| e.to_string())?;
            let device = device.ok_or("CreateDevice returned no device")?;
            set_x_axis_format(&device)?;
            device
                .SetCooperativeLevel(
                    HWND(hwnd as *mut c_void),
                    DISCL_EXCLUSIVE | DISCL_BACKGROUND,
                )
                .map_err(|e| format!("{name}: {e}"))?;
            // The driver's centring spring would fight the simulated aligning torque.
            let mut autocenter = DIPROPDWORD {
                diph: DIPROPHEADER {
                    dwSize: size_of::<DIPROPDWORD>() as u32,
                    dwHeaderSize: size_of::<DIPROPHEADER>() as u32,
                    dwObj: 0,
                    dwHow: DIPH_DEVICE,
                },
                dwData: DIPROPAUTOCENTER_OFF,
            };
            let _ = device.SetProperty(DIPROP_AUTOCENTER_ID as *const GUID, &mut autocenter.diph);
            device.Acquire().map_err(|e| format!("{name}: {e}"))?;

            let mut axes = [0u32]; // DIJOFS_X
            let mut direction = [0i32];
            let mut force = DICONSTANTFORCE { lMagnitude: 0 };
            let mut params = DIEFFECT {
                dwSize: size_of::<DIEFFECT>() as u32,
                dwFlags: DIEFF_CARTESIAN | DIEFF_OBJECTOFFSETS,
                dwDuration: INFINITE,
                dwGain: DI_FFNOMINALMAX,
                dwTriggerButton: DIEB_NOTRIGGER,
                cAxes: 1,
                rgdwAxes: axes.as_mut_ptr(),
                rglDirection: direction.as_mut_ptr(),
                cbTypeSpecificParams: size_of::<DICONSTANTFORCE>() as u32,
                lpvTypeSpecificParams: (&raw mut force).cast(),
                ..Default::default()
            };
            let mut effect = None;
            device
                .CreateEffect(&GUID_ConstantForce, &mut params, &mut effect, None)
                .map_err(|e| format!("{name}: {e}"))?;
            let effect = effect.ok_or("CreateEffect returned no effect")?;
            effect.Start(1, 0).map_err(|e| format!("{name}: {e}"))?;
            // Optional: a wheel that cannot play them still gets the steering torque.
            let vibrations = (0..super::VIBRATIONS)
                .map_while(|_| create_sine(&device))
                .map(|effect| Vibration {
                    effect,
                    magnitude: 0,
                    period: 0,
                })
                .collect();
            Ok(Self {
                _di: di,
                device,
                effect,
                vibrations,
                name,
                magnitude: 0,
            })
        }
    }

    /// Sets the force in -1..1 of the motor's maximum; positive pushes towards +X
    /// (usually right).
    pub fn set(&mut self, force: f64) -> Result<(), String> {
        let magnitude = (force.clamp(-1.0, 1.0) * DI_FFNOMINALMAX as f64).round() as i32;
        if magnitude == self.magnitude {
            return Ok(());
        }
        let mut constant = DICONSTANTFORCE {
            lMagnitude: magnitude,
        };
        let mut params = DIEFFECT {
            dwSize: size_of::<DIEFFECT>() as u32,
            cbTypeSpecificParams: size_of::<DICONSTANTFORCE>() as u32,
            lpvTypeSpecificParams: (&raw mut constant).cast(),
            ..Default::default()
        };
        // SAFETY: `params` and `constant` outlive the calls.
        unsafe {
            if self
                .effect
                .SetParameters(&mut params, DIEP_TYPESPECIFICPARAMS)
                .is_err()
            {
                // Lost the device (another app took it, or it was unplugged): reacquire once.
                self.device.Acquire().map_err(|e| e.to_string())?;
                self.effect
                    .SetParameters(&mut params, DIEP_TYPESPECIFICPARAMS | DIEP_START)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.magnitude = magnitude;
        Ok(())
    }

    /// Plays vibration `i` with `magnitude` in 0..1 of the motor's maximum at `hz`.
    /// Does nothing where the device has no such vibration.
    pub fn vibrate(&mut self, i: usize, magnitude: f64, hz: f64) -> Result<(), String> {
        let Some(v) = self.vibrations.get_mut(i) else {
            return Ok(());
        };
        let magnitude = (magnitude.clamp(0.0, 1.0) * DI_FFNOMINALMAX as f64).round() as u32;
        // Whole hertz: each change of period may restart the wave on some drivers.
        let period = (1e6 / hz.round().max(1.0)) as u32;
        if magnitude == v.magnitude && (period == v.period || magnitude == 0) {
            return Ok(());
        }
        let mut periodic = DIPERIODIC {
            dwMagnitude: magnitude,
            lOffset: 0,
            dwPhase: 0,
            dwPeriod: period,
        };
        let mut params = DIEFFECT {
            dwSize: size_of::<DIEFFECT>() as u32,
            cbTypeSpecificParams: size_of::<DIPERIODIC>() as u32,
            lpvTypeSpecificParams: (&raw mut periodic).cast(),
            ..Default::default()
        };
        // SAFETY: `params` and `periodic` outlive the call.
        unsafe { v.effect.SetParameters(&mut params, DIEP_TYPESPECIFICPARAMS) }
            .map_err(|e| e.to_string())?;
        v.magnitude = magnitude;
        v.period = period;
        Ok(())
    }

    /// Restarts the effect if the device stopped it (MOZA's hands-off protection, a
    /// driver reset, ...), which updating its force alone does not undo. Returns
    /// whether it had stopped.
    pub fn keep_playing(&mut self) -> Result<bool, String> {
        let mut status = 0;
        // SAFETY: plain COM calls; `status` outlives them.
        unsafe {
            if self.effect.GetEffectStatus(&mut status).is_ok() && status & DIEGES_PLAYING != 0 {
                return Ok(false);
            }
            let _ = self.device.Acquire();
            self.effect.Start(1, 0).map_err(|e| e.to_string())?;
            for v in &self.vibrations {
                let _ = v.effect.Start(1, 0);
            }
        }
        Ok(true)
    }
}

impl Drop for Wheel {
    fn drop(&mut self) {
        // Some drivers (MOZA) keep playing the last force after the device is released,
        // even after the process exits, so zero it and reset the device first.
        let mut constant = DICONSTANTFORCE { lMagnitude: 0 };
        let mut params = DIEFFECT {
            dwSize: size_of::<DIEFFECT>() as u32,
            cbTypeSpecificParams: size_of::<DICONSTANTFORCE>() as u32,
            lpvTypeSpecificParams: (&raw mut constant).cast(),
            ..Default::default()
        };
        // SAFETY: plain COM calls on live interfaces; `params` outlives them.
        unsafe {
            for v in &self.vibrations {
                let _ = v.effect.Stop();
                let _ = v.effect.Unload();
            }
            let _ = self
                .effect
                .SetParameters(&mut params, DIEP_TYPESPECIFICPARAMS);
            let _ = self.effect.Stop();
            let _ = self.device.SendForceFeedbackCommand(DISFFC_RESET);
            let _ = self.effect.Unload();
            let _ = self.device.Unacquire();
        }
    }
}

/// A silent sine effect on the X axis, started; `None` if the device cannot play one.
fn create_sine(device: &IDirectInputDevice8W) -> Option<IDirectInputEffect> {
    let mut axes = [0u32]; // DIJOFS_X
    let mut direction = [0i32];
    let mut periodic = DIPERIODIC {
        dwMagnitude: 0,
        lOffset: 0,
        dwPhase: 0,
        dwPeriod: 20_000,
    };
    let mut params = DIEFFECT {
        dwSize: size_of::<DIEFFECT>() as u32,
        dwFlags: DIEFF_CARTESIAN | DIEFF_OBJECTOFFSETS,
        dwDuration: INFINITE,
        dwGain: DI_FFNOMINALMAX,
        dwTriggerButton: DIEB_NOTRIGGER,
        cAxes: 1,
        rgdwAxes: axes.as_mut_ptr(),
        rglDirection: direction.as_mut_ptr(),
        cbTypeSpecificParams: size_of::<DIPERIODIC>() as u32,
        lpvTypeSpecificParams: (&raw mut periodic).cast(),
        ..Default::default()
    };
    let mut effect = None;
    // SAFETY: plain COM calls; every pointer passed outlives the call it is passed to.
    unsafe {
        device
            .CreateEffect(&GUID_Sine, &mut params, &mut effect, None)
            .ok()?;
        let effect = effect?;
        effect.Start(1, 0).ok()?;
        Some(effect)
    }
}

unsafe extern "system" fn collect(instance: *mut DIDEVICEINSTANCEW, found: *mut c_void) -> BOOL {
    // SAFETY: DirectInput passes a valid instance, and `found` is the Vec given to EnumDevices.
    unsafe { (*found.cast::<Vec<DIDEVICEINSTANCEW>>()).push(*instance) };
    BOOL(DIENUM_CONTINUE as i32)
}

/// A device must have a data format before it can be acquired; effects address the
/// X axis by its offset in it.
unsafe fn set_x_axis_format(device: &IDirectInputDevice8W) -> Result<(), String> {
    let mut objects = [DIOBJECTDATAFORMAT {
        pguid: &GUID_XAxis,
        dwOfs: 0,
        dwType: DIDFT_AXIS | DIDFT_ANYINSTANCE,
        dwFlags: 0,
    }];
    let mut format = DIDATAFORMAT {
        dwSize: size_of::<DIDATAFORMAT>() as u32,
        dwObjSize: size_of::<DIOBJECTDATAFORMAT>() as u32,
        dwFlags: DIDF_ABSAXIS,
        dwDataSize: 4,
        dwNumObjs: objects.len() as u32,
        rgodf: objects.as_mut_ptr(),
    };
    // SAFETY: DirectInput copies the format during the call.
    unsafe { device.SetDataFormat(&mut format) }.map_err(|e| e.to_string())
}
