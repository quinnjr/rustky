use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::WaylandSurface,
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

#[cfg(any(feature = "rhai-scripting", feature = "python-scripting"))]
use crate::config::Module;
use crate::config::Config;
use crate::monitor::Monitor;
use crate::render::Renderer;
use crate::styled::StyledLine;

pub fn run(cfg: Config, renderer: Renderer, monitor: Monitor) {
    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, event_queue) = registry_queue_init(&conn).expect("failed to init registry");
    let qh: QueueHandle<RustkyState> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr_layer_shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let seat_state = SeatState::new(&globals, &qh);

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Bottom,
        Some("rustky".to_string()),
        None,
    );

    layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
    layer.set_size(cfg.window.width, cfg.window.height);
    layer.set_exclusive_zone(-1); // don't push other surfaces
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_margin(cfg.window.y, cfg.window.x, 0, 0);
    layer.commit();

    let pool = SlotPool::new(
        (cfg.window.width * cfg.window.height * 4) as usize,
        &shm,
    )
    .expect("failed to create shm pool");

    // Initialize scripting engines
    #[cfg(feature = "rhai-scripting")]
    let rhai_engine = {
        let mut engine = crate::scripting::rhai_engine::RhaiEngine::new();
        for module in &cfg.modules {
            if let Module::Rhai {
                code,
                file,
                function,
            } = module
            {
                if let Some(code_str) = code {
                    let key = format!("inline:{function}");
                    if let Err(e) = engine.compile_inline(&key, code_str) {
                        eprintln!("rustky: {e}");
                    }
                }
                if let Some(file_path) = file {
                    let resolved = cfg.resolve_script_path(file_path);
                    let resolved_str = resolved.to_string_lossy().to_string();
                    if let Err(e) = engine.compile_file(&resolved_str) {
                        eprintln!("rustky: {e}");
                    }
                }
            }
        }
        if let Some(ref hook_path) = cfg.general.on_draw_rhai {
            let resolved = cfg.resolve_script_path(hook_path);
            let resolved_str = resolved.to_string_lossy().to_string();
            if let Err(e) = engine.load_on_draw_hook(&resolved_str) {
                eprintln!("rustky: {e}");
            }
        }
        engine
    };

    #[cfg(feature = "python-scripting")]
    let python_engine = {
        let mut engine = crate::scripting::python_engine::PythonEngine::new();
        for module in &cfg.modules {
            if let Module::Python { file, .. } = module {
                let resolved = cfg.resolve_script_path(file);
                let resolved_str = resolved.to_string_lossy().to_string();
                if let Err(e) = engine.load_file(&resolved_str) {
                    eprintln!("rustky: {e}");
                }
            }
        }
        if let Some(ref hook_path) = cfg.general.on_draw_python {
            let resolved = cfg.resolve_script_path(hook_path);
            let resolved_str = resolved.to_string_lossy().to_string();
            if let Err(e) = engine.load_on_draw_hook(&resolved_str) {
                eprintln!("rustky: {e}");
            }
        }
        engine
    };

    let mut state = RustkyState {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        seat_state,
        shm,
        pool,
        layer,
        cfg,
        renderer,
        monitor,
        width: 0,
        height: 0,
        configured: false,
        buffer: None,
        scroll_offset: 0.0,
        content_height: 0.0,
        #[cfg(feature = "rhai-scripting")]
        rhai_engine,
        #[cfg(feature = "python-scripting")]
        python_engine,
    };

    let mut event_loop: EventLoop<RustkyState> =
        EventLoop::try_new().expect("failed to create event loop");

    let loop_handle = event_loop.handle();

    let wayland_source = WaylandSource::new(conn, event_queue);
    loop_handle
        .insert_source(wayland_source, |_, queue, state| {
            queue.dispatch_pending(state)
        })
        .expect("failed to insert wayland source");

    event_loop
        .dispatch(Some(Duration::from_millis(100)), &mut state)
        .expect("initial dispatch failed");

    let update_ms = state.cfg.general.update_interval_ms;
    loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(update_ms)),
            |_, _, state: &mut RustkyState| {
                state.draw();
                TimeoutAction::ToDuration(Duration::from_millis(
                    state.cfg.general.update_interval_ms,
                ))
            },
        )
        .expect("failed to insert timer");

    state.draw();

    loop {
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .expect("event loop error");
    }
}

struct RustkyState {
    registry: RegistryState,
    output: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    cfg: Config,
    renderer: Renderer,
    monitor: Monitor,
    width: u32,
    height: u32,
    configured: bool,
    buffer: Option<Buffer>,
    scroll_offset: f32,
    content_height: f32,
    #[cfg(feature = "rhai-scripting")]
    rhai_engine: crate::scripting::rhai_engine::RhaiEngine,
    #[cfg(feature = "python-scripting")]
    python_engine: crate::scripting::python_engine::PythonEngine,
}

