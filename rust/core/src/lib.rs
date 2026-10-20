//! wh-core —— 桌面端与 Android 端共享的业务核心
//!
//! 设计原则：
//! 1. 零 UI 依赖，纯逻辑，可单测
//! 2. 所有网络模型字段全部 optional（官方声明 API 可无预警变更）
//! 3. 布局算法与平台无关，两端复用

pub mod api;
pub mod layout;
#[cfg(feature = "library")]
pub mod library;
pub mod models;
pub mod ratelimit;

pub use api::{validate_username, ApiError, WallhavenClient};
pub use layout::{JustifiedLayout, LayoutItem, LayoutRect, LayoutRow};
pub use models::{
    Collection, CollectionsResponse, SearchQuery, SearchResponse, UserSettings,
    UserSettingsResponse, Wallpaper,
};
pub use ratelimit::TokenBucket;
