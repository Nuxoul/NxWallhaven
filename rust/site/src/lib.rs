//! Platform-neutral authenticated website operations, using the caller's browser session.
//! No API key, HTTP client, cookie store, browser creation, or login automation lives here.
mod collection_management;
mod website;
mod website_collections;

pub use collection_management::ManageOperation;
pub use website::{
    FavoriteState, FavoriteTarget, Request, Response, SiteClient, SiteError, SiteTransport,
};
