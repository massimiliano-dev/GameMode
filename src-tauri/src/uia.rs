//! Which element has the keyboard focus, through UI Automation: the
//! on-screen keyboard opens by itself when it is a text field. Cross-process
//! COM, 10-100 ms per call: asked from its own thread, never the 8 ms
//! controller loop. Apps whose accessible tree is off (Steam's CEF) answer
//! with a pane, and elevated windows (UAC installers) don't answer at all:
//! then there is no auto-open, and the manual way stays.

#![cfg_attr(not(windows), allow(dead_code))]

/// A text field with the keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextField {
    /// Left, top, right and bottom edges of the field, in screen pixels:
    /// also tells one field from another.
    pub rect: [i32; 4],
    /// A password field.
    pub password: bool,
}

/// The focused text field, if the focus is in one. Never from the
/// controller loop: the UI Automation client is created on each calling
/// thread's first use, and the call is cross-process.
pub fn focused_text_field() -> Option<TextField> {
    #[cfg(windows)]
    {
        imp::focused_text_field()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, UIA_EditControlTypeId};

    use super::TextField;

    thread_local! {
        /// `None` until the first call; `Some(None)` when UIA isn't available.
        static CLIENT: RefCell<Option<Option<IUIAutomation>>> = const { RefCell::new(None) };
    }

    fn client() -> Option<IUIAutomation> {
        CLIENT.with(|c| {
            c.borrow_mut()
                .get_or_insert_with(|| unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
                })
                .clone()
        })
    }

    pub fn focused_text_field() -> Option<TextField> {
        let uia = client()?;
        unsafe {
            let element = uia.GetFocusedElement().ok()?;
            if element.CurrentControlType().ok()? != UIA_EditControlTypeId {
                return None;
            }
            if !element.CurrentHasKeyboardFocus().ok()?.as_bool() {
                return None;
            }
            let rect = element.CurrentBoundingRectangle().ok()?;
            let password = element.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
            Some(TextField { rect: [rect.left, rect.top, rect.right, rect.bottom], password })
        }
    }
}
