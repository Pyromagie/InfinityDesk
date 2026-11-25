//! Who gets the keyboard: the screen lock, a launcher that wants it all, a bar or menu that was
//! clicked, or the active window.

use smithay::{
    desktop::{LayerSurface, layer_map_for_output},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
};

use crate::InfinityDesk;

impl InfinityDesk {
    /// Gives the keyboard to the surface that should have it now.
    pub fn refocus(&mut self) {
        let wanted = match self.lock {
            // Never to anything else while the screen is locked.
            Some(_) => self.lock_surface(),
            None => (self.exclusive_layer())
                .or_else(|| self.layer_focus.clone().filter(|s| s.alive()))
                .or_else(|| Some(self.active.as_ref()?.toplevel()?.wl_surface().clone())),
        };
        let keyboard = self.seat.get_keyboard().unwrap();
        if keyboard.current_focus() != wanted {
            keyboard.set_focus(self, wanted, SERIAL_COUNTER.next_serial());
        }
    }

    /// The surface of the screen lock on the current monitor.
    pub fn lock_surface(&self) -> Option<WlSurface> {
        let surface = self.monitor.lock_surface.as_ref()?;
        surface.alive().then(|| surface.wl_surface().clone())
    }

    /// A launcher (or the like) that wants the keyboard as long as it is open.
    fn exclusive_layer(&self) -> Option<WlSurface> {
        let current = self.monitor.output.iter();
        [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| {
            (current.clone().chain(self.monitor.outputs())).find_map(|output| {
                let map = layer_map_for_output(output);
                let exclusive = |l: &&LayerSurface| {
                    let wants = l.cached_state().keyboard_interactivity;
                    wants == KeyboardInteractivity::Exclusive
                };
                let found = map.layers_on(layer).rev().find(exclusive);
                found.map(|l| l.wl_surface().clone())
            })
        })
    }

    /// A click at `pos` on a bar, menu or wallpaper that takes the keyboard when clicked gives it
    /// the keyboard, any other click takes it away again.
    pub fn click_layer(&mut self, pos: Point<f64, Logical>, layers: &[Layer]) {
        let screen = self.monitor.canvas.canvas_to_screen(pos);
        let under = self.monitor.output.as_ref().and_then(|output| {
            let map = layer_map_for_output(output);
            let layer = layers.iter().find_map(|&l| map.layer_under(l, screen))?;
            (layer.can_receive_keyboard_focus()).then(|| layer.wl_surface().clone())
        });
        self.layer_focus = under;
        self.refocus();
    }
}
