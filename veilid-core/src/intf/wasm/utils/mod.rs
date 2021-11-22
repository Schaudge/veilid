#![cfg(target_arch = "wasm32")]

pub mod channel;

use crate::xx::*;
use core::sync::atomic::{AtomicI8, Ordering};
use js_sys::{global, Reflect};

cfg_if! {
    if #[cfg(feature = "wee_alloc")] {
        // When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
        // allocator.
        extern crate wee_alloc;
        #[global_allocator]
        static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
    }
}

#[wasm_bindgen]
extern "C" {
    // Use `js_namespace` here to bind `console.log(..)` instead of just
    // `log(..)`
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    pub fn console_log(s: &str);

    #[wasm_bindgen]
    pub fn alert(s: &str);
}

pub fn is_nodejs() -> bool {
    static CACHE: AtomicI8 = AtomicI8::new(-1);
    let cache = CACHE.load(Ordering::Relaxed);
    if cache != -1 {
        return cache != 0;
    }

    let res = js_sys::eval("process.release.name === 'node'")
        .map(|res| res.is_truthy())
        .unwrap_or_default();

    CACHE.store(res as i8, Ordering::Relaxed);
    res
}

pub fn is_browser() -> bool {
    static CACHE: AtomicI8 = AtomicI8::new(-1);
    let cache = CACHE.load(Ordering::Relaxed);
    if cache != -1 {
        return cache != 0;
    }

    let res = Reflect::has(&global().as_ref(), &"window".into()).unwrap_or_default();

    CACHE.store(res as i8, Ordering::Relaxed);

    res
}

pub fn is_browser_https() -> bool {
    static CACHE: AtomicI8 = AtomicI8::new(-1);
    let cache = CACHE.load(Ordering::Relaxed);
    if cache != -1 {
        return cache != 0;
    }

    let res = js_sys::eval("window.location.protocol === 'https'")
        .map(|res| res.is_truthy())
        .unwrap_or_default();

    CACHE.store(res as i8, Ordering::Relaxed);

    res
}

pub fn node_require(module: &str) -> JsValue {
    if !is_nodejs() {
        return JsValue::UNDEFINED;
    }

    let mut home = env!("CARGO_MANIFEST_DIR");
    if home.len() == 0 {
        home = ".";
    }

    match js_sys::eval(format!("require(\"{}/{}\")", home, module).as_str()) {
        Ok(v) => v,
        Err(e) => {
            panic!("node_require failed: {:?}", e);
        }
    }
}
