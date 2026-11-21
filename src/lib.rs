//! quota-dash HTTP service library (testable without binding a port).

pub mod catalog;
pub mod chromium_safe_storage;
pub mod config;
pub mod i18n;
pub mod models;
pub mod providers;
pub mod refresh;
pub mod routes;
pub mod secret_store;
pub mod state;
pub mod stub;

pub use config::Config;
pub use providers::antigravity;
pub use routes::app;
pub use state::{AppState, ProviderApis};
