//! Click-through Wayland layer-shell window that shows a rendered [`Pixmap`] in the top-right corner.
//!
//! Thin on purpose: all drawing happens in [`crate::render`]; this module only owns the surface,
//! maps/unmaps it as [`Msg`]s arrive, and copies pixels into a shm buffer. Runs until killed (Ctrl+C).

use anyhow::{Context, Result, bail};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use tiny_skia::Pixmap;

/// Gap in px between the overlay and the right screen edge.
const MARGIN: i32 = 20;
/// Gap in px below the top edge: clears D2R's own clock and area-name lines in that corner.
const MARGIN_TOP: i32 = 70;

/// What the reader and focus threads tell the UI loop.
#[derive(Debug)]
pub enum Msg {
    /// A new rendered minimap; also ends a [`Msg::Hide`].
    Frame(Pixmap),
    /// No usable game state (menu, loading screen, game exited).
    Hide,
    /// Whether the game window has keyboard focus.
    Focus(bool),
}

/// Runs the overlay until the compositor closes it or every [`Msg`] sender is gone.
///
/// The surface starts hidden; it shows once focus is `true` and a frame has arrived.
/// `size` is the expected frame size (frames of another size are shown at their own size).
///
/// # Errors
/// Fails without a Wayland session, without the wlr layer-shell protocol, or on a Wayland protocol error.
pub fn run(messages: Channel<Msg>, (width, height): (u32, u32)) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init(&conn).context("listing Wayland globals")?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("compositor lacks zwlr_layer_shell_v1")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm not available")?;

    let layer = layer_shell.create_layer_surface(
        &qh,
        compositor.create_surface(&qh),
        Layer::Overlay,
        Some("d2r-map-overlay"),
        None,
    );
    // Click-through: an empty input region means pointer events go to the window below.
    // It is wl_surface state (kept across layer unmaps); the surface copies the region on set,
    // so dropping `region` at the end of `run` is fine.
    let region = Region::new(&compositor).context("creating the input region")?;
    layer.set_input_region(Some(region.wl_region()));

    let frame_bytes = usize::try_from(u64::from(width) * u64::from(height) * 4)?;
    let pool = SlotPool::new(frame_bytes, &shm).context("creating the shm pool")?;
    let mut overlay = Overlay {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        conn: conn.clone(),
        size: (width, height),
        frame: None,
        dirty: false,
        vis: Visibility::default(),
        surface: Surface::Unmapped,
        buffer: None,
        closed: false,
        error: None,
    };

    // calloop polls the Wayland socket and the channel's wake-up fd together, so a message wakes
    // the loop as promptly as a compositor event. `WaylandSource` flushes before every poll.
    let mut event_loop: EventLoop<'static, Overlay> =
        EventLoop::try_new().context("creating the event loop")?;
    WaylandSource::new(conn, queue)
        .insert(event_loop.handle())
        .map_err(|e| e.error)
        .context("adding the Wayland connection to the event loop")?;
    event_loop
        .handle()
        .insert_source(messages, |event, (), overlay: &mut Overlay| match event {
            channel::Event::Msg(msg) => overlay.handle(msg),
            channel::Event::Closed => {
                eprintln!("all overlay inputs stopped; exiting");
                overlay.closed = true;
            }
        })
        .map_err(|e| e.error)
        .context("adding the message channel to the event loop")?;

    while !overlay.closed {
        event_loop
            .dispatch(None, &mut overlay)
            .context("Wayland dispatch")?;
    }
    overlay.error.map_or(Ok(()), Err)
}

/// Copies tiny-skia's premultiplied RGBA into `wl_shm` ARGB8888 (a little-endian u32 = bytes B, G, R, A).
pub fn rgba_to_argb8888(rgba: &[u8], out: &mut [u8]) {
    for (dst, &[r, g, b, a]) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
        *dst = [b, g, r, a];
    }
}

/// Whether the overlay should be on screen; pure, so the combinations are unit-tested.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Visibility {
    focused: bool,
    has_frame: bool,
    hidden: bool,
}

impl Visibility {
    const fn on_msg(&mut self, msg: &Msg) {
        match msg {
            Msg::Frame(_) => {
                self.has_frame = true;
                self.hidden = false;
            }
            Msg::Hide => self.hidden = true,
            Msg::Focus(focused) => self.focused = *focused,
        }
    }

    const fn visible(self) -> bool {
        self.focused && self.has_frame && !self.hidden
    }
}

/// Where the layer surface is in the layer-shell map/unmap cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// Never committed, or unmapped by a null buffer: needs a buffer-less commit first.
    Unmapped,
    /// Buffer-less commit sent; a buffer may only be attached after the configure that answers it.
    AwaitingConfigure,
    /// Configure acked (sctk acks before calling our handler), no buffer attached yet.
    Configured,
    /// A buffer is attached: on screen.
    Mapped,
}

