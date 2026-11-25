//! Nobody is at the computer: programs (hypridle, swayidle) are told after the time they ask for,
//! unless a visible program (a video player) asks to stay awake. Screens can be switched off until
//! the next input.

use smithay::{
    backend::input::{
        ButtonState, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerButtonEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::IsAlive,
    wayland::{
        compositor::get_parent,
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
    },
};

use crate::InfinityDesk;

impl IdleNotifierHandler for InfinityDesk {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier
    }
}

impl IdleInhibitHandler for InfinityDesk {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        // One of them: a program may have asked several times.
        if let Some(i) = self.idle_inhibitors.iter().position(|s| *s == surface) {
            self.idle_inhibitors.swap_remove(i);
        }
    }
}

impl InfinityDesk {
    /// Every input shows that somebody is there.
    pub fn activity<I: InputBackend>(&mut self, event: &InputEvent<I>) {
        let seat = self.seat.clone();
        self.idle_notifier.notify_activity(&seat);
        // Letting go the key or button that switched the screens off does not count.
        let wakes = match event {
            InputEvent::Keyboard { event, .. } => event.state() == KeyState::Pressed,
            InputEvent::PointerButton { event, .. } => event.state() == ButtonState::Pressed,
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } => false,
            _ => true,
        };
        if self.screens_off && wakes {
            self.screens_off = false;
            self.redraw_all = true;
            self.wake_screens();
        }
    }

    /// Switches all screens off until the next input (not nested: the screen is not ours there).
    pub fn switch_screens_off(&mut self) {
        if self.udev.is_some() || self.headless {
            self.screens_off = self.darken_screens();
            match self.screens_off {
                // Nothing is shown any more.
                true => self.confirm_lock(),
                false => self.wake_screens(),
            }
        }
    }

    /// Whether a program that asks to stay awake can be seen; called regularly.
    pub fn update_idle_inhibit(&mut self) {
        self.idle_inhibitors.retain(|s| s.alive());
        let inhibited = self.idle_inhibitors.iter().any(|s| self.is_visible(s));
        self.idle_notifier.set_is_inhibited(inhibited);
    }

    /// On a window in sight, a bar or the like; while the screen is locked, only on the lock.
    fn is_visible(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if self.lock.is_some() {
            let mut locks = self
                .monitor
                .list
                .iter()
                .filter_map(|m| m.lock_surface.as_ref());
            return locks.any(|s| s.wl_surface() == &root);
        }
        if self.layer_output(&root).is_some() {
            return true;
        }
        let window = self.window_for_surface(&root);
        let Some(geo) = window.and_then(|w| self.space.element_geometry(&w)) else {
            return false;
        };
        let mut monitors = self.monitor.list.iter().filter(|m| m.output.is_some());
        monitors.any(|m| m.canvas.viewport().overlaps(geo.to_f64()))
    }
}
