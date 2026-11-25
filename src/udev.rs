//! Standalone backend: InfinityDesk drives the screen and the input devices itself, started from
//! a console or a login manager with no other compositor underneath.
//!
//! One graphics card with all its monitors, each at its own resolution with the highest refresh
//! rate it offers there. Monitors can come and go while running.

use std::{error::Error, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmNode,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::{InputEvent, InputTime},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, ImportMemWl, element::solid::SolidColorBuffer, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus},
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
    utils::{DeviceFd, Logical, Point},
    wayland::dmabuf::DmabufFeedbackBuilder,
};

use crate::{
    InfinityDesk,
    config::{Config, OutputMode, OutputSettings},
    cursor::Cursors,
    render::{Scene, bar_elements, desk_elements, lock_elements},
};

/// With each frame, it keeps whether it was drawn while the screen is locked.
type Compositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    bool,
    DrmDeviceFd,
>;

/// The graphics card with its monitors, and the input devices.
pub struct Udev {
    session: LibSeatSession,
    libinput: Libinput,
    /// Connected input devices, so changed touchpad and mouse settings reach them.
    devices: Vec<InputDevice>,
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    render_node: Option<DrmNode>,
    pub renderer: GlesRenderer,
    screens: Vec<Screen>,
    handle: LoopHandle<'static, InfinityDesk>,
}

/// A connected monitor.
struct Screen {
    connector: connector::Handle,
    /// The display controller that drives it.
    crtc: crtc::Handle,
    output: Output,
    compositor: Compositor,
    scene: Scene,
    /// Buffers of the solid bars, kept so that unchanged outlines are not redrawn.
    bar_buffers: Vec<SolidColorBuffer>,
    /// Pointer images for its scale.
    cursors: Cursors,
    /// What the monitor can show (resolutions and refresh rates).
    modes: Vec<DrmMode>,
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
        for screen in &mut self.screens {
            let scale = screen.cursors.scale();
            screen.cursors =
                Cursors::load(config.cursor_theme.as_deref(), config.cursor_size, scale);
        }
    }

    /// Changed `desk.output` settings: resolution and refresh rate, scale, orientation.
    pub fn apply_output_settings(
        &mut self,
        output: &Output,
        new: &OutputSettings,
        config: &Config,
    ) {
        let Some(screen) = self.screens.iter_mut().find(|s| &s.output == output) else {
            return;
        };
        let mode = choose_mode(&screen.modes, new.mode);
        let wl_mode = WlMode::from(mode);
        if output.current_mode() != Some(wl_mode) {
            match screen.compositor.use_mode(mode) {
                Ok(()) => screen.frame_time = frame_time(wl_mode),
                Err(err) => tracing::warn!("Cannot switch the screen's mode: {err:?}"),
            }
        }
        let mode = screen.compositor.pending_mode();
        let scale = Scale::Fractional(new.scale);
        output.change_current_state(Some(mode.into()), Some(new.transform), Some(scale), None);
        let theme = config.cursor_theme.as_deref();
        screen.cursors = Cursors::load(theme, config.cursor_size, new.scale);
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

/// A graphics card with a monitor connected.
struct Gpu {
    drm: DrmDevice,
    notifier: DrmDeviceNotifier,
    fd: DrmDeviceFd,
}

/// Opens a graphics card; `None` if no monitor is connected to it.
fn open_gpu(session: &mut LibSeatSession, path: &Path) -> Result<Option<Gpu>, Box<dyn Error>> {
    let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
    let fd = DrmDeviceFd::new(DeviceFd::from(session.open(path, flags)?));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let connected = !connected(&drm)?.is_empty();
    Ok(connected.then_some(Gpu { drm, notifier, fd }))
}

/// The connectors a monitor is connected to.
fn connected(drm: &DrmDevice) -> std::io::Result<Vec<connector::Info>> {
    let res = drm.resource_handles()?;
    Ok((res.connectors().iter())
        .filter_map(|c| drm.get_connector(*c, true).ok())
        .filter(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
        .collect())
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
    let Gpu { drm, notifier, fd } = gpu.ok_or("no graphics card with a connected monitor found")?;

    let gbm = GbmDevice::new(fd)?;
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = (EGLDevice::device_for_display(&egl).ok())
        .and_then(|d| d.try_get_render_node().ok().flatten());
    let renderer = unsafe { GlesRenderer::new(EGLContext::new(&egl)?)? };

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
                for screen in &mut udev.screens {
                    if let Err(err) = screen.compositor.reset_state() {
                        tracing::warn!("Resetting the screen state failed: {err}");
                    }
                    screen.frame_pending = false;
                }
                state.screens_off = false;
                // Monitors may have come and gone meanwhile.
                state.update_screens();
                state.wake_screens();
            }
        }
    })?;

    // A frame reached the screen.
    handle.insert_source(notifier, |event, _, state| match event {
        DrmEvent::VBlank(crtc) => state.frame_shown(crtc),
        DrmEvent::Error(err) => tracing::warn!("Screen error: {err}"),
    })?;

    // Monitors are connected and disconnected.
    let device = drm.device_id();
    handle.insert_source(UdevBackend::new(session.seat())?, move |event, _, state| {
        // While another console is shown: when it is our turn again.
        let active = state.udev.as_ref().is_some_and(|u| u.session.is_active());
        if let UdevEvent::Changed { device_id } = event
            && device_id == device
            && active
        {
            state.update_screens();
        }
    })?;

    state.udev = Some(Udev {
        session,
        libinput,
        devices: Vec::new(),
        drm,
        gbm,
        render_node,
        renderer,
        screens: Vec::new(),
        handle,
    });
    state.update_screens();
    if state.monitor.output.is_none() {
        return Err("no screen could be used".into());
    }
    // The pointer starts in the middle of the first screen.
    state.pointer_moved(state.monitor.canvas.viewport_center(), InputTime::now());
    Ok(())
}

