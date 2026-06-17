//! raw Binder event 到用户态结构的解码层。
//!
//! # 职责
//! - 校验来自共享 ABI 的 raw 字段范围。
//! - 把 inline raw event 转成便于存储和展示的 decoded event。

mod android_platform;
mod config;
mod decoder;
mod error;
mod event;

pub use android_platform::{
    AndroidPlatformMethod, AndroidPlatformMethods, AndroidPlatformMethodsPathError,
    parse_interface_token, set_android_platform_methods_tsv_path,
};
pub use config::DecoderConfig;
pub use decoder::Decoder;
pub use error::DecodeError;
pub use event::{DecodedEvent, DecodedTransaction};
