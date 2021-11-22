use crate::attachment_manager::*;
use crate::dht::crypto::Crypto;
use crate::intf::*;
use crate::veilid_api::*;
use crate::veilid_config::*;
use crate::xx::*;

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        pub type StateChangeCallback = Arc<dyn Fn(VeilidStateChange) -> SystemPinBoxFuture<()>>;
    } else {
        pub type StateChangeCallback = Arc<dyn Fn(VeilidStateChange) -> SystemPinBoxFuture<()> + Send + Sync>;
    }
}

#[derive(Debug)]
pub enum VeilidStateChange {
    Attachment {
        old_state: AttachmentState,
        new_state: AttachmentState,
    },
}

#[derive(Debug)]
pub enum VeilidState {
    Attachment(AttachmentState),
}

pub struct VeilidCoreSetup {
    pub state_change_callback: StateChangeCallback,
    pub config_callback: ConfigCallback,
}

struct VeilidCoreInner {
    config: Option<VeilidConfig>,
    table_store: Option<TableStore>,
    crypto: Option<Crypto>,
    attachment_manager: Option<AttachmentManager>,
}

#[derive(Clone)]
pub struct VeilidCore {
    inner: Arc<Mutex<VeilidCoreInner>>,
}

impl VeilidCore {
    fn new_inner() -> VeilidCoreInner {
        VeilidCoreInner {
            config: None,
            table_store: None,
            crypto: None,
            attachment_manager: None,
        }
    }
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Self::new_inner())),
        }
    }

    pub(crate) fn config(&self) -> VeilidConfig {
        self.inner.lock().config.as_ref().unwrap().clone()
    }

    pub(crate) fn attachment_manager(&self) -> AttachmentManager {
        self.inner
            .lock()
            .attachment_manager
            .as_ref()
            .unwrap()
            .clone()
    }

    pub(crate) fn table_store(&self) -> TableStore {
        self.inner.lock().table_store.as_ref().unwrap().clone()
    }

    pub(crate) fn crypto(&self) -> Crypto {
        self.inner.lock().crypto.as_ref().unwrap().clone()
    }

    // internal startup
    async fn internal_startup(&self, setup: VeilidCoreSetup) -> Result<VeilidAPI, String> {
        trace!("VeilidCore::internal_startup starting");

        cfg_if! {
            if #[cfg(target_os = "android")] {
                if utils::android::ANDROID_GLOBALS.lock().is_none() {
                    error!("Android globals are not set up");
                    return Err("Android globals are not set up".to_owned());
                }
            }
        }

        // Set up config
        trace!("VeilidCore::internal_startup init config");
        let mut config = VeilidConfig::new();
        config.init(setup.config_callback).await?;
        self.inner.lock().config = Some(config.clone());

        // Set up tablestore
        trace!("VeilidCore::internal_startup init tablestore");
        let table_store = TableStore::new(config.clone());
        table_store.init().await?;
        self.inner.lock().table_store = Some(table_store.clone());

        // Set up crypto
        trace!("VeilidCore::internal_startup init crypto");
        let crypto = Crypto::new(config.clone(), table_store.clone());
        crypto.init().await?;
        self.inner.lock().crypto = Some(crypto.clone());

        // Set up attachment manager
        trace!("VeilidCore::internal_startup init attachment manager");
        let cb = setup.state_change_callback;
        let attachment_manager =
            AttachmentManager::new(config.clone(), table_store.clone(), crypto.clone());
        attachment_manager
            .init(Arc::new(
                move |old_state: AttachmentState, new_state: AttachmentState| {
                    cb(VeilidStateChange::Attachment {
                        old_state: old_state,
                        new_state: new_state,
                    })
                },
            ))
            .await?;
        self.inner.lock().attachment_manager = Some(attachment_manager.clone());

        // Set up the API
        trace!("VeilidCore::internal_startup init API");
        let this = self.clone();
        let veilid_api = VeilidAPI::new(attachment_manager, this);
        trace!("VeilidCore::internal_startup complete");
        Ok(veilid_api)
    }

    // called once at the beginning to start the node
    pub async fn startup(&self, setup: VeilidCoreSetup) -> Result<VeilidAPI, String> {
        // Ensure we never end up partially initialized
        match self.internal_startup(setup).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.clone().internal_shutdown().await;
                Err(e)
            }
        }
    }

    // stop the node gracefully because the veilid api was dropped
    pub(crate) async fn internal_shutdown(self) {
        let mut inner = self.inner.lock();
        trace!("VeilidCore::internal_shutdown starting");

        // Shut down up attachment manager
        if let Some(attachment_manager) = &inner.attachment_manager {
            attachment_manager.terminate().await;
            inner.attachment_manager = None;
        }

        // Shut down crypto
        if let Some(crypto) = &inner.crypto {
            crypto.terminate().await;
            inner.crypto = None;
        }

        // Shut down tablestore
        if let Some(table_store) = &inner.table_store {
            table_store.terminate().await;
            inner.table_store = None;
        }

        // Shut down config
        if let Some(config) = &inner.config {
            config.terminate().await;
            inner.config = None;
        }

        trace!("VeilidCore::shutdown complete");
    }

    //
}
