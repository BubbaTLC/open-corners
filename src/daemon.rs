use crate::actions;
use crate::config::{Action, AppConfig, Corner};
use crate::service;
use anyhow::{Context, Result};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use log::{error, info, warn};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use cosmic_protocols::{
    toplevel_info::v1::client::{
        zcosmic_toplevel_handle_v1::{self, ZcosmicToplevelHandleV1},
        zcosmic_toplevel_info_v1::{self, ZcosmicToplevelInfoV1},
    },
    toplevel_management::v1::client::zcosmic_toplevel_manager_v1::{
        self, ZcosmicToplevelManagerV1,
    },
};

struct CornerZone {
    corner: Corner,
    output: wl_output::WlOutput,
    layer: LayerSurface,
    width: u32,
    height: u32,
    configured: bool,
}

#[derive(Default)]
struct ToplevelInfo {
    minimized: bool,
}

struct DaemonState {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,
    pool: SlotPool,
    loop_handle: LoopHandle<'static, DaemonState>,
    qh: QueueHandle<DaemonState>,

    config: AppConfig,
    config_path: PathBuf,
    config_stamp: Option<(SystemTime, u64)>,

    zones: Vec<CornerZone>,
    pointers: Vec<wl_pointer::WlPointer>,

    hover_gen: u64,
    active_corner: Option<Corner>,
    last_triggered: HashMap<Corner, Instant>,

    // Optional wlr foreign-toplevel support for native Show Desktop toggle
    foreign_toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<u32, (ZwlrForeignToplevelHandleV1, ToplevelInfo)>,
    minimized_by_us: Vec<u32>,

    // Optional COSMIC toplevel management support for native Show Desktop toggle on COSMIC
    _ext_toplevel_list: Option<ExtForeignToplevelListV1>,
    cosmic_toplevel_info: Option<ZcosmicToplevelInfoV1>,
    cosmic_toplevel_manager: Option<ZcosmicToplevelManagerV1>,
    cosmic_toplevels: HashMap<u32, (ZcosmicToplevelHandleV1, ToplevelInfo)>,
    cosmic_minimized_by_us: Vec<u32>,
}

fn config_file_stamp(path: &PathBuf) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some((modified, meta.len()))
}

fn corner_anchor(corner: Corner) -> Anchor {
    match corner {
        Corner::TopLeft => Anchor::TOP | Anchor::LEFT,
        Corner::TopRight => Anchor::TOP | Anchor::RIGHT,
        Corner::BottomLeft => Anchor::BOTTOM | Anchor::LEFT,
        Corner::BottomRight => Anchor::BOTTOM | Anchor::RIGHT,
    }
}

impl DaemonState {
    fn rebuild_zones(&mut self) {
        self.zones.clear();
        let outputs: Vec<wl_output::WlOutput> = self.output_state.outputs().collect();
        let size = self.config.corner_size_px.clamp(2, 64);

        info!(
            "Rebuilding hot-corner zones across {} output(s) (size={}px, debug={})",
            outputs.len(),
            size,
            self.config.debug_overlay
        );

        for output in outputs {
            for corner in Corner::ALL {
                if !self.config.action_for(corner).is_enabled() {
                    continue;
                }
                let surface = self.compositor_state.create_surface(&self.qh);
                if let Ok(region) = Region::new(&self.compositor_state) {
                    region.add(0, 0, size as i32, size as i32);
                    surface.set_input_region(Some(region.wl_region()));
                }

                let layer = self.layer_shell.create_layer_surface(
                    &self.qh,
                    surface,
                    Layer::Overlay,
                    Some(format!("open-corners-{}", corner.slug())),
                    Some(&output),
                );
                layer.set_anchor(corner_anchor(corner));
                layer.set_size(size, size);
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                layer.commit();

                self.zones.push(CornerZone {
                    corner,
                    output: output.clone(),
                    layer,
                    width: size,
                    height: size,
                    configured: false,
                });
            }
        }
    }

