mod android_get_if_addrs;
mod get_directories;
pub use android_get_if_addrs::*;
pub use get_directories::*;

use crate::xx::*;
use android_logger::{Config, FilterBuilder};
use backtrace::Backtrace;
use jni::{objects::GlobalRef, objects::JObject, objects::JString, JNIEnv, JavaVM};
use lazy_static::*;
use log::*;
use std::panic;

pub struct AndroidGlobals {
    pub vm: JavaVM,
    pub ctx: GlobalRef,
}

lazy_static! {
    pub static ref ANDROID_GLOBALS: Arc<Mutex<Option<AndroidGlobals>>> = Arc::new(Mutex::new(None));
}

pub fn veilid_core_setup_android<'a>(
    env: JNIEnv<'a>,
    ctx: JObject<'a>,
    log_tag: &'a str,
    log_level: Level,
) {
    android_logger::init_once(
        Config::default()
            .with_min_level(log_level)
            .with_tag(log_tag)
            .with_filter(
                FilterBuilder::new()
                    .filter(Some(log_tag), log_level.to_level_filter())
                    .build(),
            ),
    );
    panic::set_hook(Box::new(|panic_info| {
        let bt = Backtrace::new();
        if let Some(location) = panic_info.location() {
            error!(
                "panic occurred in file '{}' at line {}",
                location.file(),
                location.line(),
            );
        } else {
            error!("panic occurred but can't get location information...");
        }
        if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            error!("panic payload: {:?}", s);
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            error!("panic payload: {:?}", s);
        } else if let Some(a) = panic_info.payload().downcast_ref::<std::fmt::Arguments>() {
            error!("panic payload: {:?}", a);
        } else {
            error!("no panic payload");
        }
        error!("Backtrace:\n{:?}", bt);
    }));

    *ANDROID_GLOBALS.lock() = Some(AndroidGlobals {
        vm: env.get_java_vm().unwrap(),
        ctx: env.new_global_ref(ctx).unwrap(),
    });
}
