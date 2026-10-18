//! The macOS menu bar. winit's default menu would offer "About swgoh-ae3"
//! as macOS's standard About panel; instead [`crate::native_options`] turns
//! that menu off and [`install`] builds the standard one with muda, whose
//! About item opens the app's own About window (with "Check for updates").
//!
//! The app menu keeps Services, Hide, Hide Others, Show All and Quit (⌘Q),
//! and the Window menu Minimize, Zoom and Bring All to Front, as macOS
//! implements them. The Edit menu's items are the app's own: `AppKit`'s
//! versions send `copy:` and the like to the first responder, which winit's
//! view does not answer, so they would sit greyed out. Ours turn into the
//! events the shortcuts give egui ([`apply`]), so text fields keep working
//! from the menu and the keyboard alike.
//!
//! Windows and Linux have no app menu; the header's About button opens the
//! window everywhere. The menu items reach the app through an [`Inbox`],
//! drained before each frame, so this module needs no unsafe code.

use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui::{self, Event, Key, Modifiers, RawInput};

/// What a menu item asks of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open the About window.
    About,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The menu's commands, waiting for the next frame.
#[derive(Debug, Clone, Default)]
pub struct Inbox(Arc<Mutex<Vec<Command>>>);

impl Inbox {
    pub fn push(&self, command: Command) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(command);
    }

    #[must_use]
    pub fn take(&self) -> Vec<Command> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// What the frame has to do itself after [`apply`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requests {
    /// Open the About window.
    pub about: bool,
    /// Paste the clipboard, which only the integration can read: send
    /// `ViewportCommand::RequestPaste`.
    pub paste: bool,
}

/// A press and release of `key` with ⌘ (`shift` adds Shift), as the
/// keyboard shortcut would arrive.
fn shortcut(raw_input: &mut RawInput, key: Key, shift: bool) {
    // As egui-winit reports ⌘ on a Mac (Ctrl elsewhere, though only macOS
    // has this menu).
    let modifiers = Modifiers {
        shift,
        mac_cmd: cfg!(target_os = "macos"),
        ..Modifiers::COMMAND
    };
    for pressed in [true, false] {
        raw_input.events.push(Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        });
    }
}

/// Turns `commands` into the input of the frame about to run: the Edit
/// commands become the events their shortcuts give egui.
#[must_use]
pub fn apply(commands: &[Command], raw_input: &mut RawInput) -> Requests {
    let mut requests = Requests::default();
    for command in commands {
        match command {
            Command::About => requests.about = true,
            Command::Undo => shortcut(raw_input, Key::Z, false),
            Command::Redo => shortcut(raw_input, Key::Z, true),
            Command::Cut => raw_input.events.push(Event::Cut),
            Command::Copy => raw_input.events.push(Event::Copy),
            Command::Paste => requests.paste = true,
            Command::SelectAll => shortcut(raw_input, Key::A, false),
        }
    }
    requests
}

/// The menu bar, kept alive while the window is open.
#[cfg(target_os = "macos")]
pub struct Native {
    _menu: muda::Menu,
}

/// Builds the menu bar and sends its commands to `inbox`, waking `ctx`.
/// Call on the main thread once the application has launched, as eframe's
/// app creator runs.
#[cfg(target_os = "macos")]
pub fn install(ctx: &egui::Context, inbox: Inbox) -> Result<Native, String> {
    use muda::accelerator::{Accelerator, Code, Modifiers as Mods};
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

    let item = |id: &str, text: &str, mods: Mods, code: Code| {
        MenuItem::with_id(id, text, true, Some(Accelerator::new(mods, code)))
    };
    let name = crate::APP_NAME;
    let about = MenuItem::with_id("about", format!("About {name}"), true, None);
    let app = Submenu::with_items(
        name,
        true,
        &[
            &about,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some(&format!("Hide {name}"))),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(Some(&format!("Quit {name}"))),
        ],
    )
    .map_err(|e| e.to_string())?;
    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &item("undo", "Undo", Mods::META, Code::KeyZ),
            &item("redo", "Redo", Mods::META | Mods::SHIFT, Code::KeyZ),
            &PredefinedMenuItem::separator(),
            &item("cut", "Cut", Mods::META, Code::KeyX),
            &item("copy", "Copy", Mods::META, Code::KeyC),
            &item("paste", "Paste", Mods::META, Code::KeyV),
            &item("select-all", "Select All", Mods::META, Code::KeyA),
        ],
    )
    .map_err(|e| e.to_string())?;
    let window = Submenu::with_items(
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(Some("Zoom")),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::bring_all_to_front(None),
        ],
    )
    .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(&[&app, &edit, &window]).map_err(|e| e.to_string())?;
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();

    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = match event.id().as_ref() {
            "about" => Command::About,
            "undo" => Command::Undo,
            "redo" => Command::Redo,
            "cut" => Command::Cut,
            "copy" => Command::Copy,
            "paste" => Command::Paste,
            "select-all" => Command::SelectAll,
            _ => return,
        };
        inbox.push(command);
        ctx.request_repaint();
    }));
    Ok(Native { _menu: menu })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_commands_become_their_shortcuts() {
        let inbox = Inbox::default();
        for command in [Command::Copy, Command::SelectAll, Command::Paste] {
            inbox.push(command);
        }
        let mut input = RawInput::default();
        let requests = apply(&inbox.take(), &mut input);
        assert!(inbox.take().is_empty(), "drained");
        assert_eq!(
            requests,
            Requests {
                about: false,
                paste: true
            }
        );
        assert_eq!(input.events[0], Event::Copy);
        assert!(matches!(
            input.events[1],
            Event::Key { key: Key::A, pressed: true, modifiers, .. } if modifiers.command
        ));
        assert!(matches!(
            input.events[2],
            Event::Key {
                key: Key::A,
                pressed: false,
                ..
            }
        ));
        assert_eq!(input.events.len(), 3);
    }

    #[test]
    fn about_asks_for_the_window() {
        let mut input = RawInput::default();
        assert!(apply(&[Command::About], &mut input).about);
        assert_eq!(input.events, []);
    }
}