    fn draw_zone(&mut self, zone_idx: usize) {
        let Some(zone) = self.zones.get(zone_idx) else {
            return;
        };
        let width = zone.width.max(1);
        let height = zone.height.max(1);
        let stride = (width * 4) as i32;

        let Ok((buffer, canvas)) = self.pool.create_buffer(
            width as i32,
            height as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) else {
            warn!("Failed to allocate shm buffer for {:?}", zone.corner);
            return;
        };

        // Premultiplied ARGB8888 in little-endian byte order: [B, G, R, A]
        let pixel: [u8; 4] = if self.config.debug_overlay {
            [0xC0, 0x80, 0x20, 0xB0]
        } else {
            [0x00, 0x00, 0x00, 0x00]
        };

        for chunk in canvas.chunks_exact_mut(4) {
            chunk.copy_from_slice(&pixel);
        }

        let wl_surface = zone.layer.wl_surface();
        if let Err(err) = buffer.attach_to(wl_surface) {
            warn!("Failed to attach buffer to {:?}: {err}", zone.corner);
            return;
        }
        wl_surface.damage_buffer(0, 0, width as i32, height as i32);
        wl_surface.commit();
    }

    fn handle_corner_enter(&mut self, corner: Corner) {
        self.hover_gen = self.hover_gen.wrapping_add(1);
        self.active_corner = Some(corner);
        let generation = self.hover_gen;

        if let Some(last) = self.last_triggered.get(&corner) {
            if last.elapsed() < Duration::from_millis(self.config.cooldown_ms) {
                return;
            }
        }

        let dwell = self.config.dwell_ms;
        if dwell == 0 {
            self.trigger_corner(corner);
            return;
        }

        let timer = Timer::from_duration(Duration::from_millis(dwell));
        if let Err(err) = self.loop_handle.insert_source(timer, move |_, _, state| {
            if state.hover_gen == generation && state.active_corner == Some(corner) {
                state.trigger_corner(corner);
            }
            TimeoutAction::Drop
        }) {
            warn!("Failed to schedule dwell timer: {err}");
        }
    }

    fn handle_corner_leave(&mut self, corner: Corner) {
        if self.active_corner == Some(corner) {
            self.active_corner = None;
            self.hover_gen = self.hover_gen.wrapping_add(1);
        }
    }

    fn trigger_corner(&mut self, corner: Corner) {
        let action = self.config.action_for(corner).clone();
        if !action.is_enabled() {
            return;
        }
        self.last_triggered.insert(corner, Instant::now());
        info!("Hot corner triggered: {:?} -> {:?}", corner, action);

        if matches!(action, Action::ShowDesktop) && self.try_toggle_show_desktop_wayland() {
            return;
        }

        if let Err(err) = actions::execute_action(&action) {
            error!("Action {:?} failed: {err:#}", action);
        }
    }

    /// Minimize all currently visible windows via COSMIC or wlr foreign-toplevel protocols,
    /// or restore the ones we previously minimized. Returns true if a native protocol handled it.
    fn try_toggle_show_desktop_wayland(&mut self) -> bool {
        if let Some(manager) = &self.cosmic_toplevel_manager {
            if !self.cosmic_toplevels.is_empty() {
                let visible_ids: Vec<u32> = self
                    .cosmic_toplevels
                    .iter()
                    .filter(|(_, (_, info))| !info.minimized)
                    .map(|(id, _)| *id)
                    .collect();

                if !visible_ids.is_empty() {
                    self.cosmic_minimized_by_us.clear();
                    for id in visible_ids {
                        if let Some((handle, _)) = self.cosmic_toplevels.get(&id) {
                            manager.set_minimized(handle);
                            self.cosmic_minimized_by_us.push(id);
                        }
                    }
                    info!(
                        "Show Desktop: minimized {} window(s) via COSMIC toplevel protocol",
                        self.cosmic_minimized_by_us.len()
                    );
                } else if !self.cosmic_minimized_by_us.is_empty() {
                    let to_restore = std::mem::take(&mut self.cosmic_minimized_by_us);
                    for id in to_restore {
                        if let Some((handle, _)) = self.cosmic_toplevels.get(&id) {
                            manager.unset_minimized(handle);
                        }
                    }
                    info!("Show Desktop: restored previously minimized COSMIC windows");
                }
                return true;
            }
        }

        if self.foreign_toplevel_manager.is_none() || self.toplevels.is_empty() {
            return false;
        }

        let visible_ids: Vec<u32> = self
            .toplevels
            .iter()
            .filter(|(_, (_, info))| !info.minimized)
            .map(|(id, _)| *id)
            .collect();

        if !visible_ids.is_empty() {
            self.minimized_by_us.clear();
            for id in visible_ids {
                if let Some((handle, _)) = self.toplevels.get(&id) {
                    handle.set_minimized();
                    self.minimized_by_us.push(id);
                }
            }
            info!(
                "Show Desktop: minimized {} window(s) via Wayland foreign-toplevel",
                self.minimized_by_us.len()
            );
        } else if !self.minimized_by_us.is_empty() {
            let to_restore = std::mem::take(&mut self.minimized_by_us);
            for id in to_restore {
                if let Some((handle, _)) = self.toplevels.get(&id) {
                    handle.unset_minimized();
                }
            }
            info!("Show Desktop: restored previously minimized windows");
        }

        true
    }

