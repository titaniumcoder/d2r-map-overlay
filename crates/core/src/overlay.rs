//! Click-through Wayland layer-shell windows: the map in the top-right corner and the in-game
//! UI marks over the whole screen ([`ui_marks`]). `SIGUSR1` toggles both (the surfaces take no
//! keyboard, so a compositor binding sends it).

use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result, bail};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel};
use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
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

use crate::ui_marks::{self, UiMarks};

/// px from the right screen edge.
const MARGIN: i32 = 20;
/// px below the top edge: clears D2R's own clock and area-name lines.
const MARGIN_TOP: i32 = 70;

/// What the reader and focus threads tell the UI loop.
#[derive(Debug)]
pub enum Msg {
    /// Also ends a [`Msg::Hide`].
    Frame(Pixmap),
    /// No usable game state (menu, loading screen, game exited), or a town: hides the map.
    Hide,
    /// The UI marks to show; empty hides them.
    UiMarks(UiMarks),
    /// The game window has keyboard focus.
    Focus(bool),
}

/// Runs the overlay until the compositor closes it or every [`Msg`] sender is gone. It shows
/// once the game has focus and a frame has arrived.
///
/// # Errors
/// Fails without a Wayland session, without the wlr layer-shell protocol, or on a Wayland protocol error.
pub fn run(messages: Channel<Msg>, (width, height): (u32, u32)) -> Result<()> {
    // First, so an early SIGUSR1 doesn't terminate. The handler only writes a byte to this
    // socket pair (about all a signal handler may do safely); the loop toggles on the read end.
    let (toggles, on_signal) = UnixStream::pair().context("creating the SIGUSR1 socket pair")?;
    toggles
        .set_nonblocking(true)
        .context("making the SIGUSR1 socket non-blocking")?;
    signal_hook::low_level::pipe::register(signal_hook::consts::SIGUSR1, on_signal)
        .context("registering the SIGUSR1 handler")?;
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init(&conn).context("listing Wayland globals")?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("compositor lacks zwlr_layer_shell_v1")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm not available")?;

    let (map, marks) = create_panels(&compositor, &layer_shell, &qh, (width, height))?;

    let frame_bytes = usize::try_from(u64::from(width) * u64::from(height) * 4)?;
    let pool = SlotPool::new(frame_bytes, &shm).context("creating the shm pool")?;
    let mut overlay = Overlay {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        shm,
        pool,
        conn: conn.clone(),
        map,
        marks,
        ui_marks: UiMarks::default(),
        vis: Visibility::default(),
        closed: false,
        error: None,
    };

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
    event_loop
        .handle()
        .insert_source(
            Generic::new(toggles, Interest::READ, Mode::Level),
            |_, toggles, overlay: &mut Overlay| {
                let mut buf = [0u8; 64];
                // `&UnixStream` implements `Read`; calloop only hands out a shared reference.
                loop {
                    match (&**toggles).read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => (0..n).for_each(|_| overlay.toggle()),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(e) => {
                            eprintln!("SIGUSR1 toggle off: {e}");
                            break;
                        }
                    }
                }
                // EOF or a hard error: the level-triggered source would fire forever.
                Ok(PostAction::Remove)
            },
        )
        .map_err(|e| e.error)
        .context("adding the SIGUSR1 socket to the event loop")?;

    while !overlay.closed {
        event_loop
            .dispatch(None, &mut overlay)
            .context("Wayland dispatch")?;
    }
    overlay.error.map_or(Ok(()), Err)
}

/// The map's and the UI marks' click-through surfaces, not yet committed.
fn create_panels(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<Overlay>,
    map_size: (u32, u32),
) -> Result<(Panel, Panel)> {
    // Click-through: an empty input region sends pointer events to the window below.
    let region = Region::new(compositor).context("creating the input region")?;
    let layer = |namespace: &str| {
        let layer = layer_shell.create_layer_surface(
            qh,
            compositor.create_surface(qh),
            Layer::Overlay,
            Some(namespace),
            None,
        );
        layer.set_input_region(Some(region.wl_region()));
        layer
    };
    let map = Panel::new(layer("d2r-map-overlay"), Place::TopRight, map_size);
    let marks = Panel::new(layer("d2r-ui-marks"), Place::Screen, (0, 0));
    Ok((map, marks))
}

/// Premultiplied RGBA to `wl_shm` ARGB8888 (little-endian: bytes B, G, R, A).
pub fn rgba_to_argb8888(rgba: &[u8], out: &mut [u8]) {
    for (dst, &[r, g, b, a]) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
        *dst = [b, g, r, a];
    }
}

/// Whether the overlay should be on screen.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "four independent inputs, not a state machine"
)]
struct Visibility {
    focused: bool,
    has_frame: bool,
    hidden: bool,
    /// Switched off by `SIGUSR1`; stays off through frames and focus changes.
    user_hidden: bool,
}

impl Visibility {
    const fn on_msg(&mut self, msg: &Msg) {
        match msg {
            Msg::Frame(_) => {
                self.has_frame = true;
                self.hidden = false;
            }
            Msg::Hide => self.hidden = true,
            Msg::UiMarks(_) => {}
            Msg::Focus(focused) => self.focused = *focused,
        }
    }

