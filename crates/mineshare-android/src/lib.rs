//! Native Android network engine. Android owns UI, permissions and audio devices.
mod engine;
pub use engine::Engine;
#[cfg(target_os = "android")]
mod jni;

#[cfg(test)]
mod tests;