impl RustkyState {
    fn draw(&mut self) {
        if !self.configured {
            return;
        }
        let w = self.width;
        let h = self.height;
        if w == 0 || h == 0 {
            return;
        }

        self.monitor.refresh();

        #[cfg(any(feature = "rhai-scripting", feature = "python-scripting"))]
        let ctx = self.monitor.snapshot();

        let mut lines: Vec<StyledLine> = Vec::new();

        for module in &self.cfg.modules {
            let module_lines = match module {
                #[cfg(feature = "rhai-scripting")]
                Module::Rhai {
                    code,
                    file,
                    function,
                } => {
                    if let Some(code_str) = code {
                        let _ = code_str;
                        let key = format!("inline:{function}");
                        self.rhai_engine
                            .execute_module(&key, function, &ctx, false)
                    } else if let Some(file_path) = file {
                        let resolved = self.cfg.resolve_script_path(file_path);
                        let resolved_str = resolved.to_string_lossy().to_string();
                        self.rhai_engine
                            .execute_module(&resolved_str, function, &ctx, true)
                    } else {
                        vec![StyledLine::plain(
                            "[rhai: no code or file specified]".into(),
                        )]
                    }
                }
                #[cfg(feature = "python-scripting")]
                Module::Python { file, function } => {
                    let resolved = self.cfg.resolve_script_path(file);
                    let resolved_str = resolved.to_string_lossy().to_string();
                    self.python_engine
                        .execute_module(&resolved_str, function, &ctx)
                }
                other => self.monitor.collect(other),
            };
            lines.extend(module_lines);
        }

        #[cfg(feature = "rhai-scripting")]
        let lines = if self.cfg.general.on_draw_rhai.is_some() {
            self.rhai_engine.run_on_draw_hook(lines, &ctx)
        } else {
            lines
        };

        #[cfg(feature = "python-scripting")]
        let lines = if self.cfg.general.on_draw_python.is_some() {
            self.python_engine.run_on_draw_hook(lines, &ctx)
        } else {
            lines
        };

        // Track content height and clamp scroll offset
        self.content_height = self.renderer.content_height(&lines);
        let max_scroll = (self.content_height - h as f32).max(0.0);
        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);

        let pixels =
            self.renderer
                .render_styled_lines_scroll(&lines, w, h, self.scroll_offset);

        let (buffer, canvas) = self
            .pool
            .create_buffer(w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        // skia-rs outputs RGBA (premultiplied), wayland ARGB8888 = BGRA in little-endian bytes
        for (i, chunk) in pixels.chunks_exact(4).enumerate() {
            let idx = i * 4;
            if idx + 3 < canvas.len() {
                canvas[idx] = chunk[2]; // B
                canvas[idx + 1] = chunk[1]; // G
                canvas[idx + 2] = chunk[0]; // R
                canvas[idx + 3] = chunk[3]; // A
            }
        }

        self.layer
            .wl_surface()
            .attach(Some(buffer.wl_buffer()), 0, 0);
        self.layer
            .wl_surface()
            .damage_buffer(0, 0, w as i32, h as i32);
        self.layer.wl_surface().commit();

        self.buffer = Some(buffer);
    }
}

// --- Seat + Pointer handling for scroll ---

impl SeatHandler for RustkyState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            let _ = self.seat_state.get_pointer(qh, &seat);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }
}

impl PointerHandler for RustkyState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Axis {
                vertical, horizontal: _, ..
            } = &event.kind
            {
                let scroll_amount = vertical.absolute as f32;
                if scroll_amount.abs() > 0.01 {
                    self.scroll_offset += scroll_amount;
                    let max_scroll =
                        (self.content_height - self.height as f32).max(0.0);
                    self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);
                    self.draw();
                }
            }
        }
    }
}

// --- Wayland handler boilerplate ---

impl CompositorHandler for RustkyState {
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

impl OutputHandler for RustkyState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
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
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for RustkyState {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
    ) {
        std::process::exit(0);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.width = if configure.new_size.0 > 0 {
            configure.new_size.0
        } else {
            self.cfg.window.width
        };
        self.height = if configure.new_size.1 > 0 {
            configure.new_size.1
        } else {
            self.cfg.window.height
        };

        let needed = (self.width * self.height * 4) as usize;
        if self.pool.len() < needed {
            self.pool.resize(needed).expect("failed to resize pool");
        }

        self.configured = true;
        self.draw();
    }
}

impl ShmHandler for RustkyState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for RustkyState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(RustkyState);
delegate_output!(RustkyState);
delegate_layer!(RustkyState);
delegate_shm!(RustkyState);
delegate_seat!(RustkyState);
delegate_pointer!(RustkyState);
delegate_registry!(RustkyState);
