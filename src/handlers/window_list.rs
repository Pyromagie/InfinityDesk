//! wlr-foreign-toplevel-management: bars and docks get the list of windows and can activate,
//! close and fullscreen them.

use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, State, ZwlrForeignToplevelHandleV1 as Handle},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1 as Manager},
        },
        wayland_server::{
            Client, DataInit, DisplayHandle, New, Resource, protocol::wl_output::WlOutput,
        },
    },
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::{InfinityDesk, handlers::app_id_and_title};

/// Version 3 knows the parents of dialogs.
pub const VERSION: u32 = 3;

/// User data of the global and its objects.
pub struct WindowListData;

/// The programs watching the windows, and what they know.
#[derive(Default)]
pub struct WindowList {
    managers: Vec<Manager>,
    entries: Vec<Entry>,
}

/// A window as one program knows it.
struct Entry {
    window: Window,
    manager: Manager,
    handle: Handle,
    told: Option<Info>,
}

#[derive(PartialEq)]
struct Info {
    app_id: String,
    title: String,
    activated: bool,
    fullscreen: bool,
    /// The monitors it can be seen on.
    outputs: Vec<Output>,
    parent: Option<Window>,
}

impl GlobalDispatch2<Manager, InfinityDesk> for WindowListData {
    fn bind(
        &self,
        state: &mut InfinityDesk,
        _: &DisplayHandle,
        _: &Client,
        manager: New<Manager>,
        data_init: &mut DataInit<'_, InfinityDesk>,
    ) {
        let manager = data_init.init(manager, WindowListData);
        state.window_list.managers.push(manager);
    }
}

impl Dispatch2<Manager, InfinityDesk> for WindowListData {
    fn request(
        &self,
        _: &mut InfinityDesk,
        _: &Client,
        manager: &Manager,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, InfinityDesk>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            // Ends the manager; `update_window_list` forgets it.
            manager.finished();
        }
    }
}

impl Dispatch2<Handle, InfinityDesk> for WindowListData {
    fn request(
        &self,
        state: &mut InfinityDesk,
        _: &Client,
        handle: &Handle,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, InfinityDesk>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        let mut entries = state.window_list.entries.iter();
        let Some(entry) = entries.find(|e| &e.handle == handle) else {
            return;
        };
        let window = entry.window.clone();
        match request {
            Request::Activate { .. } => state.go_to_window(&window),
            Request::Close => window.toplevel().unwrap().send_close(),
            Request::SetFullscreen { .. } if state.fullscreen_monitor(&window).is_none() => {
                state.start_fullscreen(&window);
            }
            Request::UnsetFullscreen => {
                state.end_fullscreen_of(&window);
            }
            // Windows are not maximized or minimized.
            _ => {}
        }
    }
}

impl InfinityDesk {
    /// Tells the programs watching what changed; called regularly.
    pub fn update_window_list(&mut self) {
        let mut list = std::mem::take(&mut self.window_list);
        list.managers.retain(|m| m.is_alive());
        list.entries.retain(|e| {
            let open = self.space.element_geometry(&e.window).is_some();
            if !open {
                e.handle.closed();
            }
            // Told to stop, the program still hears about the windows it knows.
            open && (e.manager.is_alive() || e.handle.is_alive())
        });
        // First all windows, so that dialogs can name their parent.
        for manager in &list.managers {
            for window in self.space.elements() {
                let mut entries = list.entries.iter();
                if entries.any(|e| &e.manager == manager && &e.window == window) {
                    continue;
                }
                let dh = &self.display_handle;
                let handle = manager.client().and_then(|client| {
                    let version = manager.version();
                    (client.create_resource::<Handle, _, Self>(dh, version, WindowListData)).ok()
                });
                if let Some(handle) = handle {
                    manager.toplevel(&handle);
                    list.entries.push(Entry {
                        window: window.clone(),
                        manager: manager.clone(),
                        handle,
                        told: None,
                    });
                }
            }
        }
        for i in 0..list.entries.len() {
            let entry = &list.entries[i];
            let info = self.listed_info(&entry.window);
            if entry.told.as_ref() == Some(&info) {
                continue;
            }
            let parent = list
                .entries
                .iter()
                .find(|e| e.manager == entry.manager && info.parent.as_ref() == Some(&e.window));
            entry.tell(&info, parent.map(|p| &p.handle));
            list.entries[i].told = Some(info);
        }
        self.window_list = list;
    }

    fn listed_info(&self, window: &Window) -> Info {
        let toplevel = window.toplevel().unwrap();
        let (app_id, title) = app_id_and_title(toplevel);
        let rect = self.space.element_geometry(window).unwrap_or_default();
        let monitors = self.monitor.list.iter();
        let seen = monitors.filter(|m| m.canvas.viewport().overlaps(rect.to_f64()));
        Info {
            app_id,
            title,
            activated: self.active.as_ref() == Some(window),
            fullscreen: self.fullscreen_monitor(window).is_some(),
            outputs: seen.filter_map(|m| m.output.clone()).collect(),
            parent: (toplevel.parent()).and_then(|p| self.window_for_surface(&p)),
        }
    }

    /// A program learns about a monitor: also which windows it already knows are there.
    pub fn window_list_output(&self, output: &Output, wl_output: &WlOutput) {
        let client = wl_output.client();
        for entry in &self.window_list.entries {
            let there = (entry.told.as_ref()).is_some_and(|info| info.outputs.contains(output));
            if there && entry.handle.client() == client {
                entry.handle.output_enter(wl_output);
                entry.handle.done();
            }
        }
    }
}

impl Entry {
    /// Tells what is new; everything the first time.
    fn tell(&self, info: &Info, parent: Option<&Handle>) {
        let Some(client) = self.handle.client() else {
            return;
        };
        let handle = &self.handle;
        let told = self.told.as_ref();
        if told.is_none_or(|t| t.app_id != info.app_id) {
            handle.app_id(info.app_id.clone());
        }
        if told.is_none_or(|t| t.title != info.title) {
            handle.title(info.title.clone());
        }
        let before = told.map_or(&[][..], |t| &t.outputs);
        for output in before.iter().filter(|o| !info.outputs.contains(o)) {
            for wl_output in output.client_outputs(&client) {
                handle.output_leave(&wl_output);
            }
        }
        for output in info.outputs.iter().filter(|o| !before.contains(o)) {
            for wl_output in output.client_outputs(&client) {
                handle.output_enter(&wl_output);
            }
        }
        let same = |t: &Info| (t.activated, t.fullscreen) == (info.activated, info.fullscreen);
        if !told.is_some_and(same) {
            let mut states = Vec::new();
            if info.activated {
                states.extend((State::Activated as u32).to_ne_bytes());
            }
            if info.fullscreen && handle.version() >= 2 {
                states.extend((State::Fullscreen as u32).to_ne_bytes());
            }
            handle.state(states);
        }
        if handle.version() >= 3 && told.is_none_or(|t| t.parent != info.parent) {
            handle.parent(parent);
        }
        handle.done();
    }
}
