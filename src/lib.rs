pub mod bootstrap;
pub mod bootstrap_update;
pub mod builder;
pub mod config;
pub mod credentials;
pub mod installer;
pub mod limine;
pub mod machine;
pub mod packages;
pub mod releases;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
