//! The two HTML pages, built at compile time from `assets/`. Each script lives in its own
//! `.js` file so `node --check` can parse it.

pub const CONFIG_PAGE: &str = concat!(
    include_str!("../../../../assets/config.html"),
    "<script>\n",
    include_str!("../../../../assets/config.js"),
    "</script></body></html>\n"
);

pub const DASHBOARD_PAGE: &str = concat!(
    include_str!("../../../../assets/dashboard.html"),
    "<script>\n",
    include_str!("../../../../assets/dashboard.js"),
    "</script></body></html>\n"
);
