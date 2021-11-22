//! Test suite for the Web and headless browsers.
#![cfg(target_arch = "wasm32")]

extern crate alloc;
extern crate wasm_bindgen_test;
use core::sync::atomic::AtomicBool;
use veilid_wasm::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

static SETUP_ONCE: AtomicBool = AtomicBool::new(false);
pub fn setup() -> () {
    if SETUP_ONCE
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        console_log("setup()");
        console_error_panic_hook::set_once();
        wasm_logger::init(wasm_logger::Config::new(Level::Trace));
        init_callbacks();
    }
}
// xxx needs updating to new keys and veilid_api object
fn init_callbacks() {
    assert_eq!(js_sys::eval(r#"
    window.sleep = (milliseconds) => { return new Promise(resolve => setTimeout(resolve, milliseconds)) };
    window.stateChangeCallback = async (stateChange) => { console.log("State change: " + stateChange); };
    window.configCallback = (configKey) => { 
        switch(configKey) {
            case "namespace": return "";
            case "capabilities.protocol_udp": return false;
            case "capabilities.protocol_connect_tcp": return false;
            case "capabilities.protocol_accept_tcp": return false;
            case "capabilities.protocol_connect_ws": return true;
            case "capabilities.protocol_accept_ws": return false;
            case "capabilities.protocol_connect_wss": return true;
            case "capabilities.protocol_accept_wss": return false;
            case "tablestore.directory": return "";
            case "network.max_connections": return 16;
            case "network.node_id": return "ZLd4uMYdP4qYLtxF6GqrzBb32Z6T3rE2FWMkWup1pdY";
            case "network.node_id_secret": return "s2Gvq6HJOxgQh-3xIgfWSL3I-DWZ2c1RjZLJl2Xmg2E";
            case "network.bootstrap": return [];
            case "network.rpc.concurrency": return 2;
            case "network.rpc.queue_size": return 128;
            case "network.rpc.max_timestamp_behind": return 10000000;
            case "network.rpc.max_timestamp_ahead": return 10000000;
            case "network.rpc.timeout": return 10000000;
            case "network.rpc.max_route_hop_count": return 7;
            case "network.dht.resolve_node_timeout": return null;
            case "network.dht.resolve_node_count": return 20;
            case "network.dht.resolve_node_fanout": return 3;
            case "network.dht.max_find_node_count": return 20;
            case "network.dht.get_value_timeout": return null;
            case "network.dht.get_value_count": return 20;
            case "network.dht.get_value_fanout": return 3;
            case "network.dht.set_value_timeout": return null;
            case "network.dht.set_value_count": return 20;
            case "network.dht.set_value_fanout": return 5;
            case "network.dht.min_peer_count": return 20;
            case "network.dht.min_peer_refresh_time": return 2000000;
            case "network.dht.validate_dial_info_receipt_time": return 5000000;
            case "network.upnp": return false;
            case "network.natpmp": return false;
            case "network.address_filter": return true;
            case "network.tls.certificate_path": return "";
            case "network.tls.private_key_path": return "";
            case "network.application.path": return "/app";
            case "network.application.https.enabled": return false;
            case "network.application.https.listen_address": return "";
            case "network.application.http.enabled": return false;
            case "network.application.http.listen_address": return "";
            case "network.protocol.udp.enabled": return false;
            case "network.protocol.udp.socket_pool_size": return 0;
            case "network.protocol.udp.listen_address": return "";
            case "network.protocol.udp.public_address": return "";
            case "network.protocol.tcp.connect": return false;
            case "network.protocol.tcp.listen": return false;
            case "network.protocol.tcp.max_connections": return 32;
            case "network.protocol.tcp.listen_address": return "";
            case "network.protocol.tcp.public_address": return "";
            case "network.protocol.ws.connect": return true;
            case "network.protocol.ws.listen": return false;
            case "network.protocol.ws.max_connections": return 16;
            case "network.protocol.ws.listen_address": return "";
            case "network.protocol.ws.path": return "/ws";
            case "network.protocol.ws.public_address": return "";
            case "network.protocol.wss.connect": return true;
            case "network.protocol.wss.listen": return false;
            case "network.protocol.wss.max_connections": return 16;
            case "network.protocol.wss.listen_address": return "";
            case "network.protocol.wss.path": return "/ws";
            case "network.protocol.wss.public_address": return "";
            default: 
                console.log("config key '" + key +"' doesn't exist"); break;
        }
    };
    true
    "#).expect("failed to eval"), JsValue::TRUE);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
///

#[wasm_bindgen_test]
fn test_construct() {
    setup();

    assert_eq!(
        js_sys::eval(
            r#"
        let vc = new VeilidCore();
        true
    "#
        )
        .expect("failed to eval"),
        JsValue::TRUE
    );
}

#[wasm_bindgen_test(async)]
async fn test_startup_shutdown() {
    setup();

    assert_eq!(
        JsFuture::from(
            js_sys::eval(
                r#"
        (async function() {
            let vc = new VeilidCore();
            await vc.startup(window.stateChangeCallback, window.configCallback);
            await vc.shutdown();
            return true;
        })().then(v => {
            console.log("finished: " + v);
            return v;
        });
    "#
            )
            .expect("failed to eval")
            .dyn_into::<Promise>()
            .unwrap()
        )
        .await,
        Ok(JsValue::TRUE)
    );
}

#[wasm_bindgen_test(async)]
async fn test_attach_detach() {
    setup();

    assert_eq!(
        JsFuture::from(
            js_sys::eval(
                r#"
        (async function() {
            let vc = new VeilidCore();
            await vc.startup(window.stateChangeCallback, window.configCallback);
            await vc.attach();
            await window.sleep(1000);
            await vc.detach();
            await vc.shutdown();
            return true;
        })().then(v => {
            console.log("finished: " + v);
            return v;
        });
    "#
            )
            .expect("failed to eval")
            .dyn_into::<Promise>()
            .unwrap()
        )
        .await,
        Ok(JsValue::TRUE)
    );
}
