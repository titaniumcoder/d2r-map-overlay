//! The two HTML pages, built at compile time from `assets/` (no framework, no runtime file
//! reads). Each page's script lives in its own `.js` file so `node --check` can parse it.

/// The config page (`/config`). The user picks a build per character (selection-only
/// config); the gear itself is agent-authored and shown read-only, annotated with the live ✓
/// and strike-through.
pub const CONFIG_PAGE: &str = concat!(
    include_str!("../../assets/config.html"),
    "<script>\n",
    include_str!("../../assets/config.js"),
    "</script></body></html>\n"
);

/// The live dashboard page, dark-themed. Layout: a sticky character bar with the breakpoint
/// chips, then Sell / Keep, the vendor, the collapsible gear grid (equipped piece on top of
/// each slot card, the build's pieces below with their stats; open while `gear_attention` says
/// so, until the viewer clicks), skills, recipes, the farm list and the shared stash. The
/// script does the first-paint fetch, then follows the SSE stream (`render` is the onmessage
/// handler).
pub const DASHBOARD_PAGE: &str = concat!(
    include_str!("../../assets/dashboard.html"),
    "<script>\n",
    include_str!("../../assets/dashboard.js"),
    "</script></body></html>\n"
);
