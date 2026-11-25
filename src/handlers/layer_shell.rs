//! wlr-layer-shell: launchers (fuzzel), bars, wallpapers. They live on the screen, not on the canvas.

use smithay::{
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::{IsAlive, SERIAL_COUNTER},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
            WlrLayerShellHandler, WlrLayerShellState,
        },
    },
};

use crate::InfinityDesk;

impl WlrLayerShellHandler for InfinityDesk {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // On the monitor it asks for (if still connected), else on the current one.
        let wanted = (output.as_ref().and_then(Output::from_resource))
            .filter(|wanted| self.monitor.outputs().any(|o| o == wanted));
        match wanted.as_ref().or(self.monitor.output.as_ref()) {
            Some(output) => layer_map_for_output(output)
                .map_layer(&LayerSurface::new(surface, namespace))
                .unwrap(),
            None => surface.send_close(),
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        for output in self.monitor.outputs() {
            let mut map = layer_map_for_output(output);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                map.unmap_layer(&layer);
            }
        }

        // Give the keyboard back to the window that was active (e.g. after the launcher closed),
        // unless another surface has it (e.g. a wallpaper tool restarted while the launcher is open).
        let keyboard = self.seat.get_keyboard().unwrap();
        let focus = keyboard.current_focus();
        if focus.is_some_and(|f| f.alive() && &f != surface.wl_surface()) {
            return;
        }
        match self.active.clone() {
            Some(window) => self.focus_window(&window),
            None => keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial()),
        }
    }
}

impl InfinityDesk {
    /// Must be called on every commit: arranges layer surfaces and sends their initial configure.
    pub fn handle_layer_commit(&mut self, surface: &WlSurface) {
        let Some(output) = self.layer_output(surface) else {
            return;
        };
        let mut map = layer_map_for_output(&output);
        let Some(layer) = map
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .cloned()
        else {
            return;
        };
        map.arrange();
        drop(map);

        let initial_configure_sent = with_states(surface, |states| {
            let data = states.data_map.get::<LayerSurfaceData>().unwrap();
            data.lock().unwrap().initial_configure_sent
        });
        if initial_configure_sent {
            return;
        }
        layer.layer_surface().send_configure();

        // Launchers ask for exclusive keyboard focus.
        if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive {
            let keyboard = self.seat.get_keyboard().unwrap();
            keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        }
    }
}