    fn check_config_reload(&mut self) {
        let new_stamp = config_file_stamp(&self.config_path);
        if new_stamp == self.config_stamp {
            return;
        }
        self.config_stamp = new_stamp;
        match AppConfig::load_from(&self.config_path) {
            Ok(new_cfg) => {
                if new_cfg != self.config {
                    info!("Config changed on disk, reloading hot corners");
                    self.config = new_cfg;
                    self.rebuild_zones();
                }
            }
            Err(err) => {
                warn!("Failed to reload config: {err:#}");
            }
        }
    }
}

impl CompositorHandler for DaemonState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for DaemonState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.rebuild_zones();
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.zones.retain(|z| z.output != output);
    }
}

impl LayerShellHandler for DaemonState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.zones
            .retain(|z| z.layer.wl_surface().id() != layer.wl_surface().id());
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(idx) = self
            .zones
            .iter()
            .position(|z| z.layer.wl_surface().id() == layer.wl_surface().id())
        {
            if configure.new_size.0 > 0 {
                self.zones[idx].width = configure.new_size.0;
            }
            if configure.new_size.1 > 0 {
                self.zones[idx].height = configure.new_size.1;
            }
            self.zones[idx].configured = true;
            self.draw_zone(idx);
        }
    }
}

impl SeatHandler for DaemonState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                self.pointers.push(pointer);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            for ptr in self.pointers.drain(..) {
                ptr.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for DaemonState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let corner = self
                .zones
                .iter()
                .find(|z| z.layer.wl_surface().id() == event.surface.id())
                .map(|z| z.corner);

            let Some(corner) = corner else {
                continue;
            };

            match event.kind {
                PointerEventKind::Enter { .. } => self.handle_corner_enter(corner),
                PointerEventKind::Leave { .. } => self.handle_corner_leave(corner),
                _ => {}
            }
        }
    }
}

