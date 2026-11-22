//! Standalone backend: InfinityDesk drives the screen and the input devices itself, started from
//! a console or a login manager with no other compositor underneath.
//!
//! Kept to what one monitor on one graphics card needs: the first connected monitor, at its own
//! resolution with the highest refresh rate it offers there. Several monitors come later.

use std::{error::Error, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::{InputEvent, InputTime},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, ImportMemWl, element::solid::SolidColorBuffer, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::all_gpus,
    },
    output::{Mode as WlMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop, LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as _, Mode as DrmMode, ModeTypeFlags, connector, crtc},
        input::{AccelProfile, Device as InputDevice, DeviceCapability, Libinput},
        rustix::fs::OFlags,
    },
    utils::DeviceFd,
    wayland::dmabuf::DmabufFeedbackBuilder,
};

use crate::{
    InfinityDesk,
    config::{Config, OutputMode, OutputSettings},
    cursor::Cursors,
    render::{Grid, bar_elements, desk_elements},
};

type Compositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// The screen, the graphics card and the input devices.
pub struct Udev {
    session: LibSeatSession,
    libinput: Libinput,
    /// Connected input devices, so changed touchpad and mouse settings reach them.
    devices: Vec<InputDevice>,
    drm: DrmDevice,
    pub renderer: GlesRenderer,
    compositor: Compositor,
    grid: Grid,
    /// Buffers of the solid bars, kept so that unchanged outlines are not redrawn.
    bar_buffers: Vec<SolidColorBuffer>,
    cursors: Cursors,
    /// What the monitor can show (resolutions and refresh rates).
    modes: Vec<DrmMode>,
    handle: LoopHandle<'static, InfinityDesk>,
    /// A frame is on its way to the screen; the next one is drawn when it got there.
    frame_pending: bool,
    /// Nothing had changed: a look again is planned (after `frame_time`).
    redraw_planned: bool,
    frame_time: Duration,
}

impl Udev {
    /// Ctrl+Alt+F1..F12: shows another console (e.g. the login manager or a text console).
    pub fn switch_vt(&mut self, vt: i32) {
        if let Err(err) = self.session.change_vt(vt) {
            tracing::warn!("Switching to console {vt} failed: {err}");
        }
    }

    /// Another pointer theme or size from the config.
    pub fn set_cursor_theme(&mut self, config: &Config) {
        let scale = self.cursors.scale();
        self.cursors = Cursors::load(config.cursor_theme.as_deref(), config.cursor_size, scale);
    }

    /// Changed `desk.output` settings: resolution and refresh rate, scale, orientation.
    pub fn apply_output_settings(
        &mut self,
        output: &Output,
        new: &OutputSettings,
        config: &Config,
    ) {
        let mode = choose_mode(&self.modes, new.mode);
        let wl_mode = WlMode::from(mode);
        if output.current_mode() != Some(wl_mode) {
            match self.compositor.use_mode(mode) {
                Ok(()) => self.frame_time = frame_time(wl_mode),
                Err(err) => tracing::warn!("Cannot switch the screen's mode: {err:?}"),
            }
        }
        let mode = self.compositor.pending_mode();
        let scale = Scale::Fractional(new.scale);
        output.change_current_state(Some(mode.into()), Some(new.transform), Some(scale), None);
        let theme = config.cursor_theme.as_deref();
        self.cursors = Cursors::load(theme, config.cursor_size, new.scale);
    }

    /// Applies the touchpad and mouse settings to every connected device.
    pub fn set_up_devices(&mut self, config: &Config) {
        for device in &mut self.devices {
            set_up_device(device, config);
        }
    }
}

