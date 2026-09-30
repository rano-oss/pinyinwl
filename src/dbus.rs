//! D-Bus interface for external control of the IME.

use std::sync::OnceLock;

use tokio::sync::{mpsc, watch};
use zbus::interface;

use crate::Message;

static MODE_LABEL_TX: OnceLock<watch::Sender<String>> = OnceLock::new();

/// Publish the panel label (`中` / `英` / `全`) for D-Bus Property clients.
pub fn publish_mode_label(label: impl Into<String>) {
    if let Some(tx) = MODE_LABEL_TX.get() {
        let _ = tx.send(label.into());
    }
}

struct PinyinWlDbus {
    tx: mpsc::UnboundedSender<Message>,
    mode: watch::Receiver<String>,
}

#[interface(name = "com.pinyinwl.InputMethod1")]
impl PinyinWlDbus {
    #[zbus(property)]
    async fn mode_label(&self) -> String {
        self.mode.borrow().clone()
    }

    async fn toggle_mode(&self) {
        let _ = self.tx.send(Message::DbusToggleMode);
    }

    async fn set_passthrough(&self, passthrough: bool) {
        let _ = self.tx.send(Message::DbusSetPassthrough(passthrough));
    }

    async fn toggle_half_full_width(&self) {
        let _ = self.tx.send(Message::DbusToggleHalfFullWidth);
    }

    async fn open_settings(&self) {
        let _ = self.tx.send(Message::DbusOpenSettings);
    }
}

/// Subscription that registers D-Bus service and forwards method calls as Messages.
pub fn dbus_subscription() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(10, async |mut sender| {
        use cosmic::iced::futures::SinkExt;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let (mode_tx, mode_rx) = watch::channel(String::from("中"));
        let _ = MODE_LABEL_TX.set(mode_tx.clone());

        let dbus_obj = PinyinWlDbus {
            tx,
            mode: mode_rx.clone(),
        };

        let conn = match zbus::Connection::session().await {
            Ok(conn) => conn,
            Err(e) => {
                log::error!("Failed to connect to D-Bus session bus: {}", e);
                std::future::pending::<()>().await;
                unreachable!()
            }
        };

        if let Err(e) = conn
            .object_server()
            .at("/com/pinyinwl/InputMethod1", dbus_obj)
            .await
        {
            log::error!("Failed to register D-Bus object: {}", e);
            std::future::pending::<()>().await;
            unreachable!()
        }

        if let Err(e) = conn.request_name("com.pinyinwl.InputMethod1").await {
            log::error!("Failed to request D-Bus name: {}", e);
            std::future::pending::<()>().await;
            unreachable!()
        }

        log::info!("D-Bus service registered: com.pinyinwl.InputMethod1");

        let iface = match conn
            .object_server()
            .interface::<_, PinyinWlDbus>("/com/pinyinwl/InputMethod1")
            .await
        {
            Ok(iface) => iface,
            Err(e) => {
                log::error!("Failed to get D-Bus interface ref: {}", e);
                std::future::pending::<()>().await;
                unreachable!()
            }
        };

        let mut mode_rx = mode_tx.subscribe();
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    let _ = sender.send(msg).await;
                }
                changed = mode_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = mode_rx.borrow_and_update();
                    let iface_guard = iface.get().await;
                    if let Err(e) = iface_guard
                        .mode_label_changed(iface.signal_emitter())
                        .await
                    {
                        log::warn!("Failed to emit ModeLabel changed: {}", e);
                    }
                }
            }
        }
    })
}
