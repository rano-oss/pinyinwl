//! D-Bus control surface via [`ime_control`] (`org.inputmethod.Control1`).

use cosmic::iced::futures::Stream;
use ime_control::{self, BusIdentity, ControlEvent, MenuItem};

use crate::settings::CharacterSet;
use crate::Message;

pub const IDENTITY: BusIdentity = BusIdentity {
    well_known_name: "com.pinyinwl.InputMethod1",
    object_path: "/com/pinyinwl/InputMethod1",
};

pub fn menu_for_state(english: bool, charset: CharacterSet) -> Vec<MenuItem> {
    let mut items = ime_control::menu_cn_en_half_full_settings(english);
    let label = match charset {
        CharacterSet::Simplified => "Switch to Traditional (繁)",
        CharacterSet::Traditional => "Switch to Simplified (简)",
    };
    let settings = items.pop();
    items.push(MenuItem::new(ime_control::action::TOGGLE_CHARSET, label));
    if let Some(s) = settings {
        items.push(s);
    }
    items
}

/// Subscription that registers D-Bus and forwards control events as [`Message`]s.
pub fn dbus_subscription() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(10, async |mut sender| {
        use cosmic::iced::futures::SinkExt;

        let mut rx = ime_control::serve(
            IDENTITY,
            "中",
            menu_for_state(false, CharacterSet::Simplified),
            "pinyinwl --settings",
        )
        .await;

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                ControlEvent::Activate(id) => match id.as_str() {
                    ime_control::action::TOGGLE_MODE => Message::DbusToggleMode,
                    ime_control::action::HALF_FULL => Message::DbusToggleHalfFullWidth,
                    ime_control::action::TOGGLE_CHARSET => Message::DbusToggleCharset,
                    ime_control::action::SETTINGS => Message::DbusOpenSettings,
                    other => {
                        log::debug!("pinyinwl: ignore unknown Activate({other})");
                        continue;
                    }
                },
                ControlEvent::SetPassthrough(p) => Message::DbusSetPassthrough(p),
            };
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    })
}
