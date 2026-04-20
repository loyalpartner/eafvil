use smithay::delegate_output;
use smithay::wayland::output::OutputHandler;

use crate::EmskinState;

impl OutputHandler for EmskinState {}
delegate_output!(EmskinState);
