// SPDX-License-Identifier: GPL-3.0-only
//! Panel applet for pinyinwl IME.
//! Shows "中"/"英" on the panel. Click opens a popup with mode toggle,
//! half/full width, and settings. Panel hides this applet when IME is inactive.

use cosmic::{
    app,
    app::Core,
    applet,
    iced::core::window,
    iced::{
        platform_specific::shell::commands::popup::{destroy_popup, get_popup},
        window::Id,
        Task,
    },
    prelude::*,
    widget::{self, autosize},
};
use std::{fmt::Display, sync::LazyLock};

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
    ime_status_text: Mode,
}

#[derive(Debug, PartialEq)]
enum Mode {
    中,
    英
}

impl Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Mode::中 => write!(f, "中"),
            Mode::英 => write!(f, "英")
        }
    }
}

#[derive(Clone, Debug)]
enum Message {
    TogglePopup,
    PopupClosed(Id),
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
            ime_status_text: Mode::中,
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
            Message::ToggleMode => {
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
                match std::process::Command::new("pinyinwl")
                    .arg("--settings")
                    .spawn()
                {
                    Ok(mut child) => {
                        tokio::spawn(async move {
                            let _ = child.wait();
                        });
                    }
                    Err(e) => log::error!("Failed to launch settings: {}", e),
                }
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let content: Element<'_, Self::Message> = self
            .core
            .applet
            .text_button(self.core.applet.text(self.ime_status_text.to_string()), Message::TogglePopup)
            .into();
        autosize::autosize(content, AUTOSIZE_MAIN_ID.clone()).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let is_passthrough = self.ime_status_text == Mode::英;

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
            applet::menu_button(widget::text::body("Settings…"))
                .on_press(Message::OpenSettings),
        );

        self.core.applet.popup_container(list).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

async fn dbus_call(method: &str, body: &(impl serde::Serialize + zbus::zvariant::Type + Sync)) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    conn.call_method(
        Some(DBUS_BUS_NAME),
        DBUS_OBJ_PATH,
        Some(DBUS_BUS_NAME),
        method,
        body,
    )
    .await?;
    Ok(())
}
