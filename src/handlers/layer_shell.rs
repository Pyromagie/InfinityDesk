//! wlr-layer-shell: launchers (fuzzel), bars, wallpapers. They live on the screen, not on the canvas.

use smithay::{
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
            WlrLayerShellState,
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

        if self.layer_focus.as_ref() == Some(surface.wl_surface()) {
            self.layer_focus = None;
        }
        // The keyboard goes back to the active window (e.g. after the launcher closed).
        self.refocus();
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
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
        }
        // It may want the keyboard now, or no longer.
        if self.layer_focus.as_ref() == Some(surface) && !layer.can_receive_keyboard_focus() {
            self.layer_focus = None;
        }
        self.refocus();
    }
}
