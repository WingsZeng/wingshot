use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_client::delegate_noop;

use crate::runtime_data::RuntimeData;

delegate_noop!(RuntimeData: ignore WpViewporter);
delegate_noop!(RuntimeData: ignore WpViewport);