/// One protocol step [`Overlay::sync`] takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Nothing,
    /// Set the layer state and commit without a buffer (requests a configure).
    CommitEmpty,
    /// Attach the current frame and commit.
    Present,
    /// Attach a null buffer and commit.
    Unmap,
}

/// The next step towards `visible` and the surface state after it. `dirty`: the frame changed
/// since it was last presented.
const fn plan(surface: Surface, visible: bool, dirty: bool) -> (Action, Surface) {
    match (surface, visible) {
        (Surface::Unmapped, true) => (Action::CommitEmpty, Surface::AwaitingConfigure),
        (Surface::Configured, true) => (Action::Present, Surface::Mapped),
        (Surface::Mapped, true) if dirty => (Action::Present, Surface::Mapped),
        (Surface::Mapped, false) => (Action::Unmap, Surface::Unmapped),
        // Waiting for a configure, already up to date, or hidden and staying hidden.
        _ => (Action::Nothing, surface),
    }
}

struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    conn: Connection,
    /// Size of the layer surface (the latest frame's size).
    size: (u32, u32),
    frame: Option<Pixmap>,
    /// `frame` changed since it was last presented.
    dirty: bool,
    vis: Visibility,
    surface: Surface,
    /// The buffer on screen; kept alive while the compositor may read it.
    buffer: Option<Buffer>,
    closed: bool,
    /// Why the loop stopped, if it was a failure.
    error: Option<anyhow::Error>,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("size", &self.size)
            .field("vis", &self.vis)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl Overlay {
    fn handle(&mut self, msg: Msg) {
        self.vis.on_msg(&msg);
        if let Msg::Frame(frame) = msg {
            self.frame = Some(frame);
            self.dirty = true;
        }
        self.sync();
    }

    /// Moves the surface one step towards [`Visibility::visible`]; stops the loop on failure.
    fn sync(&mut self) {
        let (action, next) = plan(self.surface, self.vis.visible(), self.dirty);
        let done = match action {
            Action::Nothing => Ok(()),
            Action::CommitEmpty => {
                self.commit_empty();
                Ok(())
            }
            Action::Present => self.present(),
            Action::Unmap => {
                self.unmap();
                Ok(())
            }
        };
        self.surface = next;
        if let Err(e) = done.and_then(|()| self.conn.flush().context("flushing the Wayland connection")) {
            self.error = Some(e.context("updating the overlay"));
            self.closed = true;
        }
    }

    /// (Re)sends the layer state and commits without a buffer. The protocol resets a layer
    /// surface to its just-created state on unmap, so this runs before every (re)map.
    fn commit_empty(&self) {
        self.layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        self.layer.set_margin(MARGIN_TOP, MARGIN, 0, 0);
        self.layer.set_size(self.size.0, self.size.1);
        self.layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        self.layer.commit();
    }

    /// Attaching a null buffer unmaps the layer surface.
    fn unmap(&mut self) {
        self.layer.wl_surface().attach(None, 0, 0);
        self.layer.commit();
        self.buffer = None;
    }

    /// Copies the frame into a fresh shm buffer, damages the whole surface and commits.
    fn present(&mut self) -> Result<()> {
        let Some(frame) = &self.frame else {
            bail!("present without a frame");
        };
        let (w, h) = (frame.width(), frame.height());
        if (w, h) != self.size {
            // Double-buffered: applies with the commit below.
            self.layer.set_size(w, h);
            self.size = (w, h);
        }
        let (wi, hi) = (i32::try_from(w)?, i32::try_from(h)?);
        let (buffer, canvas) = self
            .pool
            .create_buffer(wi, hi, wi * 4, wl_shm::Format::Argb8888)
            .context("allocating a shm buffer")?;
        if canvas.len() != frame.data().len() {
            bail!(
                "shm buffer is {} bytes, frame is {}",
                canvas.len(),
                frame.data().len()
            );
        }
        rgba_to_argb8888(frame.data(), canvas);
        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, wi, hi);
        buffer.attach_to(surface).context("attaching the shm buffer")?;
        self.layer.commit();
        // Dropping the previous buffer is safe while the compositor holds it: sctk destroys it on release.
        self.buffer = Some(buffer);
        self.dirty = false;
        Ok(())
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if configure.new_size != self.size && configure.new_size != (0, 0) {
            eprintln!(
                "compositor sized the overlay {:?}, drawing {:?}",
                configure.new_size, self.size
            );
        }
        // Later configures (while mapped) need no action: the next frame is drawn at our size anyway.
        if self.surface == Surface::AwaitingConfigure {
            self.surface = Surface::Configured;
            self.sync();
        }
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

// sctk's surface bookkeeping tracks outputs, so the OutputState delegate is required even unused.
impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_registry!(Overlay);
// Routes every other Wayland event to sctk's per-object handlers (which call the traits above).
smithay_client_toolkit::delegate_dispatch2!(Overlay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_becomes_bgra_bytes() {
        let rgba = [1, 2, 3, 4, 10, 20, 30, 40];
        let mut out = [0; 8];
        rgba_to_argb8888(&rgba, &mut out);
        assert_eq!(out, [3, 2, 1, 4, 30, 20, 10, 40]);
        // As a little-endian u32 that is 0xAARRGGBB.
        assert_eq!(u32::from_le_bytes([3, 2, 1, 4]), 0x0401_0203);
    }

    fn frame() -> Result<Msg> {
        Ok(Msg::Frame(Pixmap::new(2, 2).context("pixmap")?))
    }

    #[test]
    fn visibility_needs_focus_and_a_frame() -> Result<()> {
        let mut v = Visibility::default();
        assert!(!v.visible(), "starts hidden");
        v.on_msg(&Msg::Focus(true));
        assert!(!v.visible(), "focused but no frame yet");
        v.on_msg(&frame()?);
        assert!(v.visible());
        v.on_msg(&Msg::Hide);
        assert!(!v.visible(), "reader hide wins over focus");
        v.on_msg(&Msg::Focus(false));
        v.on_msg(&Msg::Focus(true));
        assert!(!v.visible(), "refocus doesn't undo a hide");
        v.on_msg(&frame()?);
        assert!(v.visible(), "a new frame ends the hide");
        Ok(())
    }

    #[test]
    fn frame_while_unfocused_shows_on_refocus() -> Result<()> {
        let mut v = Visibility::default();
        v.on_msg(&frame()?);
        assert!(!v.visible(), "unfocused");
        v.on_msg(&Msg::Hide);
        v.on_msg(&frame()?);
        assert!(!v.visible(), "still unfocused");
        v.on_msg(&Msg::Focus(true));
        assert!(v.visible());
        v.on_msg(&Msg::Focus(false));
        assert!(!v.visible());
        Ok(())
    }

    #[test]
    fn surface_plan() {
        use Action as A;
        use Surface as S;
        // Map: buffer-less commit, wait for the configure, then present.
        assert_eq!(
            plan(S::Unmapped, true, true),
            (A::CommitEmpty, S::AwaitingConfigure)
        );
        assert_eq!(
            plan(S::AwaitingConfigure, true, true),
            (A::Nothing, S::AwaitingConfigure)
        );
        assert_eq!(plan(S::Configured, true, false), (A::Present, S::Mapped));
        // Mapped: redraw only when the frame changed; unmap when no longer visible.
        assert_eq!(plan(S::Mapped, true, true), (A::Present, S::Mapped));
        assert_eq!(plan(S::Mapped, true, false), (A::Nothing, S::Mapped));
        assert_eq!(plan(S::Mapped, false, true), (A::Unmap, S::Unmapped));
        // Hidden stays hidden, wherever the surface is.
        assert_eq!(plan(S::Unmapped, false, true), (A::Nothing, S::Unmapped));
        assert_eq!(
            plan(S::AwaitingConfigure, false, true),
            (A::Nothing, S::AwaitingConfigure)
        );
        assert_eq!(plan(S::Configured, false, true), (A::Nothing, S::Configured));
    }

    /// Drives `plan` like `Overlay` does (configure events included) through hide/show cycles.
    #[test]
    fn surface_remaps_after_unmap() {
        use Action as A;
        let mut surface = Surface::Unmapped;
        let mut actions = Vec::new();
        let mut step = |surface: &mut Surface, visible: bool, configure: bool| {
            if configure && *surface == Surface::AwaitingConfigure {
                *surface = Surface::Configured;
            }
            let (action, next) = plan(*surface, visible, true);
            *surface = next;
            actions.push(action);
        };
        step(&mut surface, true, false); // shown
        step(&mut surface, true, true); // configure arrives
        step(&mut surface, false, false); // focus lost
        step(&mut surface, true, false); // focus back
        step(&mut surface, false, false); // lost again before the configure
        step(&mut surface, false, true); // configure arrives while hidden
        step(&mut surface, true, false); // focus back: already configured
        assert_eq!(
            actions,
            [
                A::CommitEmpty,
                A::Present,
                A::Unmap,
                A::CommitEmpty,
                A::Nothing,
                A::Nothing,
                A::Present
            ]
        );
        assert_eq!(surface, Surface::Mapped);
    }
}