impl Udev {
    /// Sets up a newly connected monitor.
    fn add_screen(
        &mut self,
        connector: &connector::Info,
        config: &Config,
    ) -> Result<Output, Box<dyn Error>> {
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        // A display controller (CRTC) that can drive it and no other monitor uses.
        let res = self.drm.resource_handles()?;
        let crtc = (connector.encoders().iter())
            .filter_map(|e| self.drm.get_encoder(*e).ok())
            .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
            .find(|crtc| self.screens.iter().all(|s| s.crtc != *crtc))
            .ok_or("no display controller (CRTC) is free for it")?;

        let settings = config.output(&name);
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
        output.change_current_state(
            Some(wl_mode),
            Some(settings.transform),
            Some(Scale::Fractional(settings.scale)),
            Some((0, 0).into()),
        );
        output.set_preferred(wl_mode);

        let surface = (self.drm).create_surface(crtc, mode, &[connector.handle()])?;
        let mut planes = self.drm.planes(&crtc)?;
        // Overlay planes are not needed, and they break on NVIDIA.
        planes.overlay.clear();
        let allocator = GbmAllocator::new(
            self.gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let exporter = GbmFramebufferExporter::new(self.gbm.clone(), self.render_node.into());
        let formats = (self.renderer.egl_context().dmabuf_render_formats()).clone();
        let compositor = DrmCompositor::new(
            &output,
            surface,
            Some(planes),
            allocator,
            exporter,
            [Fourcc::Xrgb8888, Fourcc::Argb8888],
            formats,
            self.drm.cursor_size(),
            Some(self.gbm.clone()),
        )
        .map_err(|e| format!("cannot drive it: {e:?}"))?;
        self.screens.push(Screen {
            connector: connector.handle(),
            crtc,
            output: output.clone(),
            compositor,
            scene: Scene::new(&mut self.renderer),
            bar_buffers: Vec::new(),
            cursors: Cursors::load(
                config.cursor_theme.as_deref(),
                config.cursor_size,
                settings.scale,
            ),
            modes,
            frame_pending: false,
            redraw_planned: false,
            frame_time: frame_time(wl_mode),
        });
        Ok(output)
    }
}

impl InfinityDesk {
    /// Sets up monitors that were connected, and forgets disconnected ones.
    fn update_screens(&mut self) {
        let Some(udev) = &mut self.udev else {
            return;
        };
        let connectors = match connected(&udev.drm) {
            Ok(connectors) => connectors,
            Err(err) => return tracing::warn!("Looking for monitors failed: {err}"),
        };
        let is_gone = |s: &Screen| connectors.iter().all(|c| c.handle() != s.connector);
        let gone: Vec<Output> = (udev.screens.extract_if(.., |s| is_gone(s)))
            .map(|s| s.output)
            .collect();
        for output in gone {
            tracing::info!("Screen {}: disconnected", output.name());
            self.remove_output(&output);
        }
        for connector in connectors {
            let Some(udev) = &mut self.udev else {
                return;
            };
            if udev
                .screens
                .iter()
                .any(|s| s.connector == connector.handle())
            {
                continue;
            }
            match udev.add_screen(&connector, &self.config) {
                Ok(output) => {
                    let crtc = udev.screens.last().unwrap().crtc;
                    self.add_output(output);
                    self.render_screen(crtc);
                }
                Err(err) => {
                    let (name, id) = (connector.interface().as_str(), connector.interface_id());
                    tracing::warn!("Screen {name}-{id}: {err}");
                }
            }
        }
    }

    /// Draws all screens again (standalone), which also switches them on.
    pub fn wake_screens(&mut self) {
        let screens = self.udev.iter().flat_map(|udev| &udev.screens);
        let crtcs: Vec<crtc::Handle> = screens.map(|s| s.crtc).collect();
        for crtc in crtcs {
            self.render_screen(crtc);
        }
    }

