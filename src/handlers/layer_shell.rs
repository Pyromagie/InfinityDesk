//! wlr-layer-shell: launchers (fuzzel), bars, wallpapers. They live on the screen, not on the canvas.

use smithay::{
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::SERIAL_COUNTER,
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
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // There is only one output for now.
        if let Some(output) = &self.output {
            layer_map_for_output(output)
                .map_layer(&LayerSurface::new(surface, namespace))
                .unwrap();
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let Some(output) = &self.output else { return };
        let mut map = layer_map_for_output(output);
        let layer = map
            .layers()
            .find(|l| l.layer_surface() == &surface)
            .cloned();
        if let Some(layer) = layer {
            map.unmap_layer(&layer);
        }
        drop(map);

        // Give the keyboard back to the topmost window (e.g. after the launcher closed).
        match self.space.elements().last().cloned() {
            Some(window) => self.focus_window(&window),
            None => {
                let keyboard = self.seat.get_keyboard().unwrap();
                keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
            }
        }
    }
}

impl InfinityDesk {
    /// Must be called on every commit: arranges layer surfaces and sends their initial configure.
    pub fn handle_layer_commit(&mut self, surface: &WlSurface) {
        let Some(output) = &self.output else { return };
        let mut map = layer_map_for_output(output);
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
