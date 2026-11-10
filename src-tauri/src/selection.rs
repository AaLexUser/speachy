//! Reads the text currently selected in the frontmost application.

/// Returns the selected text of the focused app, if any.
///
/// On macOS this asks the Accessibility API first, which does not disturb
/// anything. Apps that do not expose their selection that way (many
/// browsers and Electron apps) get a simulated Cmd+C instead, with the
/// clipboard put back afterwards.
pub fn selected_text() -> Option<String> {
    let text = platform::selected_text()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Text on the clipboard, if any.
pub fn clipboard_text() -> Option<String> {
    let text = arboard::Clipboard::new().ok()?.get_text().ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether the app may read selections and post key events. Always true
/// where the operating system does not gate this.
pub fn accessibility_granted() -> bool {
    platform::accessibility_granted(false)
}

/// Asks the system to show its permission prompt, if needed.
pub fn request_accessibility() -> bool {
    platform::accessibility_granted(true)
}

#[cfg(target_os = "macos")]
mod platform {
    use accessibility_sys::{
        kAXErrorSuccess, kAXFocusedUIElementAttribute, kAXSelectedTextAttribute, kAXTrustedCheckOptionPrompt,
        AXIsProcessTrustedWithOptions, AXUIElementCopyAttributeValue, AXUIElementCreateSystemWide, AXUIElementRef,
    };
    use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData};
    use std::ptr;
    use std::time::{Duration, Instant};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        /// Current modifier-key state of an event source.
        fn CGEventSourceFlagsState(state_id: u32) -> u64;
    }

    /// Virtual key code of the "C" key (kVK_ANSI_C).
    const KEY_C: u16 = 8;
    const HID_SYSTEM_STATE: u32 = 1;

    pub fn accessibility_granted(prompt: bool) -> bool {
        unsafe {
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let value = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
            let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
        }
    }

    pub fn selected_text() -> Option<String> {
        if let Some(text) = accessibility_selection().filter(|t| !t.trim().is_empty()) {
            return Some(text);
        }
        copy_selection()
    }

    /// The `AXSelectedText` of the focused UI element.
    fn accessibility_selection() -> Option<String> {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            let mut focused: CFTypeRef = ptr::null();
            let attribute = CFString::from_static_string(kAXFocusedUIElementAttribute);
            let status = AXUIElementCopyAttributeValue(system, attribute.as_concrete_TypeRef(), &mut focused);
            CFRelease(system as CFTypeRef);
            if status != kAXErrorSuccess || focused.is_null() {
                return None;
            }
            let mut value: CFTypeRef = ptr::null();
            let attribute = CFString::from_static_string(kAXSelectedTextAttribute);
            let status = AXUIElementCopyAttributeValue(focused as AXUIElementRef, attribute.as_concrete_TypeRef(), &mut value);
            CFRelease(focused);
            if status != kAXErrorSuccess || value.is_null() {
                return None;
            }
            if CFGetTypeID(value) != CFString::type_id() {
                CFRelease(value);
                return None;
            }
            // The copy rule applies: we own `value` and the wrapper releases it.
            Some(CFString::wrap_under_create_rule(value as CFStringRef).to_string())
        }
    }

    type SavedItem = Vec<(Retained<NSPasteboardType>, Retained<NSData>)>;

    /// Everything on the pasteboard, so it can be put back.
    fn save_pasteboard(pasteboard: &NSPasteboard) -> Vec<SavedItem> {
        let Some(items) = pasteboard.pasteboardItems() else { return Vec::new() };
        items
            .iter()
            .map(|item| {
                item.types()
                    .iter()
                    .filter_map(|ty| item.dataForType(&ty).map(|data| (ty, data)))
                    .collect()
            })
            .collect()
    }

    fn restore_pasteboard(pasteboard: &NSPasteboard, saved: Vec<SavedItem>) {
        pasteboard.clearContents();
        if saved.is_empty() {
            return;
        }
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = saved
            .into_iter()
            .map(|types| {
                let item = NSPasteboardItem::new();
                for (ty, data) in types {
                    item.setData_forType(&data, &ty);
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        pasteboard.writeObjects(&NSArray::from_retained_slice(&items));
    }

    fn post_copy() -> bool {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else { return false };
        for key_down in [true, false] {
            let Ok(event) = CGEvent::new_keyboard_event(source.clone(), KEY_C, key_down) else { return false };
            // Exactly Command, whatever modifiers are physically held.
            event.set_flags(CGEventFlags::CGEventFlagCommand);
            event.post(CGEventTapLocation::HID);
        }
        true
    }

    /// Waits (briefly) for the shortcut's modifier keys to be released, so
    /// they cannot combine with the simulated Cmd+C.
    fn wait_for_modifiers_released() {
        const MODIFIERS: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000; // shift, control, option, command
        let deadline = Instant::now() + Duration::from_millis(350);
        while Instant::now() < deadline {
            if unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) } & MODIFIERS == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Copies the selection with a simulated Cmd+C, reads it, and restores
    /// the previous clipboard contents.
    fn copy_selection() -> Option<String> {
        if !accessibility_granted(false) {
            return None;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        let saved = save_pasteboard(&pasteboard);
        let before = pasteboard.changeCount();
        wait_for_modifiers_released();
        if !post_copy() {
            return None;
        }
        let deadline = Instant::now() + Duration::from_millis(450);
        let mut copied = None;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(15));
            if pasteboard.changeCount() != before {
                // Give the app a moment to finish writing all its types.
                std::thread::sleep(Duration::from_millis(20));
                copied = pasteboard.stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string());
                break;
            }
        }
        if pasteboard.changeCount() != before {
            restore_pasteboard(&pasteboard, saved);
        }
        copied
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    use std::time::Duration;

    pub fn accessibility_granted(_prompt: bool) -> bool {
        true
    }

    pub fn selected_text() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            // X11 keeps the current selection in the PRIMARY buffer.
            use arboard::{GetExtLinux, LinuxClipboardKind};
            let primary = arboard::Clipboard::new().ok().and_then(|mut c| c.get().clipboard(LinuxClipboardKind::Primary).text().ok());
            if let Some(text) = primary.filter(|t| !t.trim().is_empty()) {
                return Some(text);
            }
        }
        copy_selection()
    }

    /// Copies the selection with a simulated Ctrl+C and restores the
    /// previous clipboard text.
    fn copy_selection() -> Option<String> {
        let mut clipboard = arboard::Clipboard::new().ok()?;
        let previous = clipboard.get_text().ok();
        let _ = clipboard.clear();
        let mut enigo = Enigo::new(&Settings::default()).ok()?;
        enigo.key(Key::Control, Direction::Press).ok()?;
        let copied = enigo.key(Key::Unicode('c'), Direction::Click);
        let _ = enigo.key(Key::Control, Direction::Release);
        copied.ok()?;
        let mut text = None;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(15));
            if let Ok(found) = clipboard.get_text() {
                if !found.is_empty() {
                    text = Some(found);
                    break;
                }
            }
        }
        match previous {
            Some(previous) => {
                let _ = clipboard.set_text(previous);
            }
            None => {
                let _ = clipboard.clear();
            }
        }
        text
    }
}
