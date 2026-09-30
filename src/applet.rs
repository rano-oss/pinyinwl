// SPDX-License-Identifier: GPL-3.0-only
//! Panel applet for pinyinwl IME.
//! Shows "中"/"英"/"全" on the panel via D-Bus `ModeLabel`. Click opens a popup
//! with mode toggle, half/full width, and settings (D-Bus OpenSettings, with
//! `--settings` fallback if the IME is not running).

use cosmic::{
    app,
    app::Core,
    applet,
    iced::Subscription,
    iced::core::window,
    iced::{
        platform_specific::shell::commands::popup::{destroy_popup, get_popup},
        window::Id,
        Task,
    },
    prelude::*,
    widget::{self, autosize},
};
use std::sync::LazyLock;

const APP_ID: &str = "com.system76.CosmicAppletPinyin";
const DBUS_BUS_NAME: &str = "com.pinyinwl.InputMethod1";
const DBUS_OBJ_PATH: &str = "/com/pinyinwl/InputMethod1";

static AUTOSIZE_MAIN_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("autosize-main"));

fn main() -> cosmic::iced::Result {
    env_logger::init();
    cosmic::applet::run::<ImeApplet>(())
}

struct ImeApplet {
    core: Core,
    popup: Option<Id>,
    ime_status_text: String,
}

#[derive(Clone, Debug)]
enum Message {
    TogglePopup,
    PopupClosed(Id),
    ImeStatusText(String),
    ToggleMode,
    ToggleHalfFullWidth,
    OpenSettings,
}

impl cosmic::Application for ImeApplet {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, app::Task<Self::Message>) {
        let applet = ImeApplet {
            core,
            popup: None,
            ime_status_text: String::from("中"),
        };
        (applet, Task::none())
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup = Some(new_id);
                    let popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::ImeStatusText(text) => {
                self.ime_status_text = text;
            }
            Message::ToggleMode => {
                self.ime_status_text = if self.ime_status_text == "英" {
                    String::from("中")
                } else {
                    String::from("英")
                };
                tokio::spawn(async {
                    if let Err(e) = dbus_call("ToggleMode", &()).await {
                        log::error!("Failed to toggle mode: {}", e);
                    }
                });
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
            }
            Message::ToggleHalfFullWidth => {
                tokio::spawn(async {
                    if let Err(e) = dbus_call("ToggleHalfFullWidth", &()).await {
                        log::error!("Failed to toggle half/full width: {}", e);
                    }
                });
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
            }
            Message::OpenSettings => {
                tokio::spawn(async {
                    if let Err(e) = dbus_call("OpenSettings", &()).await {
                        log::warn!(
                            "OpenSettings via D-Bus failed ({e}); falling back to pinyinwl --settings"
                        );
                        match std::process::Command::new("pinyinwl")
                            .arg("--settings")
                            .spawn()
                        {
                            Ok(mut child) => {
                                let _ = child.wait();
                            }
                            Err(spawn_err) => {
                                log::error!("Failed to launch settings: {spawn_err}");
                            }
                        }
                    }
                });
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let label = if self.ime_status_text.is_empty() {
            "中".to_string()
        } else {
            self.ime_status_text.clone()
        };

        let content: Element<'_, Self::Message> = self
            .core
            .applet
            .text_button(self.core.applet.text(label), Message::TogglePopup)
            .into();

        autosize::autosize(content, AUTOSIZE_MAIN_ID.clone()).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let is_passthrough = self.ime_status_text == "英";

        let mut list = widget::column::with_capacity(4).padding([8, 0]);

        if is_passthrough {
            list = list.push(
                applet::menu_button(widget::text::body("Switch to Chinese"))
                    .on_press(Message::ToggleMode),
            );
        } else {
            list = list.push(
                applet::menu_button(widget::text::body("Switch to English"))
                    .on_press(Message::ToggleMode),
            );
        }

        list = list.push(
            applet::menu_button(widget::text::body("Half / Full Width"))
                .on_press(Message::ToggleHalfFullWidth),
        );

        list = list.push(applet::padded_control(widget::divider::horizontal::default()));

        list = list.push(
            applet::menu_button(widget::text::body("Settings…")).on_press(Message::OpenSettings),
        );

        self.core.applet.popup_container(list).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        mode_label_subscription().map(Message::ImeStatusText)
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

fn mode_label_subscription() -> Subscription<String> {
    use cosmic::iced::stream;

    Subscription::run(|| {
        stream::channel(8, |mut output| async move {
            loop {
                if let Err(e) = watch_mode_label(&mut output).await {
                    log::debug!("ModeLabel watch ended: {e}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        })
    })
}

async fn watch_mode_label(
    output: &mut cosmic::iced::futures::channel::mpsc::Sender<String>,
) -> zbus::Result<()> {
    use cosmic::iced::futures::{SinkExt, StreamExt};

    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, DBUS_BUS_NAME, DBUS_OBJ_PATH, DBUS_BUS_NAME).await?;

    let label: String = proxy.get_property("ModeLabel").await?;
    let _ = output.send(label).await;

    let mut changes = proxy.receive_property_changed::<String>("ModeLabel").await;
    while let Some(change) = changes.next().await {
        if let Ok(label) = change.get().await {
            let _ = output.send(label).await;
        }
    }
    Ok(())
}

async fn dbus_call(
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::Type + Sync),
) -> zbus::Result<()> {
    use std::sync::OnceLock;
    use tokio::sync::Mutex;

    static CONN: OnceLock<Mutex<Option<zbus::Connection>>> = OnceLock::new();
    let slot = CONN.get_or_init(|| Mutex::new(None));

    let mut guard = slot.lock().await;
    if guard.is_none() {
        *guard = Some(zbus::Connection::session().await?);
    }
    let conn = guard.as_ref().unwrap();
    match conn
        .call_method(
            Some(DBUS_BUS_NAME),
            DBUS_OBJ_PATH,
            Some(DBUS_BUS_NAME),
            method,
            body,
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(e) => {
            *guard = None;
            Err(e)
        }
    }
}
