mod if_not_initialized;
mod init_test_service_authority;
/// Position managment instructions
///
/// We added this module in 2025 August as part of audit remediations.
/// One of the remediations was about margin.register_position() not allowing
/// registering AdapterCollateral owned by TokenAdmin::Adapter.
/// This functionality used to exist in a program that was removed as part of
/// taking over the codebase. Removing code that used the margin system made
/// justifying some features difficult.
mod positions;
mod slippy;
mod tokens;

pub use if_not_initialized::*;
pub use init_test_service_authority::*;
pub use positions::*;
pub use slippy::*;
pub use tokens::*;
