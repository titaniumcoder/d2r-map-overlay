//! Click-through Wayland layer-shell window that shows a rendered [`Pixmap`] in the top-right corner.
//!
//! Thin on purpose: all drawing happens in [`crate::render`]; this module only owns the surface
//! and copies pixels into a shm buffer. Runs until killed (Ctrl+C).

use anyhow::{Context, Result, bail};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
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

/// Gap in px between the overlay and the top and right screen edges.
const MARGIN: i32 = 20;

/// Opens the overlay showing `frame` and blocks until the compositor closes it or the process is killed.
///
/// # Errors
/// Fails without a Wayland session, without the wlr layer-shell protocol, or on a Wayland protocol error.
pub fn show(frame: Pixmap) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, mut queue) = registry_queue_init(&conn).context("listing Wayland globals")?;
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
    layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
    layer.set_margin(MARGIN, MARGIN, 0, 0);
    layer.set_size(frame.width(), frame.height());
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Click-through: an empty input region means pointer events go to the window below.
    // The surface copies the region on set, so dropping `region` at the end of `show` is fine.
    let region = Region::new(&compositor).context("creating the input region")?;
    layer.set_input_region(Some(region.wl_region()));
    // First commit without a buffer; the compositor answers with a configure, then we draw.
    layer.commit();

    let pool = SlotPool::new(frame.data().len(), &shm).context("creating the shm pool")?;
    let mut overlay = Overlay {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        frame,
        configured: false,
        buffer: None,
        closed: false,
    };
    while !overlay.closed {
        queue
            .blocking_dispatch(&mut overlay)
            .context("Wayland dispatch")?;
    }
    Ok(())
}

/// Copies tiny-skia's premultiplied RGBA into `wl_shm` ARGB8888 (a little-endian u32 = bytes B, G, R, A).
pub fn rgba_to_argb8888(rgba: &[u8], out: &mut [u8]) {
    for (dst, &[r, g, b, a]) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
        *dst = [b, g, r, a];
    }
}

struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    frame: Pixmap,
    configured: bool,
    /// The buffer on screen; kept alive while the compositor may read it.
    buffer: Option<Buffer>,
    closed: bool,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("size", &(self.frame.width(), self.frame.height()))
            .field("configured", &self.configured)
            .finish_non_exhaustive()
    }
}

impl Overlay {
    /// Replaces the shown frame (M3: called for every new snapshot). Size changes are applied too.
    #[allow(
        dead_code,
        reason = "M3 pushes live frames through this; M2 shows one static frame"
    )]
    fn set_frame(&mut self, frame: Pixmap) -> Result<()> {
        if (frame.width(), frame.height()) != (self.frame.width(), self.frame.height()) {
            self.layer.set_size(frame.width(), frame.height());
        }
        self.frame = frame;
        if self.configured { self.present() } else { Ok(()) }
    }

    /// Copies the frame into a fresh shm buffer, damages the whole surface and commits.
    fn present(&mut self) -> Result<()> {
        let (w, h) = (self.frame.width(), self.frame.height());
        let (wi, hi) = (i32::try_from(w)?, i32::try_from(h)?);
        let (buffer, canvas) = self
            .pool
            .create_buffer(wi, hi, wi * 4, wl_shm::Format::Argb8888)
            .context("allocating a shm buffer")?;
        if canvas.len() != self.frame.data().len() {
            bail!(
                "shm buffer is {} bytes, frame is {}",
                canvas.len(),
                self.frame.data().len()
            );
        }
        rgba_to_argb8888(self.frame.data(), canvas);
        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, wi, hi);
        buffer.attach_to(surface).context("attaching the shm buffer")?;
        self.layer.commit();
        // Dropping the previous buffer is safe while the compositor holds it: sctk destroys it on release.
        self.buffer = Some(buffer);
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
        let want = (self.frame.width(), self.frame.height());
        if configure.new_size != want && configure.new_size != (0, 0) {
            eprintln!(
                "compositor sized the overlay {:?}, drawing {want:?}",
                configure.new_size
            );
        }
        if !self.configured {
            self.configured = true;
            if let Err(e) = self.present() {
                eprintln!("drawing the overlay failed: {e:#}");
                self.closed = true;
            }
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
}
