//! Logic behind the NetSpeedGauge taskbar readout.
// Window, font and drawing handles are opaque numbers Windows hands out, never dereferenced here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
// Drawing calls take a position, size and look; a struct per call would only hide that.
#![allow(clippy::too_many_arguments)]

pub use netspeedgauge_shared::{format, settings, usage_store};

pub mod calendar;
pub mod canvas;
pub mod disk_writer;
pub mod messages;
pub mod net_counters;
pub mod popup_ui;
pub mod readout;
pub mod settings_ui;
pub mod startup;
pub mod taskbar_dock;
pub mod theme;
pub mod updater;
pub mod usage_ui;