    /// Switches all screens off (standalone); drawing them switches them on again. Whether it
    /// worked.
    pub fn darken_screens(&mut self) -> bool {
        let mut all = true;
        for screen in self.udev.iter_mut().flat_map(|udev| &mut udev.screens) {
            match screen.compositor.clear() {
                Ok(()) => screen.frame_pending = false,
                Err(err) => {
                    tracing::warn!("Switching a screen off failed: {err}");
                    all = false;
                }
            }
        }
        all
    }

    /// Draws the next frame of a screen if something changed. Then it reports when the frame is
    /// shown (`frame_shown`); if nothing changed, it is looked at again after one frame.
    fn render_screen(&mut self, crtc: crtc::Handle) {
        let Some(output) = self.screen(crtc).map(|s| s.output.clone()) else {
            return;
        };
        let Some(i) = self.monitor.position(&output) else {
            return;
        };
        self.step_animations();
        // The pointer is only on the current monitor.
        let pointer = (i == self.monitor.current()).then_some(self.pointer_screen);
        self.on_monitor(i, |state| state.draw_screen(crtc, &output, pointer));
    }

    /// The same, as part of `render_screen` on the screen's monitor.
    fn draw_screen(
        &mut self,
        crtc: crtc::Handle,
        output: &Output,
        pointer: Option<Point<f64, Logical>>,
    ) {
        let bars = self.bars();
        let outlines = self.outlines();
        let offsets = self.window_offsets();
        let upper_layers = self.upper_layers();
        let (locked, lock_surface) = (self.lock.is_some(), self.lock_surface());
        let background = self.background();
        let Some(udev) = self.udev.as_mut() else {
            return;
        };
        if self.redraw_all {
            self.redraw_all = false;
            for screen in &mut udev.screens {
                screen.compositor.reset_buffers();
            }
        }
        let Some(screen) = udev.screens.iter_mut().find(|s| s.crtc == crtc) else {
            return;
        };
        if screen.frame_pending || !udev.session.is_active() || self.screens_off {
            return;
        }
        let mut elements = match pointer {
            Some(pointer) => (screen.cursors).elements(
                &mut udev.renderer,
                pointer,
                self.cursor,
                &self.client_cursor,
            ),
            None => Vec::new(),
        };
        let scale = output.current_scale().fractional_scale();
        if locked {
            elements.extend(lock_elements(&mut udev.renderer, output, lock_surface));
        } else {
            elements.extend(bar_elements(&mut screen.bar_buffers, &bars, scale));
            elements.extend(desk_elements(
                &mut udev.renderer,
                output,
                &self.space,
                &self.monitor.canvas,
                &mut screen.scene,
                &self.config,
                upper_layers,
                &offsets,
                &outlines,
                self.monitor.fullscreen.as_ref().map(|f| &f.window),
            ));
        }
        let rendered = screen.compositor.render_frame(
            &mut udev.renderer,
            &elements,
            background,
            FrameFlags::DEFAULT,
        );
        let mut unchanged = false;
        screen.frame_pending = match rendered {
            Ok(frame) if !frame.is_empty => match screen.compositor.queue_frame(locked) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("Showing the frame failed: {err:?}");
                    false
                }
            },
            Ok(_) => {
                unchanged = true;
                false
            }
            Err(err) => {
                tracing::warn!("Drawing the frame failed: {err:?}");
                false
            }
        };
        if !screen.frame_pending && !screen.redraw_planned {
            screen.redraw_planned = true;
            let timer = Timer::from_duration(screen.frame_time);
            let planned = udev.handle.insert_source(timer, move |_, _, state| {
                if let Some(screen) = state.screen(crtc) {
                    screen.redraw_planned = false;
                }
                state.render_screen(crtc);
                TimeoutAction::Drop
            });
            if planned.is_err() {
                screen.redraw_planned = false;
            }
        }
        // What is shown already looks like that.
        if locked && unchanged {
            self.lock_shown(output);
        }
        self.send_frames(output);
    }

    fn screen(&mut self, crtc: crtc::Handle) -> Option<&mut Screen> {
        let udev = self.udev.as_mut()?;
        udev.screens.iter_mut().find(|s| s.crtc == crtc)
    }

    /// The last frame is on the screen now: on to the next one.
    fn frame_shown(&mut self, crtc: crtc::Handle) {
        if let Some(screen) = self.screen(crtc) {
            screen.frame_pending = false;
            match screen.compositor.frame_submitted() {
                // Drawn while locked.
                Ok(Some(true)) => {
                    let output = screen.output.clone();
                    self.lock_shown(&output);
                }
                Ok(_) => {}
                Err(err) => tracing::warn!("Frame bookkeeping failed: {err:?}"),
            }
        }
        self.render_screen(crtc);
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