    const fn visible(self) -> bool {
        self.focused && self.has_frame && !self.hidden && !self.user_hidden
    }

    /// The UI marks show in towns too, where the map hides (the vendors are there).
    const fn marks_visible(self, has_marks: bool) -> bool {
        self.focused && has_marks && !self.user_hidden
    }
}

/// Whether a surface can be shown: once configured it needs a frame (a screen-sized frame is
/// only drawn when the configure gave the size).
const fn drawable(has_frame: bool, surface: Surface) -> bool {
    has_frame || matches!(surface, Surface::Unmapped | Surface::AwaitingConfigure)
}

/// Where the layer surface is in the layer-shell map/unmap cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// Never committed, or unmapped by a null buffer: needs a buffer-less commit first.
    Unmapped,
    /// Buffer-less commit sent; a buffer may only be attached after the configure that answers it.
    AwaitingConfigure,
    /// Configure acked, no buffer attached yet.
    Configured,
    Mapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Nothing,
    /// Set the layer state and commit without a buffer (requests a configure).
    CommitEmpty,
    Present,
    /// Attach a null buffer and commit.
    Unmap,
}

/// The next step towards `visible` and the surface state after it.
const fn plan(surface: Surface, visible: bool, dirty: bool) -> (Action, Surface) {
    match (surface, visible) {
        (Surface::Unmapped, true) => (Action::CommitEmpty, Surface::AwaitingConfigure),
        (Surface::Configured, true) => (Action::Present, Surface::Mapped),
        (Surface::Mapped, true) if dirty => (Action::Present, Surface::Mapped),
        (Surface::Mapped, false) => (Action::Unmap, Surface::Unmapped),
        _ => (Action::Nothing, surface),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Below D2R's clock and area-name lines.
    TopRight,
    /// The whole output, so surface px are screen px; the compositor gives the size.
    Screen,
}

/// One layer surface and the frame it shows.
struct Panel {
    layer: LayerSurface,
    place: Place,
    /// (0, 0) for [`Place::Screen`] until the first configure.
    size: (u32, u32),
    frame: Option<Pixmap>,
    /// `frame` changed since it was last presented.
    dirty: bool,
    surface: Surface,
    /// The buffer on screen; kept alive while the compositor may read it.
    buffer: Option<Buffer>,
}

impl Panel {
    const fn new(layer: LayerSurface, place: Place, size: (u32, u32)) -> Self {
        Self {
            layer,
            place,
            size,
            frame: None,
            dirty: false,
            surface: Surface::Unmapped,
            buffer: None,
        }
    }

    fn show(&mut self, frame: Option<Pixmap>) {
        self.frame = frame;
        self.dirty = true;
    }

    /// One step towards `visible`.
    fn sync(&mut self, visible: bool, pool: &mut SlotPool) -> Result<()> {
        let (action, next) = plan(self.surface, visible, self.dirty);
        let done = match action {
            Action::Nothing => Ok(()),
            Action::CommitEmpty => {
                self.commit_empty();
                Ok(())
            }
            Action::Present => self.present(pool),
            Action::Unmap => {
                self.unmap();
                Ok(())
            }
        };
        self.surface = next;
        done
    }

    /// The protocol resets a layer surface on unmap, so this runs before every (re)map.
    fn commit_empty(&self) {
        match self.place {
            Place::TopRight => {
                self.layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
                self.layer.set_margin(MARGIN_TOP, MARGIN, 0, 0);
                self.layer.set_size(self.size.0, self.size.1);
            }
            Place::Screen => {
                self.layer
                    .set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                // Over bars' reserved space too: the game is fullscreen.
                self.layer.set_exclusive_zone(-1);
                self.layer.set_size(0, 0);
            }
        }
        self.layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        self.layer.commit();
    }

    fn unmap(&mut self) {
        self.layer.wl_surface().attach(None, 0, 0);
        self.layer.commit();
        self.buffer = None;
    }

    fn present(&mut self, pool: &mut SlotPool) -> Result<()> {
        let Some(frame) = &self.frame else {
            bail!("present without a frame");
        };
        let (w, h) = (frame.width(), frame.height());
        // A screen-sized frame keeps the stretched size the compositor gave.
        if (w, h) != self.size && self.place == Place::TopRight {
            // Double-buffered: applies with the commit below.
            self.layer.set_size(w, h);
            self.size = (w, h);
        }
        let (wi, hi) = (i32::try_from(w)?, i32::try_from(h)?);
        let (buffer, canvas) = pool
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

    /// The configure that answers [`Panel::commit_empty`] arrived; true if it was awaited.
    fn configured(&mut self, configure: &LayerSurfaceConfigure) -> bool {
        match self.place {
            Place::Screen if configure.new_size != (0, 0) => self.size = configure.new_size,
            _ if configure.new_size != self.size && configure.new_size != (0, 0) => eprintln!(
                "compositor sized an overlay surface {:?}, drawing {:?}",
                configure.new_size, self.size
            ),
            _ => {}
        }
        // A later configure only resizes the screen panel (the caller redraws it); the map's
        // next frame is drawn at its own size anyway.
        let awaited = self.surface == Surface::AwaitingConfigure;
        if awaited {
            self.surface = Surface::Configured;
        }
        awaited
    }
}

struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    conn: Connection,
    map: Panel,
    marks: Panel,
    /// What `marks` shows, drawn again when the screen size arrives.
    ui_marks: UiMarks,
    vis: Visibility,
    closed: bool,
    error: Option<anyhow::Error>,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("map", &(self.map.size, self.map.surface))
            .field("marks", &(self.marks.size, self.marks.surface))
            .field("vis", &self.vis)
            .finish_non_exhaustive()
    }
}