impl ShmHandler for DaemonState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for DaemonState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for DaemonState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                let id = toplevel.id().protocol_id();
                state.toplevels.insert(id, (toplevel, ToplevelInfo::default()));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                state.foreign_toplevel_manager = None;
                state.toplevels.clear();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(DaemonState, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for DaemonState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::State { state: raw_states } => {
                let minimized = raw_states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .any(|v| v == zwlr_foreign_toplevel_handle_v1::State::Minimized as u32);
                if let Some((_, info)) = state.toplevels.get_mut(&id) {
                    info.minimized = minimized;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.toplevels.remove(&id);
                state.minimized_by_us.retain(|&x| x != id);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for DaemonState {
    fn event(
        state: &mut Self,
        _proxy: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            if let Some(info) = &state.cosmic_toplevel_info {
                let cosmic_handle = info.get_cosmic_toplevel(&toplevel, qh, ());
                let id = cosmic_handle.id().protocol_id();
                state
                    .cosmic_toplevels
                    .insert(id, (cosmic_handle, ToplevelInfo::default()));
            }
        }
    }

    wayland_client::event_created_child!(DaemonState, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for DaemonState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtForeignToplevelHandleV1,
        _event: ext_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZcosmicToplevelInfoV1, ()> for DaemonState {
    fn event(
        _state: &mut Self,
        _proxy: &ZcosmicToplevelInfoV1,
        _event: zcosmic_toplevel_info_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZcosmicToplevelHandleV1, ()> for DaemonState {
    fn event(
        state: &mut Self,
        proxy: &ZcosmicToplevelHandleV1,
        event: zcosmic_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            zcosmic_toplevel_handle_v1::Event::State { state: raw_states } => {
                let minimized = raw_states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .any(|v| v == zcosmic_toplevel_handle_v1::State::Minimized as u32);
                if let Some((_, info)) = state.cosmic_toplevels.get_mut(&id) {
                    info.minimized = minimized;
                }
            }
            zcosmic_toplevel_handle_v1::Event::Closed => {
                state.cosmic_toplevels.remove(&id);
                state.cosmic_minimized_by_us.retain(|&x| x != id);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZcosmicToplevelManagerV1, ()> for DaemonState {
    fn event(
        _state: &mut Self,
        _proxy: &ZcosmicToplevelManagerV1,
        _event: zcosmic_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_compositor!(DaemonState);
delegate_output!(DaemonState);
delegate_shm!(DaemonState);
delegate_seat!(DaemonState);
delegate_pointer!(DaemonState);
delegate_layer!(DaemonState);
delegate_registry!(DaemonState);

pub fn run_daemon() -> Result<()> {
    let config_path = AppConfig::config_path()?;
    let config = AppConfig::load_from(&config_path)?;
    if !config_path.exists() {
        let _ = config.save_to(&config_path);
    }

    let conn = Connection::connect_to_env()
        .context("Failed to connect to Wayland display. Is a Wayland session running?")?;

    let (globals, event_queue) =
        registry_queue_init::<DaemonState>(&conn).context("Failed to initialize Wayland globals")?;
    let qh = event_queue.handle();

    let compositor_state =
        CompositorState::bind(&globals, &qh).context("wl_compositor is not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context(
        "zwlr_layer_shell_v1 is not available on this compositor (required for Wayland hot corners)",
    )?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm is not available")?;
    let pool = SlotPool::new(64 * 64 * 4 * 8, &shm).context("Failed to create Wayland shm pool")?;

    let foreign_toplevel_manager = globals
        .bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ())
        .ok();
    if foreign_toplevel_manager.is_some() {
        info!("Bound zwlr_foreign_toplevel_management_v1 for native Show Desktop");
    }

    let cosmic_toplevel_info = globals
        .bind::<ZcosmicToplevelInfoV1, _, _>(&qh, 1..=2, ())
        .ok();
    let cosmic_toplevel_manager = globals
        .bind::<ZcosmicToplevelManagerV1, _, _>(&qh, 1..=4, ())
        .ok();
    let ext_toplevel_list = if cosmic_toplevel_info.is_some() && cosmic_toplevel_manager.is_some() {
        globals
            .bind::<ExtForeignToplevelListV1, _, _>(&qh, 1..=1, ())
            .ok()
    } else {
        None
    };
    if ext_toplevel_list.is_some() {
        info!("Bound COSMIC toplevel management protocols for native Show Desktop");
    }

    let mut event_loop: EventLoop<DaemonState> =
        EventLoop::try_new().context("Failed to create calloop event loop")?;
    let loop_handle = event_loop.handle();

    let mut state = DaemonState {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        shm,
        layer_shell,
        pool,
        loop_handle: loop_handle.clone(),
        qh: qh.clone(),
        config,
        config_stamp: config_file_stamp(&config_path),
        config_path,
        zones: Vec::new(),
        pointers: Vec::new(),
        hover_gen: 0,
        active_corner: None,
        last_triggered: HashMap::new(),
        foreign_toplevel_manager,
        toplevels: HashMap::new(),
        minimized_by_us: Vec::new(),
        _ext_toplevel_list: ext_toplevel_list,
        cosmic_toplevel_info,
        cosmic_toplevel_manager,
        cosmic_toplevels: HashMap::new(),
        cosmic_minimized_by_us: Vec::new(),
    };

    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow::anyhow!("Failed to insert WaylandSource into calloop: {e}"))?;

    // Periodic config watcher (500ms) for live reload when settings change
    let reload_timer = Timer::from_duration(Duration::from_millis(500));
    loop_handle
        .insert_source(reload_timer, |_, _, state| {
            state.check_config_reload();
            TimeoutAction::ToDuration(Duration::from_millis(500))
        })
        .map_err(|e| anyhow::anyhow!("Failed to register config reload timer: {e}"))?;

    service::write_pid_file()?;
    info!(
        "open-corners daemon started (DE: {})",
        actions::DesktopEnv::detect().name()
    );

    // Initial dispatch to discover outputs, then build corner zones if not yet built
    event_loop.dispatch(Some(Duration::from_millis(50)), &mut state)?;
    if state.zones.is_empty() {
        state.rebuild_zones();
    }

    let result = loop {
        if let Err(err) = event_loop.dispatch(None, &mut state) {
            break Err(anyhow::anyhow!("Event loop error: {err}"));
        }
    };

    service::remove_pid_file();
    result
}