/// Touchpad or mouse settings, depending on the device. Without settings, libinput leaves even
/// tapping off. Devices that cannot tap are no touchpads.
fn set_up_device(device: &mut InputDevice, config: &Config) {
    let profile = |flat| match flat {
        true => AccelProfile::Flat,
        false => AccelProfile::Adaptive,
    };
    let results = if device.config_tap_finger_count() > 0 {
        let t = &config.touchpad;
        vec![
            device.config_tap_set_enabled(t.tap),
            device.config_scroll_set_natural_scroll_enabled(t.natural_scroll),
            device.config_dwt_set_enabled(t.dwt),
            device.config_accel_set_speed(t.accel_speed),
            device.config_accel_set_profile(profile(t.accel_flat)),
        ]
    } else if device.has_capability(DeviceCapability::Pointer) {
        let m = &config.mouse;
        vec![
            device.config_scroll_set_natural_scroll_enabled(m.natural_scroll),
            device.config_left_handed_set(m.left_handed),
            device.config_accel_set_speed(m.accel_speed),
            device.config_accel_set_profile(profile(m.accel_flat)),
        ]
    } else {
        return;
    };
    if results.iter().any(|r| r.is_err()) {
        tracing::warn!("{}: not every setting is supported", device.name());
    }
}

/// A graphics card with a monitor connected, and the display controller (CRTC) for it.
struct Gpu {
    drm: DrmDevice,
    notifier: DrmDeviceNotifier,
    fd: DrmDeviceFd,
    connector: connector::Info,
    crtc: crtc::Handle,
}

/// Opens a graphics card; `None` if no monitor is connected to it.
fn open_gpu(session: &mut LibSeatSession, path: &Path) -> Result<Option<Gpu>, Box<dyn Error>> {
    let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
    let fd = DrmDeviceFd::new(DeviceFd::from(session.open(path, flags)?));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let res = drm.resource_handles()?;
    let connected = (res.connectors().iter())
        .filter_map(|c| drm.get_connector(*c, true).ok())
        .find(|c| c.state() == connector::State::Connected && !c.modes().is_empty());
    let Some(connector) = connected else {
        return Ok(None);
    };
    let crtc = (connector.encoders().iter())
        .filter_map(|e| drm.get_encoder(*e).ok())
        .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
        .next()
        .ok_or("no display controller (CRTC) for the monitor")?;
    Ok(Some(Gpu {
        drm,
        notifier,
        fd,
        connector,
        crtc,
    }))
}