impl Overlay {
    fn handle(&mut self, msg: Msg) {
        self.vis.on_msg(&msg);
        match msg {
            Msg::Frame(frame) => self.map.show(Some(frame)),
            Msg::UiMarks(marks) => {
                self.ui_marks = marks;
                self.draw_marks();
            }
            Msg::Hide | Msg::Focus(_) => {}
        }
        self.sync();
    }

    /// Draws the UI marks at the screen size; none before the first configure.
    fn draw_marks(&mut self) {
        let frame =
            crate::info::font().and_then(|font| ui_marks::render(&self.ui_marks, self.marks.size, font));
        self.marks.show(frame);
    }

    fn toggle(&mut self) {
        self.vis.user_hidden = !self.vis.user_hidden;
        eprintln!(
            "SIGUSR1: overlay {}",
            if self.vis.user_hidden { "hidden" } else { "shown" }
        );
        self.sync();
    }

    /// Stops the loop on failure.
    fn sync(&mut self) {
        let map = self.map.sync(self.vis.visible(), &mut self.pool);
        let marks_visible = self.vis.marks_visible(!self.ui_marks.is_empty())
            && drawable(self.marks.frame.is_some(), self.marks.surface);
        let marks = self.marks.sync(marks_visible, &mut self.pool);
        if let Err(e) = map
            .and(marks)
            .and_then(|()| self.conn.flush().context("flushing the Wayland connection"))
        {
            self.error = Some(e.context("updating the overlay"));
            self.closed = true;
        }
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
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if *layer == self.marks.layer {
            let size = self.marks.size;
            let awaited = self.marks.configured(&configure);
            let resized = self.marks.size != size;
            if resized || (awaited && self.marks.frame.is_none()) {
                self.draw_marks();
            }
            if awaited || resized {
                self.sync();
            }
        } else if self.map.configured(&configure) {
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
    fn the_user_switch_hides_through_frames_and_focus() -> Result<()> {
        let mut v = Visibility::default();
        v.on_msg(&Msg::Focus(true));
        v.on_msg(&frame()?);
        v.user_hidden = true;
        assert!(!v.visible(), "switched off");
        v.on_msg(&frame()?);
        v.on_msg(&Msg::Focus(false));
        v.on_msg(&Msg::Focus(true));
        assert!(!v.visible(), "new frames and refocus don't undo the switch");
        v.user_hidden = false;
        assert!(v.visible(), "switched back on");
        Ok(())
    }

    #[test]
    fn the_ui_marks_show_in_town_but_follow_focus_and_the_user_switch() {
        let mut v = Visibility::default();
        v.on_msg(&Msg::Focus(true));
        v.on_msg(&Msg::Hide);
        assert!(v.marks_visible(true), "a town hides only the map");
        assert!(!v.marks_visible(false), "no marks, nothing to show");
        v.on_msg(&Msg::UiMarks(UiMarks::default()));
        assert!(!v.visible(), "a marks message doesn't end the map's hide");
        v.user_hidden = true;
        assert!(!v.marks_visible(true), "the switch hides both");
        v.user_hidden = false;
        v.on_msg(&Msg::Focus(false));
        assert!(!v.marks_visible(true), "unfocused");
    }

    #[test]
    fn a_configured_surface_without_a_frame_stays_hidden() {
        use Surface as S;
        assert!(drawable(false, S::Unmapped), "commit first to learn the size");
        assert!(drawable(false, S::AwaitingConfigure));
        assert!(
            !drawable(false, S::Configured),
            "no size from the compositor, no frame"
        );
        assert!(!drawable(false, S::Mapped), "the frame failed to draw: unmap");
        assert!(drawable(true, S::Configured));
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
        assert_eq!(
            plan(S::Unmapped, true, true),
            (A::CommitEmpty, S::AwaitingConfigure)
        );
        assert_eq!(
            plan(S::AwaitingConfigure, true, true),
            (A::Nothing, S::AwaitingConfigure)
        );
        assert_eq!(plan(S::Configured, true, false), (A::Present, S::Mapped));
        assert_eq!(plan(S::Mapped, true, true), (A::Present, S::Mapped));
        assert_eq!(plan(S::Mapped, true, false), (A::Nothing, S::Mapped));
        assert_eq!(plan(S::Mapped, false, true), (A::Unmap, S::Unmapped));
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
