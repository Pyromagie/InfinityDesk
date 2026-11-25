//! ext-session-lock: a program (swaylock, hyprlock) locks the screen. Until it unlocks, only its
//! surfaces are shown and get input, also if it crashes: then another one can take over.

use smithay::{
    backend::input::InputTime,
    input::pointer::CursorIcon,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{Resource, protocol::wl_output::WlOutput},
    },
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};

use crate::{InfinityDesk, monitor::screen_size};

/// The screen is locked.
pub struct Lock {
    /// The program that locked it.
    client: ExtSessionLockV1,
    /// It is told once every monitor shows nothing else any more.
    waiting: Option<SessionLocker>,
    /// The monitors that show nothing else.
    shown: Vec<Output>,
}

impl SessionLockHandler for InfinityDesk {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // Locked already, unless by a program that is gone.
        if self.lock.as_ref().is_some_and(|l| l.client.is_alive()) {
            return;
        }
        tracing::info!("The screen is locked");
        // Whatever is being dragged or open ends.
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), InputTime::now());
        keyboard.unset_grab(self);
        self.end_switching();
        self.gesture = None;
        self.resize_hint.clear();
        self.cursor = CursorIcon::Default;
        for monitor in &mut self.monitor.list {
            monitor.layout_menu = None;
            monitor.lock_surface = None;
        }
        self.lock = Some(Lock {
            client: confirmation.ext_session_lock().clone(),
            waiting: Some(confirmation),
            shown: Vec::new(),
        });
        self.lock_changed();
        self.confirm_lock();
    }

    fn unlock(&mut self) {
        tracing::info!("The screen is unlocked");
        self.lock = None;
        for monitor in &mut self.monitor.list {
            monitor.lock_surface = None;
        }
        self.lock_changed();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let ours = (self.lock.as_ref()).is_some_and(|l| &l.client == surface.ext_session_lock());
        let i = Output::from_resource(&output).and_then(|o| self.monitor.position(&o));
        if let Some(i) = i.filter(|_| ours) {
            self.monitor.list[i].lock_surface = Some(surface);
            self.size_lock_surface(i);
            self.lock_changed();
        }
    }
}

impl InfinityDesk {
    /// The keyboard and the pointer go to what is there now.
    fn lock_changed(&mut self) {
        self.refocus();
        let location = (self.monitor.canvas).screen_to_canvas(self.pointer_screen);
        self.pointer_moved(location, InputTime::now());
    }

    /// The lock surface covers its monitor (also when the monitor changes its size).
    pub fn size_lock_surface(&self, i: usize) {
        let monitor = &self.monitor.list[i];
        if let (Some(surface), Some(output)) = (&monitor.lock_surface, &monitor.output) {
            let size = screen_size(output);
            surface.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
            surface.send_configure();
        }
    }

    /// A frame drawn while the screen was locked is really on a monitor now.
    pub fn lock_shown(&mut self, output: &Output) {
        if let Some(lock) = &mut self.lock
            && lock.waiting.is_some()
            && !lock.shown.contains(output)
        {
            lock.shown.push(output.clone());
            self.confirm_lock();
        }
    }

    /// Once all monitors show the lock (or are off), the program learns that the screen is locked.
    pub fn confirm_lock(&mut self) {
        let Some(lock) = &mut self.lock else {
            return;
        };
        if (self.screens_off || self.monitor.outputs().all(|o| lock.shown.contains(o)))
            && let Some(confirmation) = lock.waiting.take()
        {
            confirmation.lock();
        }
    }
}