pub fn init_udev(
    event_loop: &mut EventLoop<'static, InfinityDesk>,
    state: &mut InfinityDesk,
) -> Result<(), Box<dyn Error>> {
    let (mut session, session_notifier) = LibSeatSession::new()
        .map_err(|e| format!("no access to the seat (start from a console): {e}"))?;

    // The graphics card with a monitor connected (`INFINITYDESK_DRM_DEVICE` picks one).
    let paths = match std::env::var_os("INFINITYDESK_DRM_DEVICE") {
        Some(path) => vec![path.into()],
        None => all_gpus(session.seat())?,
    };
    let mut gpu = None;
    for path in paths {
        match open_gpu(&mut session, &path) {
            Ok(Some(found)) => {
                tracing::info!("Using the graphics card {}", path.display());
                gpu = Some(found);
                break;
            }
            Ok(None) => tracing::info!("{}: no monitor connected", path.display()),
            Err(err) => tracing::warn!("{}: {err}", path.display()),
        }
    }
    let Gpu {
        mut drm,
        notifier,
        fd,
        connector,
        crtc,
    } = gpu.ok_or("no graphics card with a connected monitor found")?;

    let gbm = GbmDevice::new(fd)?;
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = (EGLDevice::device_for_display(&egl).ok())
        .and_then(|d| d.try_get_render_node().ok().flatten());
    let mut renderer = unsafe { GlesRenderer::new(EGLContext::new(&egl)?)? };

    let name = format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    );
    let settings = state.config.output(&name);
    let modes = connector.modes().to_vec();
    let mode = choose_mode(&modes, settings.mode);
    let wl_mode = WlMode::from(mode);
    tracing::info!(
        "Screen {name}: {}x{} at {:.2} Hz",
        wl_mode.size.w,
        wl_mode.size.h,
        wl_mode.refresh as f64 / 1000.0
    );
    let (w, h) = connector.size().unwrap_or((0, 0));
    let output = Output::new(
        name,
        PhysicalProperties {
            size: (w as i32, h as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "Unknown".into(),
            model: "Unknown".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<InfinityDesk>(&state.display_handle);
    output.change_current_state(
        Some(wl_mode),
        Some(settings.transform),
        Some(Scale::Fractional(settings.scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(wl_mode);

    let surface = drm.create_surface(crtc, mode, &[connector.handle()])?;
    let mut planes = drm.planes(&crtc)?;
    // Overlay planes are not needed, and they break on NVIDIA.
    planes.overlay.clear();
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let compositor = DrmCompositor::new(
        &output,
        surface,
        Some(planes),
        allocator,
        exporter,
        [Fourcc::Xrgb8888, Fourcc::Argb8888],
        render_formats,
        drm.cursor_size(),
        Some(gbm),
    )
    .map_err(|e| format!("cannot drive the screen: {e:?}"))?;

    // What apps may send: pixel buffers in memory, and GPU buffers (dmabuf).
    state.shm_state.update_formats(renderer.shm_formats());
    match render_node {
        Some(node) => {
            let feedback =
                DmabufFeedbackBuilder::new(node.dev_id(), renderer.dmabuf_formats()).build()?;
            (state.dmabuf_state).create_global_with_default_feedback::<InfinityDesk>(
                &state.display_handle,
                &feedback,
            );
        }
        None => tracing::warn!("No render node: apps cannot hand over GPU buffers"),
    }

    // Keyboards, mice and touchpads.
    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    (libinput.udev_assign_seat(&session.seat()))
        .map_err(|()| "cannot take the input devices of the seat")?;
    let handle = event_loop.handle();
    handle.insert_source(
        LibinputInputBackend::new(libinput.clone()),
        |mut event, _, state| {
            if let Some(udev) = &mut state.udev {
                match &mut event {
                    InputEvent::DeviceAdded { device } => {
                        set_up_device(device, &state.config);
                        udev.devices.push(device.clone());
                    }
                    InputEvent::DeviceRemoved { device } => udev.devices.retain(|d| d != device),
                    _ => {}
                }
            }
            state.process_input_event(event)
        },
    )?;

    // Another console was shown, and back.
    handle.insert_source(session_notifier, |event, _, state| {
        let Some(udev) = &mut state.udev else {
            return;
        };
        match event {
            SessionEvent::PauseSession => {
                tracing::info!("Paused: another console is shown");
                udev.libinput.suspend();
                udev.drm.pause();
            }
            SessionEvent::ActivateSession => {
                tracing::info!("Back from another console");
                if udev.libinput.resume().is_err() {
                    tracing::warn!("The input devices could not be taken back");
                }
                if let Err(err) = udev.drm.activate(false) {
                    tracing::warn!("The screen could not be taken back: {err}");
                }
                // The other console may have changed the screen setup.
                if let Err(err) = udev.compositor.reset_state() {
                    tracing::warn!("Resetting the screen state failed: {err}");
                }
                udev.frame_pending = false;
                state.render_screen();
            }
        }
    })?;

    // A frame reached the screen.
    handle.insert_source(notifier, |event, _, state| match event {
        DrmEvent::VBlank(_) => state.frame_shown(),
        DrmEvent::Error(err) => tracing::warn!("Screen error: {err}"),
    })?;

    let grid = Grid::new(&mut renderer);
    state.udev = Some(Udev {
        session,
        libinput,
        devices: Vec::new(),
        drm,
        renderer,
        compositor,
        grid,
        bar_buffers: Vec::new(),
        cursors: Cursors::load(
            state.config.cursor_theme.as_deref(),
            state.config.cursor_size,
            settings.scale,
        ),
        modes,
        handle,
        frame_pending: false,
        redraw_planned: false,
        frame_time: frame_time(wl_mode),
    });
    state.set_output(output);
    // The pointer starts in the middle of the screen.
    state.pointer_moved(state.canvas.viewport_center(), InputTime::now());
    state.render_screen();
    Ok(())
}

impl InfinityDesk {
    /// Draws the next frame if something changed. Then the screen reports when it is shown
    /// (`frame_shown`); if nothing changed, it is looked at again after one frame.
    pub fn render_screen(&mut self) {
        let Some(output) = self.output.clone() else {
            return;
        };
        self.step_animations();
        let bars = self.bars();
        let offsets = self.window_offsets();
        let upper_layers = self.upper_layers();
        let pointer = self.pointer_screen;
        let Some(udev) = self.udev.as_mut() else {
            return;
        };
        if udev.frame_pending || !udev.session.is_active() {
            return;
        }
        let mut elements = (udev.cursors).elements(
            &mut udev.renderer,
            pointer,
            self.cursor,
            &self.client_cursor,
        );
        let scale = output.current_scale().fractional_scale();
        elements.extend(bar_elements(&mut udev.bar_buffers, &bars, scale));
        elements.extend(desk_elements(
            &mut udev.renderer,
            &output,
            &self.space,
            &self.canvas,
            &mut udev.grid,
            &self.config,
            upper_layers,
            &offsets,
        ));
        let rendered = udev.compositor.render_frame(
            &mut udev.renderer,
            &elements,
            self.config.background,
            FrameFlags::DEFAULT,
        );
        udev.frame_pending = match rendered {
            Ok(frame) if !frame.is_empty => match udev.compositor.queue_frame(()) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("Showing the frame failed: {err:?}");
                    false
                }
            },
            Ok(_) => false,
            Err(err) => {
                tracing::warn!("Drawing the frame failed: {err:?}");
                false
            }
        };
        if !udev.frame_pending && !udev.redraw_planned {
            udev.redraw_planned = true;
            let timer = Timer::from_duration(udev.frame_time);
            let planned = udev.handle.insert_source(timer, |_, _, state| {
                if let Some(udev) = &mut state.udev {
                    udev.redraw_planned = false;
                }
                state.render_screen();
                TimeoutAction::Drop
            });
            if planned.is_err() {
                udev.redraw_planned = false;
            }
        }
        self.send_frames(&output);
    }

    /// The last frame is on the screen now: on to the next one.
    fn frame_shown(&mut self) {
        if let Some(udev) = &mut self.udev {
            if let Err(err) = udev.compositor.frame_submitted() {
                tracing::warn!("Frame bookkeeping failed: {err:?}");
            }
            udev.frame_pending = false;
        }
        self.render_screen();
    }
}

/// The mode from the config (at its resolution, the refresh rate closest to the wanted one, or
/// the highest), else the monitor's own resolution at the highest refresh rate it offers there.
fn choose_mode(modes: &[DrmMode], wanted: Option<OutputMode>) -> DrmMode {
    let preferred = (modes.iter())
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(&modes[0]);
    let size = wanted.map_or(preferred.size(), |w| (w.width as u16, w.height as u16));
    let refresh = |m: &&DrmMode| WlMode::from(**m).refresh;
    let at_size = modes.iter().filter(|m| m.size() == size);
    let best = match wanted.and_then(|w| w.refresh) {
        Some(hz) => at_size.min_by_key(|m| (refresh(m) - (hz * 1000.0).round() as i32).abs()),
        None => at_size.max_by_key(refresh),
    };
    match best {
        Some(mode) => *mode,
        None => {
            tracing::warn!(
                "The monitor has no mode {}x{}, using its own",
                size.0,
                size.1
            );
            choose_mode(modes, None)
        }
    }
}

/// How long a frame is shown at this refresh rate.
fn frame_time(mode: WlMode) -> Duration {
    Duration::from_secs_f64(1000.0 / mode.refresh as f64)
}
